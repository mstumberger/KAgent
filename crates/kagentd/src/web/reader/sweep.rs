//! A reader browser left by a daemon that crashed (web.md section 8, "sweep"). A browser that holds the reader's
//! profile blocks the next read ("is another instance using the reader's profile?"), so at daemon start every
//! process that runs on THIS data folder's reader profile and is not a descendant of this daemon is ended: TERM, then
//! KILL after 3 s. What matches is decided by a pure function from a process list; nothing else is ever touched.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Time to end by themselves after TERM.
const GRACE: Duration = Duration::from_secs(3);
/// Names (the last part of the command) of Chromium-family browsers.
const BROWSERS: [&str; 5] = ["brave", "chrome", "chromium", "msedge", "microsoft-edge"];

/// One process: its id, its parent and its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub argv: Vec<String>,
}

fn is_browser(argv0: &str) -> bool {
    let name = argv0.rsplit('/').next().unwrap_or(argv0).to_lowercase();
    BROWSERS.iter().any(|b| name.contains(b))
}

/// Whether `pid` is `ancestor` or below it.
fn descends_from(procs: &[Proc], pid: u32, ancestor: u32) -> bool {
    let mut at = pid;
    // A parent chain is short; the bound keeps a list with a loop in it from running for ever.
    for _ in 0..64 {
        if at == ancestor {
            return true;
        }
        match procs.iter().find(|p| p.pid == at) {
            Some(p) if p.ppid != at && p.ppid != 0 => at = p.ppid,
            _ => return false,
        }
    }
    false
}

/// The processes to end: a Chromium-family browser whose command line holds exactly `--user-data-dir=<profile>`
/// (the renderers and the GPU process of a browser carry it too) and that is neither `own` nor below it. Any other
/// profile never matches: not a prefix of it, not the owner's own browser, not the window of KAgent.
pub fn stale(procs: &[Proc], profile: &Path, own: u32) -> Vec<u32> {
    let flag = format!("--user-data-dir={}", profile.display());
    procs
        .iter()
        .filter(|p| p.argv.first().is_some_and(|a| is_browser(a)))
        .filter(|p| p.argv.iter().any(|a| a.trim_end_matches('/') == flag))
        .filter(|p| !descends_from(procs, p.pid, own))
        .map(|p| p.pid)
        .collect()
}

/// The parent id from `/proc/<pid>/stat`: the field after the state, which follows the last `)` (a command may hold one).
pub fn parse_ppid(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

/// Every process of the machine this user may read, from `/proc`.
fn read_procs() -> Vec<Proc> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let raw = std::fs::read(e.path().join("cmdline")).ok()?;
            let argv: Vec<String> = raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
            let ppid = parse_ppid(&std::fs::read_to_string(e.path().join("stat")).ok()?)?;
            Some(Proc { pid, ppid, argv })
        })
        .collect()
}

async fn signal(name: &str, pids: &[u32]) {
    for pid in pids {
        let _ = tokio::process::Command::new("kill").arg(format!("-{name}")).arg(pid.to_string()).status().await;
    }
}

/// End what `stale` finds in the processes `list` gives: TERM, and KILL for what is still there after `grace`.
/// The number of processes it ended (or tried to).
pub async fn sweep(profile: &Path, own: u32, list: impl Fn() -> Vec<Proc> + Clone + Send + 'static, grace: Duration) -> usize {
    let found = {
        let list = list.clone();
        let profile = profile.to_path_buf();
        tokio::task::spawn_blocking(move || stale(&list(), &profile, own)).await.unwrap_or_default()
    };
    if found.is_empty() {
        return 0;
    }
    tracing::warn!("reader sweep: ending {} process(es) of a browser left on the reader's profile: {found:?}", found.len());
    signal("TERM", &found).await;
    let end = tokio::time::Instant::now() + grace;
    let mut left = found.clone();
    while !left.is_empty() && tokio::time::Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (l, profile) = (list.clone(), profile.to_path_buf());
        let now = tokio::task::spawn_blocking(move || stale(&l(), &profile, own)).await.unwrap_or_default();
        left.retain(|p| now.contains(p));
    }
    if !left.is_empty() {
        tracing::warn!("reader sweep: {left:?} did not end after TERM, KILL");
        signal("KILL", &left).await;
    }
    found.len()
}

/// Daemon start: sweep the reader profile of this data folder.
pub async fn at_start(data: &Path) {
    let profile: PathBuf = super::browser::profile_dir(data);
    sweep(&profile, std::process::id(), read_procs, GRACE).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "/home/owner/.local/share/kagent";

    fn p(pid: u32, ppid: u32, args: &[&str]) -> Proc {
        Proc { pid, ppid, argv: args.iter().map(|a| a.to_string()).collect() }
    }

    fn profile() -> PathBuf {
        super::super::browser::profile_dir(Path::new(DATA))
    }

    /// The machine: the owner's main browser (renderer too), the window of KAgent (its own profile), the `browser` tool's profile,
    /// a profile that only starts the same way, a leftover reader (a main process and two helpers), and this daemon with its own reader.
    fn machine() -> Vec<Proc> {
        let reader = format!("--user-data-dir={DATA}/reader-profile");
        vec![
            p(1, 0, &["/sbin/init"]),
            p(900, 1, &["/usr/lib/systemd/systemd", "--user"]),
            p(1000, 900, &["/opt/brave.com/brave/brave", "--profile-directory=Default"]),
            p(1001, 1000, &["/opt/brave.com/brave/brave", "--type=renderer", "--user-data-dir=/home/owner/.config/BraveSoftware/Brave-Browser"]),
            p(1100, 900, &["/usr/bin/google-chrome", "--app=http://127.0.0.1:7878/", &format!("--user-data-dir={DATA}/app-profile")]),
            p(1200, 900, &["/usr/bin/chromium", &format!("--user-data-dir={DATA}/browser-profile")]),
            p(1300, 900, &["/usr/bin/chromium", &format!("--user-data-dir={DATA}/reader-profile-old")]),
            p(1301, 900, &["/usr/bin/chromium", &format!("--user-data-dir=/home/other/.local/share/kagent/reader-profile")]),
            p(2000, 1, &["/usr/bin/chromium", "--headless=new", "--remote-debugging-port=0", &reader, "about:blank"]),
            p(2001, 2000, &["/usr/bin/chromium", "--type=zygote", &reader]),
            p(2002, 2001, &["/usr/bin/chromium", "--type=renderer", &reader]),
            p(3000, 900, &["/home/owner/KAgent/target/release/kagentd", "serve"]),
            p(3100, 3000, &["/usr/bin/chromium", "--headless=new", &reader, "about:blank"]),
            p(3101, 3100, &["/usr/bin/chromium", "--type=gpu-process", &reader]),
        ]
    }

    #[test]
    fn a_leftover_reader_is_found_and_no_other_browser_is() {
        let mut found = stale(&machine(), &profile(), 3000);
        found.sort();
        assert_eq!(found, vec![2000, 2001, 2002], "the leftover and its helpers; not the owner's browser, the window, the browser tool, a lookalike, or this daemon's own reader");
    }

    #[test]
    fn what_is_below_this_daemon_is_never_ended_and_what_is_not_below_it_is() {
        let procs = machine();
        assert!(stale(&procs, &profile(), 3000).iter().all(|p| *p < 3000));
        let all = stale(&procs, &profile(), 4242);
        assert!(all.contains(&3100) && all.contains(&3101), "a daemon with another pid does not own them");
        assert!(!stale(&procs, &profile(), 1).contains(&2000), "pid 1 is the parent of the leftover: it counts as below it");
    }

    #[test]
    fn only_this_data_folders_profile_and_only_browsers() {
        let other = Path::new("/home/other/.local/share/kagent/reader-profile");
        assert_eq!(stale(&machine(), other, 3000), vec![1301], "another data folder's reader profile is that one's own");
        assert!(stale(&machine(), Path::new("/home/owner/.config/BraveSoftware/Brave-Browser"), 3000) == vec![1001], "the function follows the profile it is given, nothing else");
        // A process that merely mentions the profile (an editor, a shell, a tail) is not a browser.
        let procs = vec![p(10, 1, &["/usr/bin/tail", "-f", &format!("{DATA}/reader-profile/chrome_debug.log"), &format!("--user-data-dir={DATA}/reader-profile")]), p(11, 1, &["vim", &format!("--user-data-dir={DATA}/reader-profile")])];
        assert!(stale(&procs, &profile(), 3000).is_empty());
        // A trailing slash is the same folder; a longer name is not.
        let slash = vec![p(10, 1, &["/usr/bin/brave-browser", &format!("--user-data-dir={DATA}/reader-profile/")])];
        assert_eq!(stale(&slash, &profile(), 3000), vec![10]);
        let longer = vec![p(10, 1, &["/usr/bin/brave-browser", &format!("--user-data-dir={DATA}/reader-profile2")])];
        assert!(stale(&longer, &profile(), 3000).is_empty());
        assert!(stale(&[], &profile(), 3000).is_empty());
        assert!(stale(&[p(10, 1, &[])], &profile(), 3000).is_empty(), "a process with no command line (a zombie)");
    }

    #[test]
    fn a_loop_in_the_parents_ends() {
        let procs = vec![p(5, 6, &["/usr/bin/chromium", &format!("--user-data-dir={DATA}/reader-profile")]), p(6, 5, &["x"])];
        assert_eq!(stale(&procs, &profile(), 3000), vec![5]);
    }

    #[test]
    fn the_parent_is_read_after_the_last_bracket_of_the_command() {
        assert_eq!(parse_ppid("2000 (chromium) S 1 2000 2000 0 -1 4194560"), Some(1));
        assert_eq!(parse_ppid("2001 (Web Content (x)) S 2000 2000 2000 0"), Some(2000), "a command may hold spaces and brackets");
        assert_eq!(parse_ppid("garbage"), None);
        assert_eq!(parse_ppid("1 (x)"), None);
    }

    /// A real process, started here and ended here: `chromium` is a link to python3 that sleeps, with the profile on its command line.
    #[tokio::test]
    async fn a_real_leftover_is_ended_and_a_browser_on_another_profile_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let python = ["/usr/bin/python3", "/usr/local/bin/python3"].into_iter().find(|p| Path::new(p).exists());
        let Some(python) = python else { return };
        let link = dir.path().join("chromium");
        std::os::unix::fs::symlink(python, &link).unwrap();
        let profile = dir.path().join("reader-profile");
        let start = |profile: &Path| {
            std::process::Command::new(&link).args(["-c", "import time; time.sleep(60)", &format!("--user-data-dir={}", profile.display())]).spawn().unwrap()
        };
        let (mut leftover, mut other) = (start(&profile), start(&dir.path().join("other-profile")));
        // Both are visible in /proc once exec has happened.
        for _ in 0..50 {
            let seen = read_procs();
            if seen.iter().filter(|p| p.argv.iter().any(|a| a.starts_with("--user-data-dir="))).count() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // `own` is a pid nothing is below, so the leftover (a child of this test) counts as a stranger's.
        let ended = sweep(&profile, u32::MAX, read_procs, Duration::from_secs(3)).await;
        assert_eq!(ended, 1, "only the one on the reader's profile");
        let status = tokio::task::spawn_blocking(move || {
            let status = leftover.wait().unwrap();
            let other_alive = other.try_wait().unwrap().is_none();
            let _ = other.kill();
            let _ = other.wait();
            (status, other_alive)
        })
        .await
        .unwrap();
        assert!(!status.0.success(), "the leftover was ended by a signal: {:?}", status.0);
        assert!(status.1, "the browser on another profile still runs");
    }
}
