//! Live voice through the daemon (IMPLEMENTATION.md S10, api.md section 5).
//!
//! Browser <-> `/live` <-> kagentd <-> Gemini BidiGenerateContent. The API key never
//! leaves the daemon; tool calls run through the same agent path as text turns
//! (permissions, approvals, audit); transcripts land in the session; levels feed the
//! visualizers. The browser socket survives upstream rolls (goAway / drops) thanks to
//! session resumption handles.

pub mod approve;
pub mod protocol;

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

use kagent_core::protocol::{AgentEvent, Message, Role, ToolCall, Via};

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

/// Voice persona + recent text context so voice and chat feel like one conversation.
fn instruction(st: &AppState, history: &[Message]) -> String {
    let base = st.agent.config().system_prompt;
    let mut recent = String::new();
    for m in history.iter().filter(|m| matches!(m.role, Role::User | Role::Assistant) && !m.content.trim().is_empty()).rev().take(8).collect::<Vec<_>>().into_iter().rev() {
        let who = if m.role == Role::User { "User" } else { "You" };
        recent.push_str(&format!("{who}: {}\n", kagent_core::util::truncate_str(m.content.trim(), 300)));
    }
    format!(
        "{base}\n\nVOICE MODE: you are speaking out loud. Answer in one to three short spoken sentences, \
no markdown, no lists, no code. Do not narrate tool calls or repeat yourself; say a few words first \
only when an action will take a while. Answer with the result. Actions \
that change the system show an approval card on the screen: say what you asked for and that it waits \
for approval there; the user can answer it by voice ('yes' / 'no'; root actions need 'approve'), so \
ask a short question like 'Shall I send it?' and wait; never claim it is done before the result arrives.\n\
Destructive actions (reboot, power off, kill, closing windows, deleting) ALWAYS need the user's \
explicit spoken confirmation, even with full control: first warn about unsaved work and ask \
'Shall I reboot now? Say yes, reboot.' A question from the user ('should we reboot?') is never \
a request; one-word or garbled fragments are never a confirmation.\n\
Listening: the user speaks English or Slovenian. Transcripts are noisy: a fragment in another \
language (German, Spanish, Italian, Japanese...) or one that makes no sense in context is a \
mis-hearing or background noise: ignore it or ask the user to repeat; never type, send or act on it.\n\
Dictation: when the user says what to write to someone, the message is only their words for the \
other person, never the instruction around it ('send this', 'yes', 'tell him', 'okay'). Use the \
`message` tool: it types without sending; read the exact text back and send only after a yes. If \
the user says 'what I told you', use their earlier exact words; if you are not sure, ask.\n\
Retries: when something did not work, check first (screen with window, focus_context) and fix the \
cause; do not repeat the same action blindly, and never type the same text twice.{}",
        if recent.is_empty() { String::new() } else { format!("\n\nRecent conversation:\n{recent}") }
    )
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
        None => st.agent.store().create("Voice session").await?.id,
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
    let mut voice = st.settings.read().unwrap().live_voice.clone();
    let mut handle: Option<String> = None;
    let (mut user_buf, mut asst_buf) = (String::new(), String::new());
    let mut rolls = 0u32;
    let mut ever_connected = false;
    // Media paused when the session went live, resumed when it ends (voice.pauseMedia).
    let media_ctx = kagent_core::tool::ToolContext::standalone(Duration::from_secs(3), 64_000);
    let mut paused: Option<Vec<String>> = None;
    // Speech heard since each pending approval card appeared (voice yes/no).
    let mut heard: HashMap<String, String> = HashMap::new();

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
        let history = st.agent.messages(&session).await.unwrap_or_default();
        let tools = st.agent.advertised_specs();
        let setup = protocol::setup_message(&SetupParams {
            model: &target.model,
            voice: &voice,
            instruction: &instruction(&st, &history),
            tools: &tools,
            resume_handle: handle.as_deref(),
        });
        if !send_up(&mut up_tx, UpMsg::Text(setup.to_string().into())).await {
            continue;
        }
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
                            if !ever_connected && !why.is_empty() {
                                status(&out, &st, &session, "error", &format!("Gemini closed the connection: {why}"));
                                break 'conn;
                            }
                            continue 'conn;
                        }
                        None | Some(Err(_)) => continue 'conn,
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
                                if paused.is_none() && st.settings.read().unwrap().voice_pauses_media() {
                                    paused = Some(kagent_core::tools::media::pause_playing(&media_ctx).await);
                                }
                                status(&out, &st, &session, "live", "");
                            }
                            ServerEvent::Audio { data, rate } => {
                                st.levels.feed_pcm16(Who::Assistant, &protocol::decode_pcm16(&data));
                                let _ = out.send(json!({"type": "audio", "data": data, "rate": rate}));
                            }
                            ServerEvent::UserTranscript(t) => {
                                user_buf.push_str(&t);
                                voice_approvals(&st, &session, &t, &mut heard, &out);
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
                                flush_transcripts(&st, &session, &mut user_buf, &mut asst_buf).await;
                                let _ = out.send(json!({"type": "turnComplete"}));
                            }
                            ServerEvent::ToolCalls(calls) => {
                                for c in calls {
                                    spawn_tool(st.clone(), session.clone(), c, out.clone(), tool_tx.clone(), cancel.clone());
                                }
                            }
                            ServerEvent::ToolCallCancellation(_) => {}
                            ServerEvent::ResumeHandle(h) => handle = Some(h),
                            ServerEvent::GoAway => continue 'conn,
                            ServerEvent::Error(e) => status(&out, &st, &session, "error", &e),
                        }
                    }
                },
                r = tool_rx.recv() => {
                    if let Some(r) = r {
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
    flush_transcripts(&st, &session, &mut user_buf, &mut asst_buf).await;
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

/// Resolve this session's pending approval cards from the user's own speech.
fn voice_approvals(st: &AppState, session: &str, chunk: &str, heard: &mut HashMap<String, String>, out: &mpsc::UnboundedSender<Value>) {
    let pending: Vec<_> = st.agent.broker().pending().into_iter().filter(|a| a.session_id == session).collect();
    heard.retain(|id, _| pending.iter().any(|p| &p.request_id == id));
    for p in pending {
        let buf = heard.entry(p.request_id.clone()).or_default();
        buf.push_str(chunk);
        let Some(answer) = approve::classify_for(buf, p.privileged || p.destructive, &p.summary) else { continue };
        let decision = match answer {
            approve::Answer::Yes => kagent_core::permission::ApprovalDecision::AllowOnce,
            approve::Answer::No => kagent_core::permission::ApprovalDecision::Deny { reason: Some("denied by voice".into()) },
        };
        let said = buf.trim().to_string();
        if st.agent.broker().resolve(&p.request_id, decision) {
            tracing::info!("voice {} '{}' for {}: \"{said}\"", if answer == approve::Answer::Yes { "approved" } else { "denied" }, p.tool, p.summary);
            let _ = out.send(json!({"type": "tool", "callId": p.call_id, "name": p.tool, "state": if answer == approve::Answer::Yes { "approved by voice" } else { "denied by voice" }}));
        }
        heard.remove(&p.request_id);
    }
}

/// Tool calls run off the audio loop: an approval can take a minute.
fn spawn_tool(st: Arc<AppState>, session: String, c: protocol::FunctionCall, out: mpsc::UnboundedSender<Value>, tool_tx: mpsc::UnboundedSender<Value>, cancel: CancellationToken) {
    tokio::spawn(async move {
        let id = if c.id.is_empty() { format!("live_{}", uuid::Uuid::new_v4().simple()) } else { c.id.clone() };
        let _ = out.send(json!({"type": "tool", "callId": id, "name": c.name, "state": "started"}));
        let call = ToolCall { id: id.clone(), name: c.name.clone(), input: c.args.clone() };
        let privilege = st.settings.read().unwrap().privilege_method();
        let (msg, status) = st.agent.execute_call(&session, "live", &call, &cancel, privilege).await;
        // Keep the voice tool call in the session so the transcript shows what was done.
        let record = vec![Message::assistant(String::new(), vec![call.clone()]).with_via(Via::Voice), msg.clone()];
        if let Err(e) = st.agent.append_external(&session, record).await {
            tracing::warn!("could not store voice tool call: {e:#}");
        }
        let state = serde_json::to_value(status).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let _ = out.send(json!({"type": "tool", "callId": id, "name": c.name, "state": state}));
        // Keep the spoken context small: the model only needs the gist.
        let result = json!({"status": state, "output": kagent_core::util::truncate_str(&msg.content, 6_000)});
        let _ = tool_tx.send(protocol::tool_response(&[(c.id, c.name, result)]));
    });
}
