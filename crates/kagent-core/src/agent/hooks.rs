//! Hooks (hooks.md sections 1, 3 and 4): the points of the agent's life a hook command
//! of the owner may watch. `kagent-core` does not know the daemon's hooks: the loop asks
//! the two traits when the daemon plugged them in, and goes on exactly as before when it
//! did not (no hook, no call, nothing stored).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use super::{Agent, SessionState};
use crate::protocol::{Message, ToolCall, TurnStatus, Via};
use crate::tool::CallAssessment;
use crate::util::{normalize_input, truncate_str, validate_schema};

pub(crate) mod protect;

/// Context hooks may add to one turn, in characters (hooks.md section 3).
pub const CONTEXT_CHARS_PER_TURN: usize = 4_000;
/// How much of a tool's output a `PostToolUse` hook sees.
const TOOL_OUTPUT_CHARS: usize = 4_000;
/// The `Stop` hooks of one turn run in a task of their own for at most this long
/// (hooks.md section 4); the longest single hook may take 60 s.
pub const STOP_HOOKS_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);

/// A point of the agent's life that is not about one tool call (hooks.md section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent<'a> {
    /// Before the first turn of a session: its history is still empty.
    SessionStart { session_id: &'a str, via: Via },
    /// Before the first model request of a turn; `prompt` is the message that started it.
    UserPromptSubmit { session_id: &'a str, via: Via, prompt: &'a str },
    /// After `TurnFinished`.
    Stop { session_id: &'a str, via: Via, turn_id: &'a str, status: TurnStatus, error: Option<&'a str> },
}

impl HookEvent<'_> {
    /// The event as the payload's `event` field names it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SessionStart { .. } => "SessionStart",
            Self::UserPromptSubmit { .. } => "UserPromptSubmit",
            Self::Stop { .. } => "Stop",
        }
    }

    pub fn session_id(&self) -> &str {
        match self {
            Self::SessionStart { session_id, .. } | Self::UserPromptSubmit { session_id, .. } | Self::Stop { session_id, .. } => session_id,
        }
    }
}

/// The hooks of the loop the daemon provides; `None` of them when the daemon has none.
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// Runs the hooks of `event`. `Some(context)` is `additionalContext`: it is stored
    /// for the next model request of the session.
    async fn event(&self, event: HookEvent<'_>) -> Option<String>;
}

/// What a `PreToolUse` hook says of a call (hooks.md section 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookVerdict {
    /// Go on: the call is judged as if the hook were not there.
    Allow,
    /// A card, even where the permission engine would allow the call.
    Ask,
    /// The call ends with "not executed".
    Deny,
}

/// What the `PreToolUse` hooks said about a call.
#[derive(Debug, Clone, PartialEq)]
pub struct HookInput {
    pub verdict: HookVerdict,
    /// Why a hook asks or denies.
    pub reason: String,
    /// `updatedInput`: the input in place of the model's. `None` keeps the model's.
    pub input: Option<Value>,
    /// `additionalContext`, for the next model request of the session.
    pub context: Option<String>,
}

impl HookInput {
    /// What a hook that is missing or failed counts as (hooks.md section 4).
    pub const ALLOW: Self = Self { verdict: HookVerdict::Allow, reason: String::new(), input: None, context: None };
}

/// The hooks of the tool calls the daemon provides, reached through `ToolServices`.
#[async_trait]
pub trait ToolHooks: Send + Sync {
    /// `PreToolUse`: after the schema check and before `authorize`. The hook sees the
    /// call as it was assessed for the permission engine.
    async fn pre(&self, session_id: &str, call: &ToolCall, assessment: &CallAssessment) -> HookInput;
    /// `PostToolUse`: after the tool answered. `Some(context)` is `additionalContext`.
    async fn post(&self, session_id: &str, call: &ToolCall, output: &str, is_error: bool, duration_ms: u64) -> Option<String>;
}

/// Why a call ended at its `PreToolUse` hooks.
pub(crate) enum HookRefusal {
    /// A hook denied the call: "not executed: <reason>".
    Denied(String),
    /// A hook replaced the input with one the tool's schema refuses: the call fails.
    Invalid(String),
}

/// The daemon's hooks of the loop, and the context they added for the next model
/// request of each session: `(event, context)`, in the order it came.
pub(crate) struct HookState {
    daemon: Option<Arc<dyn AgentHooks>>,
    pending: Mutex<HashMap<String, Vec<(&'static str, String)>>>,
}

impl HookState {
    pub(crate) fn new(daemon: Option<Arc<dyn AgentHooks>>) -> Self {
        Self { daemon, pending: Mutex::new(HashMap::new()) }
    }
}

/// At most what is left of `budget` characters of `text`, on a character boundary.
fn take(budget: &mut usize, text: &str) -> Option<String> {
    let kept: String = text.trim().chars().take(*budget).collect();
    *budget -= kept.chars().count();
    (!kept.is_empty()).then_some(kept)
}

impl Agent {
    /// The hooks at the start of a turn: `SessionStart` before the first turn of a
    /// session (its history is empty), then `UserPromptSubmit`. What they add waits in
    /// the session for the turn's first model request.
    pub(crate) async fn turn_hooks(&self, session_id: &str, state: &SessionState, prompt: &str, via: Via) {
        let Some(hooks) = self.hooks.daemon.clone() else { return };
        let first = state.messages.lock().await.is_empty();
        let mut added = Vec::new();
        if first {
            let event = HookEvent::SessionStart { session_id, via };
            added.extend(hooks.event(event).await.map(|text| (event.name(), text)));
        }
        let event = HookEvent::UserPromptSubmit { session_id, via, prompt };
        added.extend(hooks.event(event).await.map(|text| (event.name(), text)));
        self.add_hook_context(session_id, added);
    }

    /// `Stop`, after `TurnFinished`, in a task of its own that nobody awaits, for at most
    /// `STOP_HOOKS_LIMIT`: a slow or hung hook never holds the turn, `wait_idle` or the
    /// next turn. What it adds waits for a later turn.
    pub(crate) fn spawn_stop_hooks(self: &Arc<Self>, session_id: &str, turn_id: &str, via: Via, status: TurnStatus, error: Option<String>) {
        if self.hooks.daemon.is_none() {
            return;
        }
        let (this, session_id, turn_id) = (self.clone(), session_id.to_string(), turn_id.to_string());
        tokio::spawn(async move {
            if tokio::time::timeout(STOP_HOOKS_LIMIT, this.stop_hooks(&session_id, &turn_id, via, status, error.as_deref())).await.is_err() {
                tracing::warn!(session_id, turn_id, "the Stop hooks took longer than {} s and were dropped", STOP_HOOKS_LIMIT.as_secs());
            }
        });
    }

    /// The `Stop` hooks of one turn, awaited (`spawn_stop_hooks` runs them).
    pub(crate) async fn stop_hooks(&self, session_id: &str, turn_id: &str, via: Via, status: TurnStatus, error: Option<&str>) {
        let Some(hooks) = self.hooks.daemon.clone() else { return };
        let event = HookEvent::Stop { session_id, via, turn_id, status, error };
        // A panicking hook must not take the task down without a word.
        let run = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(hooks.event(event)));
        let added = match run.await {
            Ok(text) => text.map(|text| ("Stop", text)),
            Err(_) => {
                tracing::error!(session_id, turn_id, "the Stop hooks panicked");
                None
            }
        };
        self.add_hook_context(session_id, added);
    }

    /// The context hooks added since the last model request of the session, as stored
    /// user messages; together at most what is left of `budget` characters.
    pub(crate) fn hook_messages(&self, session_id: &str, budget: &mut usize) -> Vec<Message> {
        let added = self.hooks.pending.lock().unwrap().remove(session_id).unwrap_or_default();
        added.into_iter().filter_map(|(event, text)| take(budget, &text).map(|text| Message::user(format!("[hook {event}] {text}")))).collect()
    }

    fn add_hook_context(&self, session_id: &str, added: impl IntoIterator<Item = (&'static str, String)>) {
        let added: Vec<_> = added.into_iter().filter(|(_, text)| !text.trim().is_empty()).collect();
        if added.is_empty() {
            return;
        }
        let mut pending = self.hooks.pending.lock().unwrap();
        // A session the agent forgot meanwhile has no request left that could read it.
        if self.sessions.lock().unwrap().contains_key(session_id) {
            pending.entry(session_id.to_string()).or_default().extend(added);
        }
    }

    fn tool_hooks(&self) -> Option<Arc<dyn ToolHooks>> {
        self.services.as_ref().and_then(|s| s.hooks.read().unwrap().clone())
    }

    /// `PreToolUse` (hooks.md sections 1 and 4): after the schema check and before
    /// `authorize`. A hook may deny the call, ask for a card (`ask`) or replace the
    /// input. A replaced input is checked and assessed again, so `authorize` and the
    /// audit see what really runs.
    pub(crate) async fn pre_tool_hook(
        &self,
        session_id: &str,
        call: &mut ToolCall,
        assessment: &mut CallAssessment,
        ask: &mut Option<String>,
    ) -> Result<(), HookRefusal> {
        let Some(tool) = self.tools.get(&call.name) else { return Ok(()) };
        let Some(hooks) = self.tool_hooks() else { return Ok(()) };
        let pre = hooks.pre(session_id, call, assessment).await;
        if let Some(text) = &pre.context {
            self.add_hook_context(session_id, [("PreToolUse", text.clone())]);
        }
        match pre.verdict {
            HookVerdict::Deny => {
                return Err(HookRefusal::Denied(if pre.reason.is_empty() { "denied by a PreToolUse hook".to_string() } else { pre.reason }));
            }
            HookVerdict::Ask => {
                *ask = Some(if pre.reason.is_empty() { "it asks for your confirmation".to_string() } else { pre.reason });
            }
            HookVerdict::Allow => {}
        }
        let Some(input) = pre.input else { return Ok(()) };
        let contract = tool.contract();
        let input = normalize_input(&contract.input_schema, input);
        let issues = validate_schema(&contract.input_schema, &input);
        if !issues.is_empty() {
            return Err(HookRefusal::Invalid(format!(
                "invalid input for {} as replaced by a PreToolUse hook: {}. Fix the hook, or call again without it.",
                call.name,
                issues.join("; ")
            )));
        }
        call.input = input;
        *assessment = self.named(session_id, tool.assess(&call.input));
        if let Some(reason) = self.child_refusal(session_id, &call.name, assessment) {
            return Err(HookRefusal::Denied(reason));
        }
        Ok(())
    }

    /// `PostToolUse`: after the tool returned. What the hooks add waits for the next
    /// model request of the session.
    pub(crate) async fn post_tool_hook(&self, session_id: &str, call: &ToolCall, output: &str, is_error: bool, duration_ms: u64) {
        let Some(hooks) = self.tool_hooks() else { return };
        let output = truncate_str(output, TOOL_OUTPUT_CHARS);
        let added = hooks.post(session_id, call, output, is_error, duration_ms).await.map(|text| ("PostToolUse", text));
        self.add_hook_context(session_id, added);
    }
}

#[cfg(test)]
mod tests;
