//! The routes of the hooks (api.md section 6): list, add, change, delete and test. The
//! work is in functions that take the hub, so they are tested without a server (a second
//! daemon cannot run next to the owner's, S17); the handlers wrap them.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use kagent_core::protocol::AgentEvent;
use kagent_core::tool::Tool;
use kagent_core::tools::shell::Shell;

use super::config::{Event, EVENTS, EVENT_NAMES, Hook, HooksSection};
use super::runner::{self, Ran, Verdict};
use super::HookHub;
use crate::server::ApiResult;
use crate::AppState;

/// Why the API answers with an error: the status, the code, the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub StatusCode, pub &'static str, pub String);

const INVALID: &str = "invalid_input";
const NOT_FOUND: &str = "not_found";

fn invalid(reason: impl Into<String>) -> Refusal {
    Refusal(StatusCode::BAD_REQUEST, INVALID, reason.into())
}

fn unknown(id: &str) -> Refusal {
    Refusal(StatusCode::NOT_FOUND, NOT_FOUND, format!("no hook '{id}'"))
}

impl From<Refusal> for crate::server::HttpError {
    fn from(r: Refusal) -> Self {
        crate::server::HttpError(r.0, r.1, r.2)
    }
}

/// The result of `POST /api/hooks/:id/test`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// `allow`, `ask` or `deny`; left out when the hook decided nothing, or failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<Verdict>,
    pub duration_ms: u64,
    pub timed_out: bool,
    /// Why the run failed, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TestResult {
    fn of(ran: Ran) -> Self {
        let Ran { exit_code, stdout, stderr, timed_out, duration_ms, decision } = ran;
        Self { exit_code, stdout, stderr, decision: decision.as_ref().ok().and_then(|d| d.verdict), duration_ms, timed_out, error: decision.err() }
    }
}

/// A `Hook` as a route reads it: strict (an unknown key is refused); the id may be left out.
fn parse(body: Value) -> Result<Hook, Refusal> {
    serde_json::from_value(body).map_err(|e| invalid(format!("invalid hook: {e}")))
}

fn restore(settings: &mut crate::settings::Settings, before: Option<Value>) {
    match before {
        Some(before) => {
            settings.extra.insert("hooks".into(), before);
        }
        None => {
            settings.extra.remove("hooks");
        }
    }
}

impl HookHub {
    /// What `GET /api/hooks` lists.
    pub fn views(&self) -> Vec<Hook> {
        self.section().all().cloned().collect()
    }

    /// `POST /api/hooks`: `body` names no id; the daemon assigns one.
    pub fn create(&self, body: Value) -> Result<Hook, Refusal> {
        let mut hook = parse(body)?;
        if !hook.id.is_empty() {
            return Err(invalid("leave id out of a new hook: the daemon assigns it"));
        }
        hook.id = Hook::fresh_id();
        hook.check().map_err(invalid)?;
        self.change(|section| section.upsert(hook.clone()))?;
        Ok(hook)
    }

    /// `PUT /api/hooks/:id`: the whole record. The id in it, when there is one, is the one in the path.
    pub fn update(&self, id: &str, body: Value) -> Result<Hook, Refusal> {
        if self.section().find(id).is_none() {
            return Err(unknown(id));
        }
        let mut hook = parse(body)?;
        if !hook.id.is_empty() && hook.id != id {
            return Err(invalid(format!("the body names the hook '{}' and the path '{}'", hook.id, id)));
        }
        hook.id = id.to_string();
        hook.check().map_err(invalid)?;
        self.change(|section| section.upsert(hook.clone()))?;
        Ok(hook)
    }

    /// `DELETE /api/hooks/:id`.
    pub fn delete(&self, id: &str) -> Result<(), Refusal> {
        if self.section().find(id).is_none() {
            return Err(unknown(id));
        }
        self.change(|section| {
            section.remove(id);
        })
    }

    /// Change the section, check the settings as a whole, save them, and tell every UI.
    fn change(&self, edit: impl FnOnce(&mut HooksSection)) -> Result<(), Refusal> {
        let mut settings = self.settings.write().unwrap();
        let before = settings.extra.get("hooks").cloned();
        let mut section = settings.hooks_config();
        edit(&mut section);
        settings.extra.insert("hooks".into(), serde_json::to_value(&section).unwrap_or_default());
        if let Err(e) = settings.validate_sections() {
            restore(&mut settings, before);
            return Err(Refusal(StatusCode::BAD_REQUEST, INVALID, e.to_string()));
        }
        if let Err(e) = settings.save(&self.path) {
            restore(&mut settings, before);
            return Err(Refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("cannot save the settings: {e}")));
        }
        let saved = settings.clone();
        drop(settings);
        let _ = self.teller.events.send(AgentEvent::SettingsChanged { settings: serde_json::to_value(&saved).unwrap_or(Value::Null) });
        Ok(())
    }

    /// `POST /api/hooks/:id/test`: the hook as it is stored, with the sample of `event`
    /// (its own event when none is named, and the documented example when no `sample`
    /// comes). Nothing of a test reaches the agent.
    pub async fn test(&self, id: &str, event: Option<Event>, sample: Option<Value>) -> Result<TestResult, Refusal> {
        let Some(hook) = self.section().find(id).cloned() else { return Err(unknown(id)) };
        let payload = sample.unwrap_or_else(|| sample_of(event.unwrap_or(hook.event)));
        Ok(TestResult::of(runner::run(&hook, &payload, super::TEST, Some(&self.teller.audit)).await))
    }
}

/// The payload of each event as the test panel shows it by default (the documented example).
pub fn samples() -> serde_json::Map<String, Value> {
    EVENTS.iter().map(|e| (e.name().to_string(), sample_of(*e))).collect()
}

/// The documented example of what a hook gets on stdin at `event`.
pub fn sample_of(event: Event) -> Value {
    let mut sample = json!({ "event": event.name(), "ts": "2026-11-26T10:15:30.123Z", "sessionId": "sess_1a2b3c4d", "via": "text" });
    let input = json!({ "command": "systemctl status nginx" });
    let extra = match event {
        Event::SessionStart => json!({}),
        Event::UserPromptSubmit => json!({ "prompt": "Why is the fan so loud?" }),
        Event::PreToolUse => {
            let assessment = Shell::new().assess(&input);
            json!({ "tool": "shell", "input": input, "assessment": serde_json::to_value(&assessment).unwrap(), "summary": assessment.summary })
        }
        Event::PostToolUse => json!({
            "tool": "shell",
            "input": input,
            "output": "* nginx.service - A high performance web server\n     Loaded: loaded\n     Active: active (running)",
            "isError": false,
            "durationMs": 42,
        }),
        Event::Stop => json!({ "turnId": "turn_1a2b3c4d", "status": "completed", "error": null }),
        Event::AlertRaised => json!({ "alert": sample_alert() }),
    };
    if let (Some(base), Some(extra)) = (sample.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    sample
}

fn sample_alert() -> Value {
    use crate::watch::{Alert, AlertStatus, Severity};
    let ts = "2026-11-26T10:15:30.123Z".parse().unwrap();
    serde_json::to_value(Alert {
        id: "alr_20261126_101530_1a2b3c".into(),
        ts,
        last_seen: ts,
        count: 3,
        rule: "disk-space".into(),
        severity: Severity::Review,
        status: AlertStatus::Open,
        title: "Disk nearly full".into(),
        cause: "/ is 92% full".into(),
        evidence: json!({ "mount": "/", "usedPct": 92 }),
        proposal: "Free disk space: disk_info says where it went".into(),
        dedupe_key: "disk:/".into(),
        note: None,
    })
    .unwrap()
}

pub async fn list(State(st): State<Arc<AppState>>) -> Response {
    Json(json!({ "hooks": st.hooks.views(), "events": EVENT_NAMES, "samples": samples() })).into_response()
}

pub async fn create(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let hook = st.hooks.create(body)?;
    Ok((StatusCode::CREATED, Json(hook)).into_response())
}

pub async fn update(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok(Json(st.hooks.update(&id, body)?).into_response())
}

pub async fn delete(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    st.hooks.delete(&id)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The body of the test route: the event whose sample is sent, and the sample itself.
#[derive(Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct TestBody {
    event: Option<String>,
    sample: Option<Value>,
}

pub async fn test(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: Option<Json<Value>>) -> ApiResult<Response> {
    let body: TestBody = body.map(|Json(b)| serde_json::from_value(b).map_err(|e| invalid(format!("invalid body: {e}")))).transpose()?.unwrap_or_default();
    let event = body
        .event
        .as_deref()
        .map(|name| Event::parse(name).ok_or_else(|| invalid(format!("unknown event '{name}' (one of {})", EVENT_NAMES.join(", ")))))
        .transpose()?;
    if body.sample.as_ref().is_some_and(|s| !s.is_object()) {
        return Err(invalid("sample must be an object").into());
    }
    Ok(Json(st.hooks.test(&id, event, body.sample).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::super::runner::{Decision, Ran};
    use super::*;

    #[test]
    fn every_event_has_its_documented_example() {
        let samples = samples();
        let mut names = EVENT_NAMES.to_vec();
        names.sort_unstable();
        assert_eq!(samples.keys().cloned().collect::<Vec<_>>(), names, "one per event");
        for event in EVENTS {
            let sample = sample_of(event);
            assert_eq!((sample["event"].as_str(), sample["sessionId"].as_str(), sample["via"].as_str()), (Some(event.name()), Some("sess_1a2b3c4d"), Some("text")));
            assert!(sample["ts"].as_str().is_some_and(|t| t.ends_with('Z')), "{}", sample["ts"]);
        }
        let pre = sample_of(Event::PreToolUse);
        assert_eq!((pre["tool"].as_str(), pre["summary"].as_str()), (Some("shell"), Some("systemctl status nginx")));
        assert_eq!(pre["assessment"]["readOnly"], true, "the assessment as the engine sees it, in its JSON form");
        let post = sample_of(Event::PostToolUse);
        assert_eq!(json!([post["output"].as_str().is_some(), post["isError"], post["durationMs"]]), json!([true, false, 42]));
        assert_eq!(sample_of(Event::UserPromptSubmit)["prompt"].as_str(), Some("Why is the fan so loud?"));
        assert_eq!(sample_of(Event::Stop)["status"], "completed");
        assert_eq!(sample_of(Event::AlertRaised)["alert"]["rule"], "disk-space");
        assert_eq!(sample_of(Event::SessionStart).as_object().unwrap().len(), 4, "the four fields of every payload");
    }

    #[test]
    fn the_test_result_names_what_the_run_decided() {
        let result = |decision| {
            let ran = Ran { exit_code: Some(2), stdout: String::new(), stderr: "no".into(), timed_out: false, duration_ms: 7, decision };
            serde_json::to_value(TestResult::of(ran)).unwrap()
        };
        assert_eq!(
            result(Ok(Decision { verdict: Some(Verdict::Deny), ..Default::default() })),
            json!({ "exitCode": 2, "stdout": "", "stderr": "no", "decision": "deny", "durationMs": 7, "timedOut": false })
        );
        assert_eq!(result(Ok(Decision::default()))["decision"], Value::Null, "no decision: the field is left out");
        assert_eq!(result(Err("timed out after 200 ms".into()))["error"], "timed out after 200 ms");
    }

    #[test]
    fn a_hook_is_read_strictly() {
        let hook = parse(json!({ "event": "Stop", "command": "/bin/true" })).unwrap();
        assert_eq!((hook.event, hook.timeout_ms), (Event::Stop, 5000));
        let err = parse(json!({ "event": "Stop", "comand": "/bin/true" })).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.2.contains("unknown field `comand`") && err.2.contains("`command`"), "{}", err.2);
        assert!(parse(json!({ "command": "/bin/true" })).unwrap_err().2.contains("missing field `event`"));
        assert!(parse(json!({ "event": "onc", "command": "/bin/true" })).unwrap_err().2.contains("unknown variant `onc`"));
    }
}
