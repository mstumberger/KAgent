//! Hooks (hooks.md): commands of the owner's own that run at points of the agent's life.
//! The records are in `settings.hooks` (`config.rs`), `runner.rs` runs one and `http.rs`
//! serves them. `HookHub` is what the loop and its tool calls ask (the `AgentHooks` and
//! `ToolHooks` of kagent-core) and what the watchers tell of an alert.

pub mod config;
pub mod http;
pub mod runner;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use kagent_core::agent::{AgentHooks, HookEvent, HookInput, HookVerdict, ToolHooks};
use kagent_core::audit::{redact_input, AuditLog};
use kagent_core::protocol::{AgentEvent, ToolCall, Via};
use kagent_core::tool::CallAssessment;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use config::{Event, Hook, HooksSection};
use runner::{Decision, Ran, Verdict};

/// The session named in the audit record of a hook that ran for an alert.
const WATCH: &str = "watch";
/// The session named in the audit record of a test run from Settings -> Hooks.
pub(crate) const TEST: &str = "hook-test";

/// What a run leaves behind outside its result: the audit record, a log line and, for a
/// failure, the notice. Cloned into the tasks nobody waits for.
#[derive(Clone)]
struct Teller {
    audit: Arc<AuditLog>,
    events: broadcast::Sender<AgentEvent>,
}

impl Teller {
    /// Run one hook that is awaited. A failure is told, never raised (hooks.md section 3).
    async fn run(&self, hook: &Hook, payload: &Value, session: &str) -> Ran {
        let ran = runner::run(hook, payload, session, Some(&self.audit)).await;
        if let Err(why) = &ran.decision {
            self.failed(&hook.id, hook.event, why);
        }
        ran
    }

    fn failed(&self, id: &str, event: Event, why: &str) {
        tracing::warn!(hook = id, event = event.name(), "hook failed: {why}");
        let _ = self.events.send(AgentEvent::Notice {
            level: "warn".into(),
            text: format!("Hook '{id}' ({}) failed: {why}. The agent goes on without it.", event.name()),
        });
    }
}

pub struct HookHub {
    settings: Arc<RwLock<crate::settings::Settings>>,
    /// settings.json: saved after a change made through the hooks routes.
    path: PathBuf,
    teller: Teller,
    /// `via` of the turn running in each session: the payload of the tool events names it.
    via: Mutex<HashMap<String, Via>>,
    /// Context an `async` hook added, for the next turn of its session.
    later: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl HookHub {
    pub fn new(
        settings: Arc<RwLock<crate::settings::Settings>>,
        path: PathBuf,
        audit: Arc<AuditLog>,
        events: broadcast::Sender<AgentEvent>,
    ) -> Self {
        Self { settings, path, teller: Teller { audit, events }, via: Mutex::new(HashMap::new()), later: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// The hooks as the settings hold them now: read at every event, so a change applies from the next one.
    pub fn section(&self) -> HooksSection {
        self.settings.read().unwrap().hooks_config()
    }

    fn via_of(&self, session: &str) -> Via {
        self.via.lock().unwrap().get(session).copied().unwrap_or(Via::Text)
    }

    /// An `async` hook (hooks.md section 2): started here, awaited by nobody. Its
    /// context, if `later` is set, is kept for the next turn of `session`.
    fn start_async(&self, hook: &Hook, payload: Value, session: &str, later: bool) {
        let (teller, later) = (self.teller.clone(), later.then(|| self.later.clone()));
        let (id, event, session) = (hook.id.clone(), hook.event, session.to_string());
        runner::spawn(hook.clone(), payload, session.clone(), Some(teller.audit.clone()), move |ran| {
            if let Err(why) = &ran.decision {
                teller.failed(&id, event, why);
            }
            if let (Some(later), Some(context)) = (later, ran.decision.ok().and_then(|d| d.additional_context)) {
                let mut later = later.lock().unwrap();
                later.entry(session).or_default().push(format!("(from the {} hook '{id}', run in the background) {context}", event.name()));
            }
        });
    }
}

/// The payload every hook gets on stdin (hooks.md section 2): the event, when, which
/// session and how it was reached, then what the event carries.
pub fn payload(event: Event, session: &str, via: Via, extra: Value) -> Value {
    let mut payload = json!({
        "event": event.name(),
        "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "sessionId": session,
        "via": via,
    });
    if let (Some(base), Value::Object(extra)) = (payload.as_object_mut(), extra) {
        base.extend(extra);
    }
    payload
}

/// The contexts of several hooks as one.
fn joined(contexts: Vec<String>) -> Option<String> {
    (!contexts.is_empty()).then(|| contexts.join("\n\n"))
}

/// `b` after `a`.
fn after(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (a, None) => a,
        (None, b) => b,
    }
}

#[async_trait]
impl ToolHooks for HookHub {
    /// `PreToolUse`. The hooks run in the order they are stored; the first denial ends
    /// the call and the hooks after it; an `ask` from any of them asks.
    async fn pre(&self, session_id: &str, call: &ToolCall, assessment: &CallAssessment) -> HookInput {
        let hooks = self.section().matching(Event::PreToolUse, &call.name);
        if hooks.is_empty() {
            return HookInput::ALLOW;
        }
        let via = self.via_of(session_id);
        let mut out = HookInput::ALLOW;
        for hook in &hooks {
            // A hook sees the input as the hook before it left it.
            let input = out.input.clone().unwrap_or_else(|| call.input.clone());
            let payload = payload(Event::PreToolUse, session_id, via, json!({ "tool": call.name, "input": redact_input(&input), "assessment": assessment, "summary": assessment.summary }));
            if hook.r#async {
                self.start_async(hook, payload, session_id, true);
                continue;
            }
            let decision: Decision = self.teller.run(hook, &payload, session_id).await.or_allow();
            out.context = after(out.context.take(), decision.additional_context);
            if let Some(input) = decision.updated_input {
                out.input = Some(input);
            }
            match decision.verdict {
                Some(Verdict::Deny) => {
                    out.verdict = HookVerdict::Deny;
                    out.reason = decision.reason;
                    return out;
                }
                Some(Verdict::Ask) if out.verdict == HookVerdict::Allow => {
                    out.verdict = HookVerdict::Ask;
                    out.reason = decision.reason;
                }
                _ => {}
            }
        }
        out
    }

    /// `PostToolUse`: `call` is what ran (the input a `PreToolUse` hook put in its place included).
    async fn post(&self, session_id: &str, call: &ToolCall, output: &str, is_error: bool, duration_ms: u64) -> Option<String> {
        let hooks = self.section().matching(Event::PostToolUse, &call.name);
        if hooks.is_empty() {
            return None;
        }
        let via = self.via_of(session_id);
        let mut context = None;
        for hook in &hooks {
            let payload = payload(Event::PostToolUse, session_id, via, json!({ "tool": call.name, "input": redact_input(&call.input), "output": output, "isError": is_error, "durationMs": duration_ms }));
            if hook.r#async {
                self.start_async(&hook, payload, session_id, true);
                continue;
            }
            context = after(context, self.teller.run(hook, &payload, session_id).await.or_allow().additional_context);
        }
        context
    }
}

#[async_trait]
impl AgentHooks for HookHub {
    /// `SessionStart`, `UserPromptSubmit` and `Stop`, as the loop raises them.
    async fn event(&self, event: HookEvent<'_>) -> Option<String> {
        let (name, session, via) = match event {
            HookEvent::SessionStart { session_id, via, .. } => (Event::SessionStart, session_id, via),
            HookEvent::UserPromptSubmit { session_id, via, .. } => (Event::UserPromptSubmit, session_id, via),
            HookEvent::Stop { session_id, via, .. } => (Event::Stop, session_id, via),
        };
        let extra = match event {
            HookEvent::SessionStart { .. } => json!({}),
            HookEvent::UserPromptSubmit { prompt, .. } => {
                // From here the tool calls of the session belong to this turn.
                self.via.lock().unwrap().insert(session.to_string(), via);
                json!({ "prompt": prompt })
            }
            HookEvent::Stop { turn_id, status, error, .. } => {
                self.via.lock().unwrap().remove(session);
                json!({ "turnId": turn_id, "status": status, "error": error })
            }
        };
        // Context an `async` hook added since the last turn of this session (hooks.md section 2).
        let mut context = Vec::new();
        if name == Event::UserPromptSubmit {
            context.extend(self.later.lock().unwrap().remove(session).unwrap_or_default());
        }
        for hook in self.section().matching(name, "") {
            let payload = payload(name, session, via, extra.clone());
            if hook.r#async {
                self.start_async(&hook, payload, session, true);
                continue;
            }
            context.extend(self.teller.run(&hook, &payload, session).await.or_allow().additional_context);
        }
        joined(context)
    }
}

impl crate::watch::AlertPush for HookHub {
    /// `AlertRaised` (hooks.md section 4): the hooks of the event run for it, and nothing waits for them.
    fn raised(&self, alert: &crate::watch::Alert) {
        for hook in self.section().matching(Event::AlertRaised, "") {
            let payload = payload(Event::AlertRaised, WATCH, Via::Text, json!({ "alert": alert }));
            if hook.r#async {
                self.start_async(&hook, payload, WATCH, false);
                continue;
            }
            let teller = self.teller.clone();
            tokio::spawn(async move { teller.run(&hook, &payload, WATCH).await; });
        }
    }
}

#[cfg(test)]
mod tests;
