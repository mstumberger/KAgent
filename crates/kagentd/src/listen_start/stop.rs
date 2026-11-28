//! "Stop listening" said in the session started at login (voice.md section 14.4). The model is not asked: the phrase ends the
//! session in the daemon, like the stop phrases of the screen watch (`screen_watch::say`). Only that session listens for it, so
//! the words in an ordinary conversation are never a command.

const PHRASES: [&str; 6] = ["stop listening", "stop the listening", "nehaj poslusati", "nehaj poslu\u{161}ati", "prenehaj poslusati", "prenehaj poslu\u{161}ati"];
const NEGATIONS: [&str; 7] = ["don't", "dont", "do not", "never", "not ", "ne ", "nikoli"];
/// How much of what was heard is kept: a phrase can arrive in two pieces.
pub const KEEP: usize = 48;

/// Whether the owner said a stop phrase at the end of `heard` (and did not negate it).
pub fn is_stop_listening(heard: &str) -> bool {
    let text = heard.to_lowercase().replace(['\u{2019}', '\u{2018}'], "'");
    PHRASES.iter().any(|p| {
        text.rfind(p).is_some_and(|at| {
            let before = &text[..at];
            let near = &before[before.len().saturating_sub(14)..];
            !NEGATIONS.iter().any(|n| near.contains(n))
        })
    })
}

/// Add a piece of the owner's words to `tail` (the last `KEEP` characters) and say whether it now ends with a stop phrase.
/// The tail is cleared when it does.
pub fn hear(tail: &mut String, piece: &str) -> bool {
    tail.push(' ');
    tail.push_str(piece);
    let keep_from = tail.char_indices().rev().nth(KEEP).map(|(i, _)| i).unwrap_or(0);
    tail.drain(..keep_from);
    let stop = is_stop_listening(tail);
    if stop {
        tail.clear();
    }
    stop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrases_in_english_and_slovenian_and_the_negations_that_are_not() {
        for yes in ["stop listening", "Stop listening!", "ok, please stop listening now", "STOP LISTENING", "nehaj poslu\u{161}ati", "nehaj poslusati", "hey, prenehaj poslu\u{161}ati"] {
            assert!(is_stop_listening(yes), "{yes}");
        }
        for no in ["don't stop listening", "do not stop listening", "never stop listening", "I am listening", "stop", "stop the music", "keep listening", "ne nehaj poslusati", "stop watching"] {
            assert!(!is_stop_listening(no), "{no}");
        }
    }

    #[test]
    fn a_phrase_in_two_pieces_is_heard_and_the_tail_stays_short() {
        let mut tail = String::new();
        assert!(!hear(&mut tail, "thanks, that is all"));
        assert!(!hear(&mut tail, "stop"));
        assert!(hear(&mut tail, "listening"), "the two pieces make the phrase");
        assert!(tail.is_empty(), "heard once: cleared");
        assert!(!hear(&mut tail, "and now something else"));
        for _ in 0..50 {
            hear(&mut tail, "a long stretch of ordinary words about the weather and the train");
        }
        assert!(tail.chars().count() <= KEEP + 2, "{}", tail.len());
        assert!(!hear(&mut tail, "do not"), "a negation before the phrase, in another piece");
        assert!(!hear(&mut tail, "stop listening"));
    }
}
