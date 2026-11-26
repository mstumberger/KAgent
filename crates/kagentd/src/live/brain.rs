//! The brain of a voice session (voice.md sections 1, 2 and 6): who thinks, the `think`
//! call that hands the user's words to the chat model, and what becomes of the calls
//! the live model has running when the server cancels them.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{SendOutcome, TurnOpts, TurnResult};
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{TurnStatus, Via};
use kagent_core::tool::ToolSpec;
use kagent_core::util;

use super::approve::{self, Answer, Card, VoiceCards};
use super::protocol::{self, FunctionCall};
use crate::sections::{VoiceBrain, VoiceSection};
use crate::AppState;

/// A brain turn longer than this is cancelled (voice.md section 2).
const BRAIN_LIMIT: Duration = Duration::from_secs(300);
/// How long after the user's voice answered a card the same words still count as that
/// answer, not as a request: the `think` call for them follows within a few seconds.
const ANSWER_MEMORY: Duration = Duration::from_secs(8);
const TOO_LONG: &str = "that took too long";
/// The reason of a card denied because the server cancelled its call.
pub const INTERRUPTED: &str = "cancelled: the user interrupted";

/// The statuses of a `think` result (tools.md section 24).
pub const OK: &str = "ok";
pub const FAILED: &str = "failed";
pub const CANCELLED: &str = "cancelled";
pub const HANDLED: &str = "handled";

/// The status detail of a session that wanted `routed` and runs `live` (voice.md section 1).
pub const NO_CHAT_MODEL: &str = "no chat model: the live model thinks";

/// What a `think` text is (voice.md section 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Think {
    /// It answers a card: the card is resolved and nothing reaches the brain.
    Handled,
    /// Only a stop phrase: the brain turn is cancelled.
    Stop,
    /// Words for the brain.
    Brain,
}

/// `pending` are the cards that wait in the session, in this order. `answered` are the
/// cards the user's voice answered a moment ago: their words, when that is all the
/// text is, are that answer again and not a request.
pub fn classify_think(text: &str, pending: &[Card], answered: &[Card]) -> Think {
    if pending.iter().any(|card| card.answers(text)) {
        return Think::Handled;
    }
    if approve::is_stop(text) {
        return Think::Stop;
    }
    if approve::words(text).len() <= approve::ANSWER_WORDS && answered.iter().any(|card| card.answers(text)) {
        return Think::Handled;
    }
    Think::Brain
}

/// What a brain turn becomes: the statuses of tools.md section 24.
pub fn outcome(result: &TurnResult) -> (String, &'static str) {
    match result.status {
        TurnStatus::Completed => {
            let reply = util::speakable(&result.text, util::SPEAK_CHARS);
            // A turn that ends with no text (a local server drops a tool call it cannot
            // parse) would leave the user in silence.
            if reply.is_empty() { (sorry("the brain gave no answer"), FAILED) } else { (reply, OK) }
        }
        TurnStatus::Cancelled => (String::new(), CANCELLED),
        TurnStatus::Failed => (sorry(result.error.as_deref().filter(|e| !e.is_empty()).unwrap_or("the turn failed")), FAILED),
    }
}

/// "Sorry, I could not think: <why>".
pub fn sorry(why: &str) -> String {
    format!("Sorry, I could not think: {why}")
}

fn chat_model_usable(st: &AppState) -> bool {
    let chat = st.settings.read().unwrap().chat_model.clone();
    // The same test the turn uses (`usable`: enabled, chat role, provider on), and not live.
    st.providers.usable(&chat).is_some_and(|r| !r.is_live())
}

/// The brain of a session, read when it connects (voice.md section 1): the section,
/// the mode that runs, and the status detail when that is not the configured one.
/// `routed` without a usable chat model runs `live`.
pub fn mode(st: &AppState) -> (VoiceSection, VoiceBrain, &'static str) {
    let section = st.settings.read().unwrap().voice_config();
    match section.brain {
        VoiceBrain::Routed if !chat_model_usable(st) => (section, VoiceBrain::Live, NO_CHAT_MODEL),
        brain => (section, brain, ""),
    }
}

/// The tools declared to the live model (voice.md sections 1 and 3): `think` alone in
/// `routed`; `think` and the tools of `voice.toolGroups` in `auto`; the tools alone in
/// `live`. In registry order. A group that is off for chat gives none: the agent
/// refuses those tools at execution, so they are not offered either.
pub fn tools(st: &AppState, mode: VoiceBrain, groups: &[String]) -> Vec<ToolSpec> {
    let mut out = Vec::new();
    if mode != VoiceBrain::Live {
        out.push(protocol::think_spec());
    }
    if mode != VoiceBrain::Routed {
        out.extend(st.agent.advertised_specs().into_iter().filter(|t| groups.iter().any(|g| g == kagent_core::tools::group_of(&t.name))));
    }
    out
}

/// A `think` call that runs. The server may cancel it (`by_server`), which does not
/// cancel the brain turn; a later `think` call queued behind the same turn may take
/// its reply over (`taken_over`).
#[derive(Debug, Default)]
pub struct ThinkState {
    pub by_server: AtomicBool,
    pub taken_over: AtomicBool,
}

/// A call of the live model that still runs.
#[derive(Clone)]
pub enum Running {
    /// A tool: its own token, a child of the session's.
    Tool(CancellationToken),
    Think(Arc<ThinkState>),
}

/// What the audio loop and the spawned calls of one live session share.
#[derive(Default)]
pub struct Calls {
    /// The tools declared to the live model; a call to another name is refused, not only hidden.
    declared: Mutex<HashSet<String>>,
    /// Calls that still run, by the id the server gave them.
    running: Mutex<HashMap<String, Running>>,
    /// A brain turn ran in the turn of the live model that is open now.
    think_used: AtomicBool,
}

impl Calls {
    pub fn declare(&self, tools: &[ToolSpec]) {
        *self.declared.lock().unwrap() = tools.iter().map(|t| t.name.clone()).collect();
    }

    pub fn declared(&self, name: &str) -> bool {
        self.declared.lock().unwrap().contains(name)
    }

    /// A tool call runs under `id`.
    pub fn start_tool(&self, id: String, token: CancellationToken) {
        self.running.lock().unwrap().insert(id, Running::Tool(token));
    }

    pub fn start_think(&self, id: &str) -> Arc<ThinkState> {
        let state = Arc::new(ThinkState::default());
        self.running.lock().unwrap().insert(id.to_string(), Running::Think(state.clone()));
        state
    }

    pub fn finish(&self, id: &str) {
        self.running.lock().unwrap().remove(id);
    }

    /// Whether a brain turn ran in the open turn of the live model; asking says no the next time.
    pub fn take_think_used(&self) -> bool {
        self.think_used.swap(false, Ordering::Relaxed)
    }

    /// A `think` call was queued behind a running brain turn: it brings that turn's
    /// reply, and the calls that waited for the same turn say nothing.
    pub fn take_over(&self, except: &str) {
        for (id, call) in self.running.lock().unwrap().iter() {
            if let (false, Running::Think(state)) = (id == except, call) {
                state.taken_over.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// `toolCallCancellation` (voice.md sections 2 and 6). A tool call that still runs gets
/// its token cancelled and its waiting card is denied; a call that ended is left
/// alone. A `think` call is only marked: interrupting the voice does not cancel the
/// brain turn.
pub fn cancel_calls(st: &AppState, session: &str, ids: &[String], calls: &Calls) {
    let running = calls.running.lock().unwrap();
    for id in ids {
        match running.get(id) {
            Some(Running::Tool(token)) => {
                let deny = ApprovalDecision::Deny { reason: Some(INTERRUPTED.into()) };
                for card in approve::pending(st, session).iter().filter(|c| &c.call_id == id) {
                    st.agent.broker().resolve(&card.request_id, deny.clone());
                }
                // The card's answer reaches the waiting call first (the broker also
                // watches the token); then the token stops whatever the tool was running.
                let token = token.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    token.cancel();
                });
            }
            Some(Running::Think(state)) => state.by_server.store(true, Ordering::Relaxed),
            None => {}
        }
    }
}

/// A `think` call (voice.md section 2), off the audio loop: a brain turn takes seconds
/// to minutes. Its reply is the `toolResponse` of the call; when the server cancelled
/// the call, a text turn instead (`[SAY]`). `state` is the call's entry of `calls`,
/// made by the audio loop before this task starts, so a cancellation that follows the
/// call at once finds it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_think(st: Arc<AppState>, session: String, c: FunctionCall, id: String, state: Arc<ThinkState>, out: mpsc::UnboundedSender<Value>, tool_tx: mpsc::UnboundedSender<Value>, voice_cards: Arc<VoiceCards>, calls: Arc<Calls>) {
    tokio::spawn(async move {
        let _ = out.send(json!({"type": "tool", "callId": id, "name": "think", "state": "started"}));
        let (reply, status) = think(&st, &session, &id, &c, &out, &voice_cards, &calls).await;
        calls.finish(&id);
        let shown = match status {
            FAILED => "error",
            OK | HANDLED => "ok",
            _ => "cancelled",
        };
        let _ = out.send(json!({"type": "tool", "callId": id, "name": "think", "state": shown}));
        let said = state.by_server.load(Ordering::Relaxed);
        if state.taken_over.load(Ordering::Relaxed) {
            // A later call of the same turn brings the reply.
            if !said && status != CANCELLED {
                let _ = tool_tx.send(protocol::tool_response(&[(&c.id, "think", json!({"reply": "", "status": CANCELLED}))]));
            }
        } else if said {
            // Nobody waits for this result any more: the reply goes out as a text turn.
            if status != CANCELLED {
                let _ = tool_tx.send(protocol::say_frame(&reply));
            }
        } else {
            let _ = tool_tx.send(protocol::tool_response(&[(&c.id, "think", json!({"reply": reply, "status": status}))]));
        }
    });
}

/// The steps of voice.md section 2, in order.
#[allow(clippy::too_many_arguments)]
async fn think(st: &AppState, session: &str, id: &str, c: &FunctionCall, out: &mpsc::UnboundedSender<Value>, voice_cards: &VoiceCards, calls: &Calls) -> (String, &'static str) {
    let text = c.args.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if text.is_empty() {
        return (String::new(), CANCELLED);
    }
    let pending = approve::pending(st, session);
    let cards: Vec<Card> = pending.iter().map(Card::of).collect();
    let answered = voice_cards.answered(ANSWER_MEMORY);
    match classify_think(&text, &cards, &answered) {
        Think::Handled => {
            // The words answer a card: resolved as a click on it would. A brain turn that
            // waits for that card goes on; its reply comes through its own call.
            if let Some(card) = pending.iter().find(|p| Card::of(p).answers(&text)) {
                let answer = approve::classify_for(&text, card.privileged || card.destructive, &card.summary).unwrap_or(Answer::No);
                approve::resolve(st, card, answer, &text, out);
            } else {
                voice_cards.forget_answer(&text);
            }
            (String::new(), HANDLED)
        }
        Think::Stop => {
            // Only a stop phrase: the turn is cancelled; its cards are denied by the agent.
            st.agent.cancel(session).await;
            (String::new(), CANCELLED)
        }
        Think::Brain => {
            calls.think_used.store(true, Ordering::Relaxed);
            // While a brain turn runs the text is queued behind it, so it steers the first.
            match st.agent.send_with(session, text, Via::Voice, TurnOpts::default()).await {
                Ok(SendOutcome::Started { .. }) => {}
                Ok(SendOutcome::Queued) => calls.take_over(id),
                Err(e) => return (sorry(&format!("{e:#}")), FAILED),
            }
            match tokio::time::timeout(BRAIN_LIMIT, st.agent.wait_idle(session)).await {
                Ok(result) => outcome(&result),
                Err(_) => {
                    st.agent.cancel(session).await;
                    let _ = tokio::time::timeout(Duration::from_secs(10), st.agent.wait_idle(session)).await;
                    (sorry(TOO_LONG), FAILED)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::protocol::Usage;

    fn card(summary: &str, strong: bool) -> Card {
        Card { strong, summary: summary.into() }
    }

    /// voice.md section 10: the `classify_think` table.
    #[test]
    fn classify_think_table() {
        let cards = [card("reboot now", true), card("send the message", false)];
        let empty: [Card; 0] = [];
        for (text, pending, think) in [
            ("", &empty[..], Think::Brain),
            ("stop", &empty[..], Think::Stop),
            ("Stop that!", &empty[..], Think::Stop),
            ("ustavi", &empty[..], Think::Stop),
            ("stop the music", &empty[..], Think::Brain),
            ("what is my CPU temperature", &empty[..], Think::Brain),
            ("stop", &cards[..], Think::Handled),
            ("Yes, reboot.", &cards[..], Think::Handled),
            ("no", &cards[..], Think::Handled),
            ("stop the music", &cards[..], Think::Handled),
            ("what is my CPU temperature", &cards[..], Think::Brain),
        ] {
            assert_eq!(classify_think(text, pending, &empty), think, "{text:?} with {} waiting cards", pending.len());
        }
        // The same words that just answered a card from the transcript are that answer once...
        assert_eq!(classify_think("yes", &empty, &[card("send the message", false)]), Think::Handled);
        // ...but words that ask for more go to the brain, and so does "stop".
        assert_eq!(classify_think("yes, and then open Firefox", &empty, &[card("send the message", false)]), Think::Brain);
        assert_eq!(classify_think("stop", &empty, &[card("send the message", false)]), Think::Stop);
        assert_eq!(classify_think("what is the weather", &empty, &[card("send the message", false)]), Think::Brain);
    }

    #[test]
    fn a_brain_turn_becomes_a_speakable_reply() {
        let result = |status, text: &str, error: Option<&str>| TurnResult { status, error: error.map(String::from), text: text.into(), usage: Usage::default() };
        assert_eq!(outcome(&result(TurnStatus::Completed, "**It is 45 C** on `coretemp`.", None)), ("It is 45 C on coretemp.".to_string(), OK));
        assert_eq!(outcome(&result(TurnStatus::Cancelled, "part", None)), (String::new(), CANCELLED));
        assert_eq!(outcome(&result(TurnStatus::Failed, "", Some("provider error 401"))), ("Sorry, I could not think: provider error 401".to_string(), FAILED));
        assert_eq!(outcome(&result(TurnStatus::Completed, "", None)), ("Sorry, I could not think: the brain gave no answer".to_string(), FAILED), "a turn with no text is not silence");
        assert_eq!(outcome(&result(TurnStatus::Failed, "", None)).1, FAILED);
    }

    #[test]
    fn a_queued_call_takes_the_turn_over() {
        let calls = Calls::default();
        let (a, b) = (calls.start_think("a"), calls.start_think("b"));
        calls.start_tool("t".into(), CancellationToken::new());
        assert!(!a.taken_over.load(Ordering::Relaxed));
        calls.take_over("b");
        assert!(a.taken_over.load(Ordering::Relaxed) && !b.taken_over.load(Ordering::Relaxed), "the calls that waited for the turn give its reply to the queued one");
        assert!(!a.by_server.load(Ordering::Relaxed), "taking over is not a cancellation by the server");
        calls.finish("a");
        assert!(!calls.declared("think"));
        calls.declare(&[protocol::think_spec()]);
        assert!(calls.declared("think") && !calls.declared("system_info"));
    }
}
