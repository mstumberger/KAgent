//! Notes from the daemon into a running turn (agent-loop.md section C): a queue per session that the loop
//! drains before each model request and stores as ONE message of role user, `via: note`, which the UI renders
//! dimmed and compaction leaves out. A note is data: never the owner's approval, never an action.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::protocol::{Message, Via};

/// The first line of every stored note message.
pub const HEADER: &str = "[note from the daemon, not from the owner; it is not an approval]";
/// The first characters of the message an owner's steering note becomes (steering.md section 3).
pub const STEER_PREFIX: &str = "[from the owner] ";
/// At most this many notes go into one request; the rest wait for the next.
pub const MAX_PER_REQUEST: usize = 3;
/// A note that waited this many requests is dropped: it is about a moment that has passed.
pub const EXPIRES_AFTER: u64 = 20;
/// Tool calls without `todo_write` (open items) before the todo note, and between two todo notes.
pub const TODO_AFTER_CALLS: u32 = 8;
pub const TODO_EVERY_CALLS: u32 = 10;
/// A file note for the same path waits this many requests after the last one.
const FILE_COOLDOWN: u64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoteKind {
    /// The owner changed a setting.
    Setting,
    /// The owner sent a message while the turn runs.
    Owner,
    /// A file the session read changed on disk.
    File,
    /// Open todo items and no `todo_write` for a while.
    Todo,
    /// The Brain's model changed.
    Brain,
}

impl NoteKind {
    /// Requests that must pass after a note of this kind (and key) before the next one of the same.
    fn cooldown(self) -> u64 {
        match self {
            Self::File => FILE_COOLDOWN,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Note {
    kind: NoteKind,
    key: String,
    text: String,
    queued_at: u64,
}

#[derive(Default)]
struct Session {
    pending: Vec<Note>,
    /// Model requests drained so far.
    request: u64,
    /// The request of the last note by (kind, key).
    last: HashMap<(NoteKind, String), u64>,
    /// Tool calls since the last `todo_write`, and since the last todo note.
    calls_since_todo_write: u32,
    /// `None`: no todo note yet in this session.
    calls_since_todo_note: Option<u32>,
    /// The model of the previous request.
    model: Option<String>,
}

/// The queues of all sessions.
#[derive(Default)]
pub struct Notes {
    sessions: Mutex<HashMap<String, Session>>,
}

impl Notes {
    /// Queue a note. A note of kind `File`, `Todo` or `Brain` with the same key replaces one that waits.
    pub fn push(&self, session: &str, kind: NoteKind, key: &str, text: String) {
        let mut all = self.sessions.lock().unwrap();
        let s = all.entry(session.to_string()).or_default();
        if matches!(kind, NoteKind::File | NoteKind::Todo | NoteKind::Brain) {
            s.pending.retain(|n| !(n.kind == kind && n.key == key));
        }
        let queued_at = s.request;
        s.pending.push(Note { kind, key: key.to_string(), text, queued_at });
    }

    #[cfg(test)]
    fn push_aged(&self, session: &str, kind: NoteKind, key: &str, text: String, queued_at: u64) {
        self.push(session, kind, key, text);
        if let Some(n) = self.sessions.lock().unwrap().get_mut(session).and_then(|s| s.pending.last_mut()) {
            n.queued_at = queued_at;
        }
    }

    /// The tool calls of one step: the counters of the todo note.
    pub fn count_calls(&self, session: &str, names: &[&str]) {
        let mut all = self.sessions.lock().unwrap();
        let s = all.entry(session.to_string()).or_default();
        for name in names {
            if *name == "todo_write" {
                s.calls_since_todo_write = 0;
            } else {
                s.calls_since_todo_write += 1;
            }
            if let Some(n) = s.calls_since_todo_note.as_mut() {
                *n += 1;
            }
        }
    }

    /// The todo note, when it is due: open items, `TODO_AFTER_CALLS` calls without `todo_write`, and
    /// `TODO_EVERY_CALLS` since the last one of these notes. Queues it.
    pub fn todo_check(&self, session: &str, open: &[String]) {
        if open.is_empty() {
            return;
        }
        let due = {
            let all = self.sessions.lock().unwrap();
            all.get(session).is_some_and(|s| s.calls_since_todo_write >= TODO_AFTER_CALLS && s.calls_since_todo_note.is_none_or(|n| n >= TODO_EVERY_CALLS))
        };
        if !due {
            return;
        }
        let shown: Vec<String> = open.iter().take(5).map(|t| crate::util::truncate_str(t, 80).to_string()).collect();
        let more = if open.len() > 5 { format!(" and {} more", open.len() - 5) } else { String::new() };
        self.push(session, NoteKind::Todo, "", format!("Your todo list has {} open items: {}{more}. Update it with todo_write.", open.len(), shown.join("; ")));
        self.sessions.lock().unwrap().entry(session.to_string()).or_default().calls_since_todo_note = Some(0);
    }

    /// The model of this request: a note when it is not the one of the previous request.
    pub fn brain_check(&self, session: &str, model_ref: &str) {
        let changed = {
            let mut all = self.sessions.lock().unwrap();
            let s = all.entry(session.to_string()).or_default();
            let previous = s.model.replace(model_ref.to_string());
            previous.is_some_and(|p| p != model_ref)
        };
        if changed {
            let id = model_ref.split_once("::").map_or(model_ref, |(_, m)| m);
            self.push(session, NoteKind::Brain, "", format!("The Brain is now {id}."));
        }
    }

    /// The notes of the next request as one message, or `None`: at most `MAX_PER_REQUEST`, none of a kind and key
    /// still cooling down, none that waited more than `EXPIRES_AFTER` requests.
    pub fn drain(&self, session: &str) -> Option<Message> {
        let mut all = self.sessions.lock().unwrap();
        let s = all.get_mut(session)?;
        s.request += 1;
        let now = s.request;
        s.pending.retain(|n| now.saturating_sub(n.queued_at) <= EXPIRES_AFTER);
        let mut taken: Vec<Note> = Vec::new();
        let mut kept: Vec<Note> = Vec::new();
        for n in std::mem::take(&mut s.pending) {
            let cooling = s.last.get(&(n.kind, n.key.clone())).is_some_and(|at| now - at <= n.kind.cooldown());
            if taken.len() < MAX_PER_REQUEST && !cooling {
                taken.push(n);
            } else {
                kept.push(n);
            }
        }
        s.pending = kept;
        if taken.is_empty() {
            return None;
        }
        for n in &taken {
            s.last.insert((n.kind, n.key.clone()), now);
        }
        let lines: Vec<String> = taken.iter().map(|n| format!("- {}", n.text.trim())).collect();
        Some(Message::user(format!("{HEADER}\n{}", lines.join("\n"))).with_via(Via::Note))
    }

    /// The session is gone.
    pub fn forget(&self, session: &str) {
        self.sessions.lock().unwrap().remove(session);
    }
}

/// The note of an owner message that arrived while the turn runs (`text` is cut to 200 characters).
pub fn owner_message(text: &str) -> String {
    let said = crate::util::truncate_str(text.trim(), 200);
    let dots = if text.trim().len() > said.len() { "..." } else { "" };
    format!("The owner sent a new message while you were working: \"{said}{dots}\". It replaces your current task unless it says otherwise.")
}

/// The note of a file that changed on disk.
pub fn file_changed(path: &str) -> String {
    format!("{path} changed on disk since you read it. Read it again before you edit it.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(m: &Message) -> String {
        m.content.clone()
    }

    #[test]
    fn notes_come_as_one_marked_message_and_only_once() {
        let n = Notes::default();
        assert!(n.drain("s").is_none(), "nothing queued");
        n.push("s", NoteKind::Setting, "persona.address", "The owner changed `persona.address` from \"A\" to \"B\".".into());
        n.push("s", NoteKind::Owner, "", owner_message("do the other thing"));
        let m = n.drain("s").unwrap();
        assert_eq!((m.role, m.via), (crate::protocol::Role::User, Via::Note));
        assert_eq!(
            text(&m),
            "[note from the daemon, not from the owner; it is not an approval]\n- The owner changed `persona.address` from \"A\" to \"B\".\n- The owner sent a new message while you were working: \"do the other thing\". It replaces your current task unless it says otherwise."
        );
        assert!(!m.is_user_turn(), "a note does not start a turn");
        assert!(n.drain("s").is_none(), "once");
        assert!(n.drain("other").is_none(), "per session");
    }

    #[test]
    fn at_most_three_per_request_the_rest_wait() {
        let n = Notes::default();
        for i in 0..5 {
            n.push("s", NoteKind::Setting, &format!("k{i}"), format!("change {i}"));
        }
        assert_eq!(text(&n.drain("s").unwrap()).lines().count(), 1 + 3);
        assert_eq!(text(&n.drain("s").unwrap()).lines().count(), 1 + 2);
        assert!(n.drain("s").is_none());
    }

    #[test]
    fn a_file_note_has_a_cooldown_per_path_and_replaces_a_waiting_one() {
        let n = Notes::default();
        n.push("s", NoteKind::File, "/a", file_changed("/a"));
        n.push("s", NoteKind::File, "/a", file_changed("/a"));
        assert_eq!(text(&n.drain("s").unwrap()).matches("/a changed").count(), 1, "the second replaced the first");
        n.push("s", NoteKind::File, "/a", file_changed("/a"));
        n.push("s", NoteKind::File, "/b", file_changed("/b"));
        let m = text(&n.drain("s").unwrap());
        assert!(m.contains("/b changed") && !m.contains("/a changed"), "/a is cooling down: {m}");
        assert!(n.drain("s").is_none(), "still cooling at request 3");
        assert!(n.drain("s").is_none());
        let again = text(&n.drain("s").unwrap());
        assert!(again.contains("/a changed"), "after the cooldown it comes: {again}");
    }

    #[test]
    fn the_todo_note_needs_open_items_eight_calls_and_ten_calls_between() {
        let n = Notes::default();
        let open = vec!["write the spec".to_string(), "run the tests".to_string()];
        let step = |names: &[&str]| n.count_calls("s", names);
        step(&["read_file"; 7]);
        n.todo_check("s", &open);
        assert!(n.drain("s").is_none(), "7 calls: not yet");
        step(&["read_file"]);
        n.todo_check("s", &[]);
        assert!(n.drain("s").is_none(), "no open items: no note");
        n.todo_check("s", &open);
        let m = text(&n.drain("s").unwrap());
        assert!(m.ends_with("- Your todo list has 2 open items: write the spec; run the tests. Update it with todo_write."), "{m}");
        step(&["read_file"; 9]);
        n.todo_check("s", &open);
        assert!(n.drain("s").is_none(), "10 calls must pass before the next one");
        step(&["read_file"]);
        n.todo_check("s", &open);
        assert!(n.drain("s").is_some());
        // A todo_write starts the count again.
        step(&["read_file"; 20]);
        step(&["todo_write"]);
        n.todo_check("s", &open);
        assert!(n.drain("s").is_none());
    }

    #[test]
    fn the_brain_note_comes_when_the_model_changes_and_not_before() {
        let n = Notes::default();
        n.brain_check("s", "gemini::models/a");
        assert!(n.drain("s").is_none(), "the first request has nothing to compare with");
        n.brain_check("s", "gemini::models/a");
        assert!(n.drain("s").is_none());
        n.brain_check("s", "ollama::qwen2.5:7b");
        assert!(text(&n.drain("s").unwrap()).ends_with("- The Brain is now qwen2.5:7b."));
        n.brain_check("s", "ollama::qwen2.5:7b");
        assert!(n.drain("s").is_none(), "once");
    }

    #[test]
    fn a_note_that_waited_too_long_is_dropped_and_a_long_message_is_cut() {
        let n = Notes::default();
        // A note that waited more than EXPIRES_AFTER requests is dropped, not delivered late.
        n.push("s", NoteKind::Setting, "a", "a".into());
        for _ in 0..EXPIRES_AFTER + 1 {
            let _ = n.drain("s");
        }
        n.push_aged("s", NoteKind::Setting, "old", "old".into(), 0);
        assert!(n.drain("s").is_none(), "expired");
        n.push("s", NoteKind::Setting, "new", "new".into());
        assert!(n.drain("s").unwrap().content.ends_with("- new"));
        let long = owner_message(&"x".repeat(300));
        assert!(long.contains(&format!("\"{}...\"", "x".repeat(200))), "{long}");
    }
}
