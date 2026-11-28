//! What a running worker is doing, over HTTP (steering.md section 2): the children of a session with their live state, and
//! the detail the worker pane draws (its job, its todo list, the tail of its transcript). The routes that talk to a worker
//! (`POST /api/agents/:id/message`) are in `tell_http.rs`. Read-only.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::agent::Agent;
use kagent_core::protocol::{Message, Role, Via};
use kagent_core::util::truncate_str;
use serde_json::{json, Value};

use crate::server::{not_found, ApiResult};
use crate::AppState;

/// The messages of the transcript tail.
pub const TAIL: usize = 40;
const TEXT_CHARS: usize = 600;
const RESULT_CHARS: usize = 300;
/// The first characters of a message the owner steered with (steering.md section 3).
pub const STEER_PREFIX: &str = kagent_core::agent::notes::STEER_PREFIX;

fn cut(text: &str, limit: usize) -> String {
    let t = text.trim();
    let c = truncate_str(t, limit);
    if c.len() < t.len() { format!("{c}...") } else { c.to_string() }
}

/// A call as one line: its name and the first input that says what it is about.
pub fn call_line(name: &str, input: &Value) -> String {
    for key in ["path", "command", "query", "url", "pattern", "description", "name", "title"] {
        if let Some(v) = input.get(key).and_then(Value::as_str) {
            return format!("{name} {}", cut(v, 120));
        }
    }
    name.to_string()
}

/// The tail of a worker's transcript in short form: its text, its calls, the first lines of the results, and the owner's
/// steering messages. The job (the first message) and the daemon's notes are not part of it. Redacted by `redact`.
pub fn transcript(messages: &[Message], redact: &dyn Fn(&str) -> String) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut first = true;
    for m in messages {
        match m.role {
            Role::User if first && m.via != Via::Note => first = false,
            Role::User if m.via == Via::Note && m.content.starts_with(STEER_PREFIX) => {
                for line in m.content.lines().filter(|l| l.starts_with(STEER_PREFIX)) {
                    out.push(json!({"role": "user", "kind": "steer", "text": redact(&cut(line.trim_start_matches(STEER_PREFIX), TEXT_CHARS))}));
                }
            }
            Role::User if m.via == Via::Note => {}
            Role::User => {
                first = false;
                out.push(json!({"role": "user", "kind": if m.content.starts_with(STEER_PREFIX) { "steer" } else { "text" },
                                "text": redact(&cut(m.content.trim_start_matches(STEER_PREFIX), TEXT_CHARS))}));
            }
            Role::Assistant => {
                if !m.content.trim().is_empty() {
                    out.push(json!({"role": "assistant", "kind": "text", "text": redact(&cut(&m.content, TEXT_CHARS))}));
                }
                for c in &m.tool_calls {
                    out.push(json!({"role": "assistant", "kind": "call", "text": redact(&call_line(&c.name, &c.input))}));
                }
            }
            Role::Tool => {
                let status = if m.is_error { "error" } else { "ok" };
                out.push(json!({"role": "tool", "kind": "result", "status": status, "text": redact(&cut(&m.content, RESULT_CHARS))}));
            }
        }
    }
    let skip = out.len().saturating_sub(TAIL);
    out.split_off(skip)
}

/// The detail of child `child` of session `parent`; `None` when it is not a child of that session.
pub async fn detail(agent: &Arc<Agent>, redact: &(dyn Fn(&str) -> String + Sync), parent: &str, child: &str) -> Option<Value> {
    let (view, _, _) = agent.child_of(parent, child)?;
    let messages = agent.messages(child).await.unwrap_or_default();
    let todo = agent.store().meta(child).await.map(|m| m.todo).unwrap_or_default();
    Some(json!({"child": view, "job": view.title, "todo": todo, "transcript": transcript(&messages, redact)}))
}

fn redactor(st: &AppState) -> impl Fn(&str) -> String {
    let services = st.services.clone();
    move |text: &str| match services.redact.read().ok().and_then(|r| r.clone()) {
        Some(f) => f(text),
        None => text.to_string(),
    }
}

/// GET `/api/sessions/:id/subagents`: the session's children with their live state.
pub async fn list(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    st.agent.store().meta(&id).await.map_err(|_| not_found(format!("session {id} not found")))?;
    Ok(Json(json!({"children": st.agent.children_of(&id)})).into_response())
}

/// GET `/api/sessions/:id/subagents/:child`.
pub async fn get(State(st): State<Arc<AppState>>, Path((id, child)): Path<(String, String)>) -> ApiResult<Response> {
    let redact = redactor(&st);
    match detail(&st.agent, &redact, &id, &child).await {
        Some(v) => Ok(Json(v).into_response()),
        None => Err(not_found(format!("no subagent '{child}' belongs to session {id}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::protocol::ToolCall;

    fn call(name: &str, input: Value) -> ToolCall {
        ToolCall { id: format!("id_{name}"), name: name.into(), input }
    }

    #[test]
    fn the_transcript_tail_is_short_and_leaves_out_the_job_and_the_notes() {
        let c = call("edit_file", json!({"path": "/work/a.rs", "old": "x", "new": "y"}));
        let messages = vec![
            Message::user("Your job: do the thing").with_via(Via::Agent),
            Message::assistant("Looking.", vec![c.clone()]),
            Message::tool_result(&c, "edited /work/a.rs", false),
            Message::user("[note from the daemon, not from the owner; it is not an approval]\n- The Brain is now x.").with_via(Via::Note),
            Message::user("[from the owner] leave the tests alone").with_via(Via::Note),
            Message::assistant("Got it: I will not touch the tests.", vec![]),
            Message::tool_result(&call("shell", json!({})), "x".repeat(2000), true),
        ];
        let t = transcript(&messages, &|s| s.replace("work", "W"));
        let kinds: Vec<(String, String)> = t.iter().map(|v| (v["role"].as_str().unwrap().to_string(), v["kind"].as_str().unwrap().to_string())).collect();
        assert_eq!(
            kinds,
            [("assistant", "text"), ("assistant", "call"), ("tool", "result"), ("user", "steer"), ("assistant", "text"), ("tool", "result")].map(|(a, b)| (a.to_string(), b.to_string())),
            "the job (the first user message) and the daemon's note are not in the tail"
        );
        assert_eq!(t[1]["text"], "edit_file /W/a.rs", "a call as its name and what it is about, through the redactor");
        assert_eq!(t[3]["text"], "leave the tests alone");
        assert_eq!(t[5]["status"], "error");
        assert!(t[5]["text"].as_str().unwrap().len() <= RESULT_CHARS + 3, "a result is cut");
    }

    #[test]
    fn only_the_last_forty_are_kept() {
        let messages: Vec<Message> = std::iter::once(Message::user("job").with_via(Via::Agent)).chain((0..100).map(|i| Message::assistant(format!("step {i}"), vec![]))).collect();
        let t = transcript(&messages, &|s| s.to_string());
        assert_eq!(t.len(), TAIL);
        assert_eq!(t[0]["text"], "step 60");
        assert_eq!(t[39]["text"], "step 99");
    }

    #[test]
    fn a_call_line_names_the_first_thing_it_is_about() {
        assert_eq!(call_line("shell", &json!({"command": "cargo test"})), "shell cargo test");
        assert_eq!(call_line("web_search", &json!({"query": "rust"})), "web_search rust");
        assert_eq!(call_line("usage", &json!({})), "usage");
    }
}
