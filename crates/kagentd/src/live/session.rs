//! The small pieces of one live session: its status line, storing its transcripts, the one-socket-per-session
//! guard and the deadline on a write to Gemini.

use std::sync::Arc;
use std::time::Duration;

use futures::SinkExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as UpMsg;

use kagent_core::protocol::{AgentEvent, Message, Via};

use crate::AppState;

pub(super) fn status(out: &mpsc::UnboundedSender<Value>, st: &AppState, session: &str, s: &str, detail: &str) {
    let _ = out.send(json!({"type": "status", "status": s, "detail": detail}));
    let _ = st.events.send(AgentEvent::LiveStatus { session_id: session.into(), status: s.into(), detail: detail.into() });
}

pub(super) async fn flush_transcripts(st: &AppState, session: &str, user: &mut String, assistant: &mut String) {
    let mut msgs = Vec::new();
    if !user.trim().is_empty() {
        msgs.push(Message::user(user.trim().to_string()).with_via(Via::Voice));
    }
    if !assistant.trim().is_empty() {
        msgs.push(Message::assistant(assistant.trim().to_string(), vec![]).with_via(Via::Voice));
    }
    user.clear();
    assistant.clear();
    if !msgs.is_empty() {
        if let Err(e) = st.agent.append_external(session, msgs).await {
            tracing::warn!("could not store voice transcript: {e:#}");
        }
    }
}

pub(super) struct BusyGuard(pub(super) Arc<AppState>, pub(super) String);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.live_sessions.lock().unwrap().remove(&self.1);
    }
}

/// Send to Gemini with a deadline: a stuck write must not freeze the audio loop.
pub(super) async fn send_up<S>(up_tx: &mut S, msg: UpMsg) -> bool
where
    S: futures::Sink<UpMsg> + Unpin,
{
    matches!(tokio::time::timeout(Duration::from_secs(5), up_tx.send(msg)).await, Ok(Ok(())))
}
