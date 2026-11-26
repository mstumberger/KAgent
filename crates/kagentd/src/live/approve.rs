//! Answering approval cards by voice. Only the user's own speech transcript (Gemini's
//! input transcription) is matched here, never the model's output, so the agent cannot
//! approve its own actions. Root actions need an explicit "approve"/"confirm".
//!
//! `is_stop` tells a stop phrase from a request (voice.md section 2). `VoiceCards` is
//! what one live session remembers of its cards: the speech heard since each appeared,
//! and the cards the user's voice answered a moment ago.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::mpsc;

use kagent_core::permission::{ApprovalDecision, ApprovalRequest};

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
}

pub(super) fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(String::from).collect()
}

const NO: [&str; 12] = ["no", "nope", "don't", "dont", "stop", "cancel", "deny", "wait", "ne", "nikar", "počakaj", "prekliči"];
const YES: [&str; 17] = ["yes", "yeah", "yep", "ok", "okay", "sure", "approve", "approved", "allow", "confirm", "proceed", "ja", "lahko", "potrdi", "dovoli", "seveda", "izvedi"];
const YES_PHRASES: [&str; 6] = ["do it", "go ahead", "send it", "run it", "go on", "daj ga"];
const STRONG: [&str; 6] = ["approve", "approved", "confirm", "potrdi", "odobri", "dovoli"];

/// The user's answer in this utterance, if any. "No" wins over "yes" ("no, don't send it").
/// `privileged`: only explicit approve/confirm words count as yes.
#[cfg(test)]
pub fn classify(text: &str, privileged: bool) -> Option<Answer> {
    classify_for(text, privileged, "")
}

/// Strong confirmation (root or destructive actions): an explicit approve/confirm word,
/// or a yes together with the action's own word from the card ("yes, reboot").
/// `action` = the card summary, e.g. "reboot now".
pub fn classify_for(text: &str, strong: bool, action: &str) -> Option<Answer> {
    let privileged = strong;
    let w = words(text);
    if w.is_empty() {
        return None;
    }
    if w.iter().any(|x| NO.contains(&x.as_str())) {
        return Some(Answer::No);
    }
    let lower = format!(" {} ", w.join(" "));
    let verb = action.split_whitespace().next().unwrap_or("").to_lowercase();
    let said_yes = w.iter().any(|x| YES.contains(&x.as_str())) || YES_PHRASES.iter().any(|p| lower.contains(&format!(" {p} ")));
    let yes = if privileged {
        w.iter().any(|x| STRONG.contains(&x.as_str())) || (said_yes && verb.len() >= 3 && w.iter().any(|x| *x == verb))
    } else {
        w.iter().any(|x| YES.contains(&x.as_str())) || YES_PHRASES.iter().any(|p| lower.contains(&format!(" {p} ")))
    };
    // Long utterances are new requests, not answers ("ok, now write to Miha that...").
    (yes && w.len() <= 8).then_some(Answer::Yes)
}

const STOP: [&str; 5] = ["stop", "cancel", "ustavi", "prekliči", "pozabi"];
const STOP_PHRASES: [&str; 3] = ["stop that", "cancel that", "never mind"];

/// The utterance is only a stop phrase (voice.md section 2): one or two words out of
/// `STOP`, or one of `STOP_PHRASES`. "Stop the music" is a request and goes to the brain.
pub fn is_stop(text: &str) -> bool {
    let w = words(text);
    match w.as_slice() {
        [a] => STOP.contains(&a.as_str()),
        [a, b] => (STOP.contains(&a.as_str()) && STOP.contains(&b.as_str())) || STOP_PHRASES.contains(&format!("{a} {b}").as_str()),
        _ => false,
    }
}

/// One approval card as the voice side sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    /// Root or destructive: only an explicit approve/confirm counts as yes.
    pub strong: bool,
    pub summary: String,
}

impl Card {
    pub fn of(request: &ApprovalRequest) -> Self {
        Self { strong: request.privileged || request.destructive, summary: request.summary.clone() }
    }

    /// Whether `text` answers this card.
    pub fn answers(&self, text: &str) -> bool {
        classify_for(text, self.strong, &self.summary).is_some()
    }
}

/// A `think` text of at most this many words is only the answer to a card its words
/// resolved a moment ago; a longer one asks for something too and goes to the brain.
pub const ANSWER_WORDS: usize = 3;

/// The approval cards of one live session, answered from the user's own speech.
pub struct VoiceCards {
    /// The user's speech since each waiting card appeared.
    heard: Mutex<HashMap<String, String>>,
    /// Cards the voice answered, oldest first, with when.
    answered: Mutex<Vec<(Card, Instant)>>,
}

impl VoiceCards {
    pub fn new() -> Self {
        Self { heard: Mutex::new(HashMap::new()), answered: Mutex::new(Vec::new()) }
    }

    /// The cards of the session that wait now.
    pub fn pending(&self, st: &AppState, session: &str) -> Vec<ApprovalRequest> {
        pending(st, session)
    }

    /// Offer the user's latest words to the waiting cards; the first one they answer is resolved.
    pub fn hear(&self, st: &AppState, session: &str, chunk: &str, out: &mpsc::UnboundedSender<Value>) {
        let pending = self.pending(st, session);
        let mut heard = self.heard.lock().unwrap();
        heard.retain(|id, _| pending.iter().any(|p| &p.request_id == id));
        for p in pending {
            let buf = heard.entry(p.request_id.clone()).or_default();
            buf.push_str(chunk);
            let Some(answer) = classify_for(buf, p.privileged || p.destructive, &p.summary) else { continue };
            if resolve(st, &p, answer, buf, out) {
                self.answered.lock().unwrap().push((Card::of(&p), Instant::now()));
            }
            heard.remove(&p.request_id);
        }
    }

    /// The cards the voice answered within `within`, oldest first.
    pub fn answered(&self, within: Duration) -> Vec<Card> {
        let mut answered = self.answered.lock().unwrap();
        answered.retain(|(_, at)| at.elapsed() <= within);
        answered.iter().map(|(card, _)| card.clone()).collect()
    }

    /// Forget the oldest recently answered card that `text` answers, so the same words
    /// count once.
    pub fn forget_answer(&self, text: &str) {
        let mut answered = self.answered.lock().unwrap();
        if let Some(i) = answered.iter().position(|(card, _)| card.answers(text)) {
            answered.remove(i);
        }
    }
}

/// The cards that wait in a session.
pub fn pending(st: &AppState, session: &str) -> Vec<ApprovalRequest> {
    st.agent.broker().pending().into_iter().filter(|a| a.session_id == session).collect()
}

/// Resolve a waiting card with the user's answer, as a click on it would. False when
/// it was resolved already (by the page, or by these words a moment earlier).
pub(super) fn resolve(st: &AppState, request: &ApprovalRequest, answer: Answer, said: &str, out: &mpsc::UnboundedSender<Value>) -> bool {
    let decision = match answer {
        Answer::Yes => ApprovalDecision::AllowOnce,
        Answer::No => ApprovalDecision::Deny { reason: Some("denied by voice".into()) },
    };
    if !st.agent.broker().resolve(&request.request_id, decision) {
        return false;
    }
    tracing::info!("voice {} '{}' for {}: \"{}\"", if answer == Answer::Yes { "approved" } else { "denied" }, request.tool, request.summary, said.trim());
    let _ = out.send(json!({"type": "tool", "callId": request.call_id, "name": request.tool, "state": if answer == Answer::Yes { "approved by voice" } else { "denied by voice" }}));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers() {
        assert_eq!(classify("Yes.", false), Some(Answer::Yes));
        assert_eq!(classify("go ahead", false), Some(Answer::Yes));
        assert_eq!(classify("Ja, lahko.", false), Some(Answer::Yes));
        assert_eq!(classify("No, don't send it", false), Some(Answer::No));
        assert_eq!(classify("wait", false), Some(Answer::No));
        assert_eq!(classify("prosecco", false), None);
        assert_eq!(classify("ok now write to Miha that I will be late for the meeting today", false), None, "a new request, not an answer");
        assert_eq!(classify("yes", true), None, "root needs an explicit approve");
        assert_eq!(classify("I approve", true), Some(Answer::Yes));
        // Destructive: the transcript that rebooted the PC must not confirm anything.
        assert_eq!(classify_for("Should we reboot?", true, "reboot now"), None);
        assert_eq!(classify_for("Ezel", true, "reboot now"), None);
        assert_eq!(classify_for("b", true, "reboot now"), None);
        assert_eq!(classify_for("yes", true, "reboot now"), None, "a bare yes is not enough");
        assert_eq!(classify_for("Yes, reboot.", true, "reboot now"), Some(Answer::Yes));
        assert_eq!(classify_for("confirm", true, "reboot now"), Some(Answer::Yes));
        assert_eq!(classify_for("no, don't reboot", true, "reboot now"), Some(Answer::No));
    }

    /// voice.md section 10: the `is_stop` table.
    #[test]
    fn stop_phrases() {
        for said in ["stop", "Stop.", "cancel", "ustavi", "prekliči", "pozabi", "stop that", "cancel that", "never mind", "Stop that!", "ustavi, prekliči"] {
            assert!(is_stop(said), "{said} is only a stop phrase");
        }
        for said in ["", "...", "stop the music", "stop it", "cancel the reboot", "can you stop the music", "never", "that", "please stop the printer job", "stoppodal"] {
            assert!(!is_stop(said), "{said} is a request");
        }
        // "Stop" is also a "no" to a waiting card: the card comes first (classify_think).
        let card = Card { strong: false, summary: "send the message".into() };
        assert!(card.answers("stop the music") && card.answers("stop"));
    }
}
