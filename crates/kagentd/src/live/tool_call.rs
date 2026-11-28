//! One tool call of the live model (voice.md sections 3 and 12): declared tools only,
//! spoken confirmation before anything the permission engine would ask about, then the
//! same agent path as a text turn.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use kagent_core::protocol::{Message, ToolCall, Via};

use super::approve::{self, Answer};
use super::confirm::{Repeats, Step, VoiceConfirm};
use super::{brain, noise, protocol};
use crate::screen::live as look;
use crate::AppState;

/// A tool call of the live model, off the audio loop: an approval can take a minute.
/// `own` is the call's token (registered under `id` by the audio loop): the server can cancel this one call.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_tool(st: Arc<AppState>, session: String, c: protocol::FunctionCall, id: String, own: CancellationToken, out: mpsc::UnboundedSender<Value>, tool_tx: mpsc::UnboundedSender<Value>, cancel: CancellationToken, calls: Arc<brain::Calls>, confirm: Arc<VoiceConfirm>, repeats: Arc<Repeats>, heard: String) {
    tokio::spawn(async move {
        let _ = out.send(json!({"type": "tool", "callId": id, "name": c.name, "state": "started"}));
        // Enforced, not only hidden: a tool not declared to this live model does not run.
        if !calls.declared(&c.name) {
            calls.finish(&id);
            let _ = out.send(json!({"type": "tool", "callId": id, "name": c.name, "state": "denied"}));
            let output = format!("tool '{}' is not available in this voice session", c.name);
            let _ = tool_tx.send(protocol::tool_response(&[(&c.id, &c.name, json!({"status": "denied", "output": output}))]));
            return;
        }
        let call = ToolCall { id, name: c.name.clone(), input: c.args.clone() };
        // Spoken confirmation first (voice.md section 12): a call the engine would ask
        // about, and every `always_ask` call (typing into a terminal, changing a
        // setting), runs only after the owner's own "yes" / "confirm"; its card, if it
        // has one, is then answered by that voice, not by a click.
        st.agent.prime_call(&call).await;
        // "Look at my screen" (vision.md section 3): the frame goes to this session, not to a vision model.
        let is_screen = c.name == "screen";
        let _frame_goes_here = is_screen.then(|| look::register(&call.id, tool_tx.clone()));
        if let Some((assessment, ask)) = st.agent.voice_gate(&session, &call) {
            let refuse = |state: &str, response: Value| {
                calls.finish(&call.id);
                let _ = out.send(json!({"type": "tool", "callId": call.id, "name": c.name, "state": state}));
                let _ = tool_tx.send(protocol::tool_response(&[(&c.id, &c.name, response)]));
            };
            let noise_question = if is_screen { None } else { noise::question(&heard, &assessment) };
            if is_screen {
                // Refused before it is asked: a locked screen, and an approval card of this session that waits.
                if let Some(why) = look::refusal(look::is_locked().await, look::open_cards(st.agent.broker().pending().iter().map(|a| a.session_id.as_str()), &session)) {
                    return refuse("denied", json!({"status": "error", "output": why}));
                }
                // Asked out loud once per voice session, whatever the permission mode: the frame leaves the machine.
                let say = look::question(look::target_of(&c.args));
                match confirm.step_once(&c.name, &c.args, &assessment, &say, Instant::now()) {
                    Step::Run => answer_card_by_voice(st.clone(), session.clone(), call.id.clone(), out.clone()),
                    Step::Ask(response) => return refuse("awaiting voice confirmation", response),
                    Step::Refused(response) => return refuse("denied", response),
                }
            } else if ask || noise_question.is_some() {
                // A call that follows only a noise fragment is asked out loud in every mode (agent-loop.md A.2).
                match confirm.step_saying(&c.name, &c.args, &assessment, Instant::now(), noise_question.as_deref()) {
                    Step::Run if ask => answer_card_by_voice(st.clone(), session.clone(), call.id.clone(), out.clone()),
                    Step::Run => {}
                    Step::Ask(response) => return refuse("awaiting voice confirmation", response),
                    Step::Refused(response) => return refuse("denied", response),
                }
            }
            // A live model that repeats one changing action is stopped after the first.
            // A second look is not a repeat of an action: the screen changes.
            if !is_screen && !assessment.read_only && !repeats.first(&c.name, &c.args, Instant::now()) {
                let output = "not executed: you ran exactly this a moment ago and it is done. Do not repeat it; tell the owner the result, or do something else.";
                return refuse("duplicate", json!({"status": "duplicate", "output": output}));
            }
        }
        let privilege = st.settings.read().unwrap().privilege_method();
        let (msg, status) = st.agent.execute_call(&session, "live", &call, &own, privilege).await;
        calls.finish(&call.id);
        // Keep the voice tool call in the session so the transcript shows what was done.
        let record = vec![Message::assistant(String::new(), vec![call.clone()]).with_via(Via::Voice), msg.clone()];
        if let Err(e) = st.agent.append_external(&session, record).await {
            tracing::warn!("could not store voice tool call: {e:#}");
        }
        let state = serde_json::to_value(status).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let _ = out.send(json!({"type": "tool", "callId": call.id, "name": c.name, "state": state}));
        // A call the server cancelled has nobody waiting for its result any more.
        if own.is_cancelled() && !cancel.is_cancelled() {
            return;
        }
        // Keep the spoken context small: the model only needs the gist.
        let result = json!({"status": state, "output": kagent_core::util::truncate_str(&msg.content, 6_000)});
        let _ = tool_tx.send(protocol::tool_response(&[(&c.id, &c.name, result)]));
    });
}

/// The confirmed call raises its card as every call does; the owner already said yes out
/// loud, so the card is answered by that voice as soon as it appears (at most 5 s).
fn answer_card_by_voice(st: Arc<AppState>, session: String, call_id: String, out: mpsc::UnboundedSender<Value>) {
    tokio::spawn(async move {
        for _ in 0..50 {
            if let Some(card) = approve::pending(&st, &session).into_iter().find(|r| r.call_id == call_id) {
                approve::resolve(&st, &card, Answer::Yes, "confirmed by voice before the call", &out);
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
}
