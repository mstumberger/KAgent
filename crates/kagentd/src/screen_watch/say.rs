//! The voice side of a watch (screen-watch.md sections 3 and 4): the open live sessions that a hit is
//! spoken into as a `[SAY]` turn, and the stop phrases of the owner ("stop watching").

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use tokio::sync::mpsc;

use super::ScreenWatch;

/// The open live sessions: the channel to their Gemini connection, by session id. `live/mod.rs` registers
/// one for as long as the session is open.
static OPEN: OnceLock<Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>> = OnceLock::new();

fn open() -> &'static Mutex<HashMap<String, mpsc::UnboundedSender<Value>>> {
    OPEN.get_or_init(Default::default)
}

/// The registration of a live session; dropped when it ends.
pub struct LiveOpen(String);

impl Drop for LiveOpen {
    fn drop(&mut self) {
        open().lock().unwrap().remove(&self.0);
    }
}

pub fn register(session: &str, sink: mpsc::UnboundedSender<Value>) -> LiveOpen {
    open().lock().unwrap().insert(session.to_string(), sink);
    LiveOpen(session.to_string())
}

/// Speak `text` as a `[SAY]` turn: into the live session of `session`, else into any open one (the owner
/// has one voice and one machine). False when no live session is open: the caller notifies instead.
pub fn speak(session: &str, text: &str) -> bool {
    let frame = crate::live::protocol::say_frame(text);
    let map = open().lock().unwrap();
    let sink = map.get(session).or_else(|| map.values().next());
    sink.is_some_and(|s| s.send(frame).is_ok())
}

const PHRASES: [&str; 9] = ["stop watching", "stop the watch", "stop the screen watch", "stop screen watch", "cancel the watch", "nehaj gledati", "nehaj opazovati", "ustavi opazovanje", "prenehaj opazovati"];
const NEGATIONS: [&str; 7] = ["don't", "dont", "do not", "never", "not ", "ne ", "nikoli"];
/// How much of what was heard is kept: a phrase can arrive in two pieces.
const KEEP: usize = 48;

/// Whether the owner said a stop phrase at the end of `heard` (and did not negate it).
pub fn is_stop_phrase(heard: &str) -> bool {
    let text = heard.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    PHRASES.iter().any(|p| {
        text.rfind(p).is_some_and(|at| {
            let before = &text[..at];
            let near = &before[before.len().saturating_sub(14)..];
            !NEGATIONS.iter().any(|n| near.contains(n))
        })
    })
}

impl ScreenWatch {
    /// The owner's words from a live session, piece by piece. A stop phrase while a watch runs ends it
    /// (`stopped`); true when it did. Nothing is kept while no watch runs.
    pub fn hear(&self, text: &str) -> bool {
        let mut heard = self.heard.lock().unwrap();
        if self.current().is_none() {
            heard.clear();
            return false;
        }
        heard.push(' ');
        heard.push_str(text);
        let keep_from = heard.char_indices().rev().nth(KEEP).map(|(i, _)| i).unwrap_or(0);
        heard.drain(..keep_from);
        if !is_stop_phrase(&heard) {
            return false;
        }
        heard.clear();
        drop(heard);
        self.stop().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_phrases_in_english_and_slovenian_and_the_negations_that_are_not() {
        for yes in ["stop watching", "Stop watching!", "ok, please stop watching now", "STOP THE SCREEN WATCH", "nehaj gledati", "ustavi opazovanje", "hey stop watching"] {
            assert!(is_stop_phrase(yes), "{yes}");
        }
        for no in ["don't stop watching", "do not stop watching", "never stop watching it", "I will keep watching", "stop", "stop the music", "keep watching", "ne ustavi opazovanje"] {
            assert!(!is_stop_phrase(no), "{no}");
        }
        assert!(is_stop_phrase("I said don't, but now stop watching"));
    }

    #[test]
    fn a_live_session_that_is_open_is_spoken_into_and_one_that_closed_is_not() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let guard = register("sess-say-1", tx);
        assert!(speak("sess-say-1", "Screen watch: done."));
        let frame = rx.try_recv().unwrap();
        assert_eq!(frame["clientContent"]["turns"][0]["parts"][0]["text"], "[SAY] Screen watch: done.");
        assert_eq!(frame["clientContent"]["turnComplete"], true);
        // Another session id still reaches the one that is open.
        assert!(speak("some-other-session", "x"));
        drop(guard);
        assert!(!speak("sess-say-1", "y"), "nobody listens now");
    }
}
