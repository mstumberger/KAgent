//! What a session read, and when the file was last changed (agent-loop.md C.2, kind `file`): the file tools
//! remember the mtime of each file a session read or wrote; the loop asks before every model request which of
//! them changed on disk since, and the edit tools refuse a file that did.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

/// At most this many files are remembered for a session (the oldest are forgotten first).
const MAX_FILES: usize = 200;

#[derive(Default)]
struct Seen {
    /// path -> (mtime when the session last saw it, order of the last sight)
    files: HashMap<String, (SystemTime, u64)>,
    clock: u64,
}

static SEEN: Mutex<Option<HashMap<String, Seen>>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut HashMap<String, Seen>) -> R) -> R {
    f(SEEN.lock().unwrap().get_or_insert_with(HashMap::new))
}

/// The same file by any spelling of its path (`~`, a symlink, `..`).
fn key(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.display().to_string())
}

fn mtime(path: &str) -> Option<SystemTime> {
    std::fs::metadata(Path::new(path)).and_then(|m| m.modified()).ok()
}

/// The session read or wrote `path` just now. A call with no session (a unit test, the API) remembers nothing.
pub fn remember(session: &str, path: &str) {
    if session.is_empty() {
        return;
    }
    let Some(at) = mtime(path) else { return };
    let path = key(path);
    with(|all| {
        let seen = all.entry(session.to_string()).or_default();
        seen.clock += 1;
        let order = seen.clock;
        seen.files.insert(path, (at, order));
        if seen.files.len() > MAX_FILES {
            if let Some(oldest) = seen.files.iter().min_by_key(|(_, (_, o))| *o).map(|(p, _)| p.clone()) {
                seen.files.remove(&oldest);
            }
        }
    });
}

/// Did `path` change on disk since the session saw it? False for a file the session never saw.
pub fn stale(session: &str, path: &str) -> bool {
    let path = key(path);
    let known = with(|all| all.get(session).and_then(|s| s.files.get(&path)).map(|(at, _)| *at));
    known.is_some_and(|at| mtime(&path).is_some_and(|now| now != at))
}

/// The files that changed since the session saw them, each once: what it knows of them is updated, so the
/// next call reports a file again only after another change. A file that is gone is forgotten.
pub fn changed(session: &str) -> Vec<String> {
    with(|all| {
        let Some(seen) = all.get_mut(session) else { return Vec::new() };
        let mut out = Vec::new();
        let mut gone = Vec::new();
        for (path, (at, _)) in seen.files.iter_mut() {
            match mtime(path) {
                Some(now) if now != *at => {
                    *at = now;
                    out.push(path.clone());
                }
                Some(_) => {}
                None => gone.push(path.clone()),
            }
        }
        for path in gone {
            seen.files.remove(&path);
        }
        out.sort();
        out
    })
}

/// The session ended: forget what it saw.
pub fn forget(session: &str) {
    with(|all| {
        all.remove(session);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(path: &Path, text: &str, secs: u64) {
        std::fs::write(path, text).unwrap();
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs)).unwrap();
    }

    #[test]
    fn a_file_that_changed_is_reported_once_and_a_file_never_seen_never() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
        touch(&a, "one", 0);
        touch(&b, "one", 0);
        let (a_s, b_s) = (a.display().to_string(), b.display().to_string());
        remember("seen-test", &a_s);
        assert!(changed("seen-test").is_empty() && !stale("seen-test", &a_s));
        touch(&a, "two", 5);
        touch(&b, "two", 5);
        assert!(stale("seen-test", &a_s) && !stale("seen-test", &b_s), "b was never seen by this session");
        assert!(!stale("other-session", &a_s));
        assert_eq!(changed("seen-test"), vec![a_s.clone()]);
        assert!(changed("seen-test").is_empty(), "reported once");
        touch(&a, "three", 9);
        assert_eq!(changed("seen-test"), vec![a_s.clone()], "again after another change");
        // Writing it oneself and remembering it again is not a change.
        touch(&a, "four", 12);
        remember("seen-test", &a_s);
        assert!(changed("seen-test").is_empty() && !stale("seen-test", &a_s));
        std::fs::remove_file(&a).unwrap();
        assert!(changed("seen-test").is_empty(), "a file that is gone is forgotten, not a change");
        forget("seen-test");
        assert!(!stale("seen-test", &b_s));
    }
}
