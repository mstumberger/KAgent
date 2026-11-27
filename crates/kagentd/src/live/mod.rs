//! Live voice through the daemon (IMPLEMENTATION.md S10, api.md section 5).
//!
//! Browser <-> `/live` <-> kagentd <-> Gemini BidiGenerateContent. The API key never
//! leaves the daemon; tool calls run through the same agent path as text turns
//! (permissions, approvals, audit); transcripts land in the session; levels feed the
//! visualizers. The browser socket survives upstream rolls (goAway / drops) thanks to
//! session resumption handles. Who thinks is `voice.brain` (voice.md): `brain.rs` has
//! the `think` call, `context.rs` what the live model is told, `instruction.rs` its text.

pub mod approve;
pub mod brain;
mod confirm;
pub mod context;
mod instruction;
pub mod protocol;
pub use think_times::median_ms as think_median_ms;
mod think_times;
mod tool_call;
mod voice_roll;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message as WsMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as UpMsg;
use tokio_util::sync::CancellationToken;

use kagent_core::protocol::{AgentEvent, Message, Via};

use crate::sections::VoiceBrain;
use crate::server::token_eq;
use crate::voice_levels::Who;
use crate::AppState;
use protocol::{ServerEvent, SetupParams};

const MAX_ROLLS: u32 = 30;

pub async fn ws_live(
    State(st): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if !token_eq(params.get("token").map(String::as_str).unwrap_or(""), &st.token) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    let session = params.get("session").cloned().filter(|s| !s.is_empty());
    ws.on_upgrade(move |socket| async move {
        if let Err(e) = run(st, socket, session).await {
            tracing::warn!("live session ended with error: {e:#}");
        }
    })
}

struct Target {
    url: String,
    model: String,
}

fn resolve_target(st: &AppState) -> Result<Target, String> {
    let model_ref = st.settings.read().unwrap().live_model.clone();
    if model_ref.trim().is_empty() {
        return Err("no live model: pick one in Settings -> Voice (a Gemini *-live model)".into());
    }
    let r = st.providers.resolve(&model_ref).map_err(|e| format!("live model {model_ref}: {e}"))?;
    if r.provider.api_key.is_empty() {
        return Err(format!("provider {} has no API key", r.provider.id));
    }
    // Tests point this at a fake server.
    let base = std::env::var("KAGENT_GEMINI_WS").unwrap_or_else(|_| protocol::GEMINI_WS.into());
    let sep = if base.contains('?') { '&' } else { '?' };
    Ok(Target { url: format!("{base}{sep}key={}", r.provider.api_key), model: r.model.id.clone() })
}

fn status(out: &mpsc::UnboundedSender<Value>, st: &AppState, session: &str, s: &str, detail: &str) {
    let _ = out.send(json!({"type": "status", "status": s, "detail": detail}));
    let _ = st.events.send(AgentEvent::LiveStatus { session_id: session.into(), status: s.into(), detail: detail.into() });
}

async fn flush_transcripts(st: &AppState, session: &str, user: &mut String, assistant: &mut String) {
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

struct BusyGuard(Arc<AppState>, String);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.live_sessions.lock().unwrap().remove(&self.1);
    }
}

async fn run(st: Arc<AppState>, socket: WebSocket, session: Option<String>) -> anyhow::Result<()> {
    let (mut b_tx, mut b_rx) = socket.split();
    // Browser writer: everything for the browser goes through one channel.
    let (out, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(v) = out_rx.recv().await {
            if b_tx.send(WsMsg::Text(v.to_string().into())).await.is_err() {
                break;
            }
        }
    });

    let session = match session {
        Some(s) => s,
        None => st.agent.store().create_with("Voice session", kagent_core::session::SessionOrigin::Voice, None).await?.id,
    };
    if !st.live_sessions.lock().unwrap().insert(session.clone()) {
        let _ = out.send(json!({"type": "status", "status": "error", "detail": "busy"}));
        drop(out);
        let _ = writer.await;
        return Ok(());
    }
    let _guard = BusyGuard(st.clone(), session.clone());
    let _ = out.send(json!({"type": "session", "sessionId": session}));

    let (tool_tx, mut tool_rx) = mpsc::unbounded_channel::<Value>();
    let cancel = CancellationToken::new();
    // Who thinks is read when the session connects (voice.md section 1): a change applies to the next session.
    let (section, mode, detail) = brain::mode(&st);
    let routed = mode == VoiceBrain::Routed;
    let instruction = instruction::of(&st, mode);
    let tools = brain::tools(&st, mode, &section.tool_groups);
    let vocabulary = if section.vocabulary { context::vocabulary(&st).await } else { Vec::new() };
    let calls = Arc::new(brain::Calls::default());
    calls.declare(&tools);
    let voice_cards = Arc::new(approve::VoiceCards::new());
    // Calls the engine would ask about are asked out loud first (voice.md section 12).
    let voice_confirm = Arc::new(confirm::VoiceConfirm::default());
    let repeats = Arc::new(confirm::Repeats::default());
    let mut voice = st.settings.read().unwrap().live_voice.clone();
    let mut handle: Option<String> = None;
    let (mut user_buf, mut asst_buf) = (String::new(), String::new());
    let mut rolls = 0u32;
    let mut ever_connected = false;
    // Set when a roll changes the voice: a server that refuses the handle with the new voice gets a fresh connection.
    let mut voice_rolled = false;
    // Media paused when the session went live, resumed when it ends (voice.pauseMedia).
    let media_ctx = kagent_core::tool::ToolContext::standalone(Duration::from_secs(3), 64_000);
    let mut paused: Option<Vec<String>> = None;
    // A turn the brain turn wrote to the session (`routed`, or `auto` with `think`) is not stored twice.
    macro_rules! close_turn {
        () => {
            if routed || calls.take_think_used() {
                user_buf.clear();
                asst_buf.clear();
            } else {
                flush_transcripts(&st, &session, &mut user_buf, &mut asst_buf).await;
            }
        };
    }

    'conn: loop {
        if rolls > MAX_ROLLS {
            status(&out, &st, &session, "error", "too many reconnects");
            break;
        }
        status(&out, &st, &session, if ever_connected { "rolling" } else { "connecting" }, "");
        let target = match resolve_target(&st) {
            Ok(t) => t,
            Err(e) => {
                status(&out, &st, &session, "error", &e);
                break;
            }
        };
        let up = match tokio::time::timeout(Duration::from_secs(10), tokio_tungstenite::connect_async(&target.url)).await {
            Ok(Ok((up, _))) => up,
            other => {
                let why = match other {
                    Ok(Err(e)) => e.to_string(),
                    _ => "timeout".into(),
                };
                tracing::warn!("live upstream connect failed: {}", why.replace(&target.url, "<url>"));
                rolls += 1;
                tokio::time::sleep(Duration::from_millis(800 * rolls as u64)).await;
                continue;
            }
        };
        rolls += 1;
        let (mut up_tx, mut up_rx) = up.split();
        let first = handle.is_none();
        let mut set_up = false;
        let setup = protocol::setup_message(&SetupParams {
            model: &target.model,
            voice: &voice,
            instruction: &instruction,
            tools: &tools,
            resume_handle: handle.as_deref(),
            history: first && !routed,
            vocabulary: &vocabulary,
        });
        if !send_up(&mut up_tx, UpMsg::Text(setup.to_string().into())).await {
            continue;
        }
        // What follows `setupComplete` (voice.md section 4); `routed` sends neither: the brain turn has it.
        let mut opening = if routed { Vec::new() } else { context::opening(&st, &session, first, section.context).await };
        // Watchdog: Google sometimes leaves the socket open but dead (no close, writes pile
        // up unsent). Ping every 10 s; no traffic at all for 25 s = reconnect with resumption.
        let mut ping = tokio::time::interval(Duration::from_secs(10));
        ping.tick().await;
        let mut last_seen = std::time::Instant::now();

        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if last_seen.elapsed() > Duration::from_secs(25) {
                        tracing::warn!(session, "live upstream silent for {:?}: reconnecting", last_seen.elapsed());
                        continue 'conn;
                    }
                    if !send_up(&mut up_tx, UpMsg::Ping(Vec::new().into())).await {
                        continue 'conn;
                    }
                }
                b = b_rx.next() => match b {
                    Some(Ok(WsMsg::Text(t))) => {
                        let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                        match v.get("type").and_then(Value::as_str) {
                            Some("audio") => {
                                let data = v.get("data").and_then(Value::as_str).unwrap_or("");
                                st.levels.feed_pcm16(Who::User, &protocol::decode_pcm16(data));
                                if !send_up(&mut up_tx, UpMsg::Text(protocol::audio_input(data).to_string().into())).await {
                                    continue 'conn;
                                }
                            }
                            Some("text") => {
                                let text = v.get("text").and_then(Value::as_str).unwrap_or("");
                                user_buf.push_str(text);
                                if !send_up(&mut up_tx, UpMsg::Text(protocol::text_input(text).to_string().into())).await {
                                    continue 'conn;
                                }
                            }
                            Some("start") => {
                                if let Some(vc) = v.get("voice").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                                    voice = vc.to_string();
                                }
                            }
                            Some("stop") => {
                                let _ = up_tx.send(UpMsg::Close(None)).await;
                                break 'conn;
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(WsMsg::Close(_))) | None | Some(Err(_)) => {
                        let _ = up_tx.send(UpMsg::Close(None)).await;
                        break 'conn;
                    }
                    _ => {}
                },
                u = up_rx.next() => {
                    last_seen = std::time::Instant::now();
                    let text = match u {
                        Some(Ok(UpMsg::Text(t))) => t.to_string(),
                        Some(Ok(UpMsg::Binary(b))) => String::from_utf8_lossy(&b).into_owned(),
                        Some(Ok(UpMsg::Close(frame))) => {
                            // Google explains refusals here (quota, model, bad setup).
                            let why = frame.map(|f| format!("{} {}", f.code, f.reason)).unwrap_or_default();
                            tracing::warn!(session, "live upstream closed: {why}");
                            if voice_roll::refused(voice_rolled, set_up) {
                                handle = None;
                                voice_rolled = false;
                            }
                            if !ever_connected && !why.is_empty() {
                                status(&out, &st, &session, "error", &format!("Gemini closed the connection: {why}"));
                                break 'conn;
                            }
                            continue 'conn;
                        }
                        None | Some(Err(_)) => {
                            if voice_roll::refused(voice_rolled, set_up) {
                                handle = None;
                                voice_rolled = false;
                            }
                            continue 'conn;
                        }
                        _ => continue,
                    };
                    let Ok(msg) = serde_json::from_str::<Value>(&text) else { continue };
                    if tracing::enabled!(tracing::Level::DEBUG) {
                        let keys: Vec<&String> = msg.as_object().map(|o| o.keys().collect()).unwrap_or_default();
                        tracing::debug!(session, "gemini -> {:?} {}", keys, kagent_core::util::truncate_str(&text, 300));
                    }
                    for ev in protocol::translate(&msg) {
                        match ev {
                            ServerEvent::SetupComplete => {
                                // A working connection: reconnect budget starts over (long
                                // sessions roll on every goAway).
                                rolls = 0;
                                ever_connected = true;
                                set_up = true;
                                voice_rolled = false;
                                if paused.is_none() && st.settings.read().unwrap().voice_pauses_media() {
                                    paused = Some(kagent_core::tools::media::pause_playing(&media_ctx).await);
                                }
                                for frame in opening.drain(..) {
                                    if !send_up(&mut up_tx, UpMsg::Text(frame.to_string().into())).await {
                                        continue 'conn;
                                    }
                                }
                                status(&out, &st, &session, "live", detail);
                            }
                            ServerEvent::Audio { data, rate } => {
                                st.levels.feed_pcm16(Who::Assistant, &protocol::decode_pcm16(&data));
                                let _ = out.send(json!({"type": "audio", "data": data, "rate": rate}));
                            }
                            ServerEvent::UserTranscript(t) => {
                                user_buf.push_str(&t);
                                voice_cards.hear(&st, &session, &t, &out);
                                if let Some(answer) = voice_confirm.hear(&t) {
                                    let said = if answer == approve::Answer::Yes { "confirmed by voice" } else { "refused by voice" };
                                    let _ = out.send(json!({"type": "status", "state": "live", "detail": said}));
                                }
                                let _ = out.send(json!({"type": "transcript", "who": "user", "text": t, "final": false}));
                            }
                            ServerEvent::AssistantTranscript(t) => {
                                asst_buf.push_str(&t);
                                let _ = out.send(json!({"type": "transcript", "who": "assistant", "text": t, "final": false}));
                            }
                            ServerEvent::Interrupted => {
                                let _ = out.send(json!({"type": "interrupted"}));
                            }
                            ServerEvent::TurnComplete => {
                                close_turn!();
                                let _ = out.send(json!({"type": "turnComplete"}));
                                // `settings set liveVoice` (voice-control.md section 2): the sentence that
                                // announced the change is over, so the connection rolls with the new voice.
                                if let Some(next) = voice_roll::changed(&st.settings.read().unwrap().live_voice, &voice) {
                                    tracing::info!(session, "live voice {voice} -> {next}: rolling the connection");
                                    voice = next;
                                    voice_rolled = true;
                                    continue 'conn;
                                }
                            }
                            ServerEvent::ToolCalls(list) => {
                                for c in list {
                                    let id = if c.id.is_empty() { format!("live_{}", uuid::Uuid::new_v4().simple()) } else { c.id.clone() };
                                    // Registered here, before the task starts: a cancellation that follows at once must find the call.
                                    if mode != VoiceBrain::Live && c.name == "think" {
                                        let state = calls.start_think(&id);
                                        brain::spawn_think(st.clone(), session.clone(), c, id, state, out.clone(), tool_tx.clone(), voice_cards.clone(), calls.clone());
                                    } else {
                                        let own = cancel.child_token();
                                        calls.start_tool(id.clone(), own.clone());
                                        tool_call::spawn_tool(st.clone(), session.clone(), c, id, own, out.clone(), tool_tx.clone(), cancel.clone(), calls.clone(), voice_confirm.clone(), repeats.clone());
                                    }
                                }
                            }
                            // Cancelling the call does not cancel the brain turn (voice.md sections 2 and 6).
                            ServerEvent::ToolCallCancellation(ids) => brain::cancel_calls(&st, &session, &ids, &calls),
                            ServerEvent::ResumeHandle(h) => handle = Some(h),
                            ServerEvent::GoAway => continue 'conn,
                            ServerEvent::Error(e) => status(&out, &st, &session, "error", &e),
                        }
                    }
                },
                r = tool_rx.recv() => {
                    if let Some(r) = r {
                        tracing::debug!(session, "gemini <- {}", kagent_core::util::truncate_str(&r.to_string(), 300));
                        if !send_up(&mut up_tx, UpMsg::Text(r.to_string().into())).await {
                            continue 'conn;
                        }
                    }
                }
            }
        }
    }
    cancel.cancel();
    if let Some(buses) = paused {
        kagent_core::tools::media::resume(&media_ctx, &buses).await;
    }
    close_turn!();
    status(&out, &st, &session, "idle", "");
    drop(out);
    let _ = writer.await;
    Ok(())
}

/// Send to Gemini with a deadline: a stuck write must not freeze the audio loop.
async fn send_up<S>(up_tx: &mut S, msg: UpMsg) -> bool
where
    S: futures::Sink<UpMsg> + Unpin,
{
    matches!(tokio::time::timeout(Duration::from_secs(5), up_tx.send(msg)).await, Ok(Ok(())))
}
