//! "Look at my screen" in live voice (vision.md section 3). A `screen` call of the live
//! model does not ask a vision model: it captures, and the frame goes to the live session
//! itself as a `realtimeInput` video frame. The pieces are here; `Screen::call` and
//! `live/tool_call.rs` use them.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use kagent_core::tool::ToolSpec;
use serde_json::{json, Value};
use tokio::sync::mpsc;

/// The longest side of a frame sent to the live model (pixels).
pub const FRAME_SIDE: u32 = 1024;
/// The answer of a call that sent its frame.
pub const SEEN: &str = "the screenshot is in your view now";
/// The answer when the session is locked.
pub const LOCKED: &str = "the screen is locked";

/// The `screen` the live model is declared: the same name as the Brain's tool, no `question`
/// (the live model looks itself), no `window`.
pub fn live_spec() -> ToolSpec {
    ToolSpec {
        name: "screen".into(),
        description: "Look at the user's screen: takes a screenshot and puts it in your view, then answer from what you see. target: window = the active window (default), monitor = the current monitor, desktop = everything; use monitor or desktop only when asked. Call it whenever the user asks what is on the screen, what a dialog or an error says, or 'can you see it'. The image leaves the machine, so the owner is asked out loud once per voice session.".into(),
        input_schema: json!({"type": "object", "properties": {"target": {"type": "string", "enum": ["window", "monitor", "desktop"]}}}),
    }
}

/// The target of a call: the active window unless the owner asked for more.
pub fn target_of(input: &Value) -> &'static str {
    match input.get("target").and_then(Value::as_str) {
        Some("monitor") => "monitor",
        Some("desktop") => "desktop",
        _ => "window",
    }
}

/// What the owner is asked out loud before the first look of a voice session.
pub fn question(target: &str) -> String {
    let what = match target {
        "monitor" => "your monitor",
        "desktop" => "your whole desktop",
        _ => "your active window",
    };
    format!("I want to look at {what}. Say yes to go ahead, or no.")
}

/// Why a look is refused, or `None`. KAgent never captures its own approval cards (a card
/// of the same session that waits for an answer is on the screen) and never a locked screen.
pub fn refusal(locked: bool, open_cards: usize) -> Option<String> {
    if locked {
        return Some(LOCKED.into());
    }
    (open_cards > 0).then(|| "not captured: an approval card is open on the screen, and KAgent never captures its own approval cards. Answer the card (yes or no) first, then ask again.".into())
}

/// How many of the waiting cards (the session id of each) belong to `session`: another conversation's card does not stop a look.
pub fn open_cards<'a>(waiting: impl IntoIterator<Item = &'a str>, session: &str) -> usize {
    waiting.into_iter().filter(|s| *s == session).count()
}

/// The session is locked: logind's `LockedHint` of this session (KDE and GNOME both set it).
pub async fn is_locked() -> bool {
    // The tests say it themselves (the screen is locked while that file exists): they never look at the owner's session.
    if let Some(file) = crate::seams::seam("KAGENT_FAKE_LOCKED") {
        return std::path::Path::new(&file).exists();
    }
    let session = std::env::var("XDG_SESSION_ID").unwrap_or_else(|_| "self".into());
    let out = tokio::process::Command::new("loginctl").args(["show-session", &session, "-p", "LockedHint", "--value"]).output().await;
    matches!(out, Ok(o) if String::from_utf8_lossy(&o.stdout).trim() == "yes")
}

/// A JPEG of the longest side `FRAME_SIDE` made of a screenshot.
pub fn frame_jpeg(png: &[u8]) -> Result<Vec<u8>, String> {
    kagent_core::session::images::to_frame(png, FRAME_SIDE).map(|e| e.bytes).map_err(|e| e.to_string())
}

/// The `realtimeInput` video frame of the live protocol.
pub fn video_frame(jpeg: &[u8]) -> Value {
    use base64::Engine as _;
    crate::live::protocol::video_input(&base64::engine::general_purpose::STANDARD.encode(jpeg))
}

/// Where the frame of a live call goes: the live session's channel to Gemini, by call id. Only
/// `live/tool_call.rs` registers one, for the call it is about to run, so a call that
/// is not the live model's (the Brain's own `screen`) has none and keeps the vision path.
static SINKS: OnceLock<Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>> = OnceLock::new();

fn sinks() -> &'static Mutex<HashMap<String, mpsc::UnboundedSender<Value>>> {
    SINKS.get_or_init(Default::default)
}

/// The registration of a call; dropped when the call is over.
pub struct LiveCall(String);

impl Drop for LiveCall {
    fn drop(&mut self) {
        sinks().lock().unwrap().remove(&self.0);
    }
}

pub fn register(call_id: &str, sink: mpsc::UnboundedSender<Value>) -> LiveCall {
    sinks().lock().unwrap().insert(call_id.to_string(), sink);
    LiveCall(call_id.to_string())
}

pub fn sink_of(call_id: &str) -> Option<mpsc::UnboundedSender<Value>> {
    sinks().lock().unwrap().get(call_id).cloned()
}

#[cfg(test)]
mod tests;
