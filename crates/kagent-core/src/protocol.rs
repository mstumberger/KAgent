//! Provider-neutral conversation types and the event stream the UI consumes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

/// Where a message came from. Voice transcripts are kept in the same history so the
/// text agent and the live model share one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    #[default]
    Text,
    Voice,
    /// A scheduled task's prompt (session origin `task`).
    Task,
    /// A Telegram chat message (S29).
    Telegram,
    /// To or from a subagent (S31).
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "is_text")]
    pub via: Via,
    /// Opaque provider data that must be replayed verbatim (e.g. Anthropic thinking
    /// signatures). Never shown to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<Value>,
}

fn is_false(v: &bool) -> bool {
    !*v
}
fn is_text(v: &Via) -> bool {
    *v == Via::Text
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self::base(Role::User, content.into())
    }
    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self { tool_calls, ..Self::base(Role::Assistant, content.into()) }
    }
    pub fn tool_result(call: &ToolCall, content: impl Into<String>, is_error: bool) -> Self {
        Self {
            tool_call_id: Some(call.id.clone()),
            tool_name: Some(call.name.clone()),
            is_error,
            ..Self::base(Role::Tool, content.into())
        }
    }
    pub fn with_via(mut self, via: Via) -> Self {
        self.via = via;
        self
    }
    fn base(role: Role, content: String) -> Self {
        Self {
            role,
            content,
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
            is_error: false,
            via: Via::Text,
            provider_state: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnStatus {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolCallStatus {
    Ok,
    Error,
    Denied,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Everything the UI needs to render a session live. One broadcast stream for all
/// sessions; clients filter by `session_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    #[serde(rename_all = "camelCase")]
    TurnStarted { session_id: String, turn_id: String, input: String, via: Via },
    #[serde(rename_all = "camelCase")]
    TextDelta { session_id: String, turn_id: String, text: String },
    #[serde(rename_all = "camelCase")]
    ReasoningDelta { session_id: String, turn_id: String, text: String },
    #[serde(rename_all = "camelCase")]
    MessageAppended { session_id: String, message: Message },
    #[serde(rename_all = "camelCase")]
    ToolCallStarted {
        session_id: String,
        turn_id: String,
        call_id: String,
        name: String,
        input: Value,
        summary: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolCallFinished {
        session_id: String,
        turn_id: String,
        call_id: String,
        name: String,
        status: ToolCallStatus,
        output: String,
        duration_ms: u64,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalRequested(crate::permission::ApprovalRequest),
    #[serde(rename_all = "camelCase")]
    ApprovalResolved { request_id: String, decision: String },
    #[serde(rename_all = "camelCase")]
    TurnFinished {
        session_id: String,
        turn_id: String,
        status: TurnStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        usage: Usage,
    },
    #[serde(rename_all = "camelCase")]
    SessionUpdated { session_id: String, title: String },
    /// The todo list of a session was replaced (plan-todo.md section 1): `items` is the
    /// whole list, empty when it was cleared.
    #[serde(rename_all = "camelCase")]
    TodoUpdated { session_id: String, items: Vec<crate::todo::TodoItem> },
    /// The session entered (`active`) or left plan mode (plan-todo.md section 2).
    #[serde(rename_all = "camelCase")]
    PlanUpdated { session_id: String, active: bool },
    #[serde(rename_all = "camelCase")]
    SettingsChanged { settings: serde_json::Value },
    /// A change was journaled or its status changed (undo).
    ChangeRecorded { change: serde_json::Value },
    AlertRaised { alert: serde_json::Value },
    AlertUpdated { alert: serde_json::Value },
    #[serde(rename_all = "camelCase")]
    LiveStatus { session_id: String, status: String, detail: String },
    #[serde(rename_all = "camelCase")]
    Notice { level: String, text: String },
    /// A command for the open web UI (from the `ui` tool or the tray).
    UiCommand { command: String, args: serde_json::Value },
    /// The skills library was rescanned: a skill added, removed or edited, or
    /// `skills.disabled` changed. `count` is the number of enabled skills.
    SkillsChanged { count: usize },
    /// A memory file was written, changed, reviewed or forgotten (memory.md section 7);
    /// `removed` when its file was deleted.
    MemoryChanged { name: String, removed: bool },
    /// The task list was saved: a task added, changed, claimed or finished
    /// (automations.md section 3).
    TasksChanged,
    /// A run of a task was recorded; `run` is a `TaskRun` (api.md section 6).
    #[serde(rename_all = "camelCase")]
    TaskRun { task_id: String, run: serde_json::Value },
    /// The Telegram bot changed state (bots.md section 6): `status` is `off`,
    /// `disabled`, `unpaired`, `polling` or `error`; `detail` says more.
    #[serde(rename_all = "camelCase")]
    TelegramStatus { status: String, detail: String },
    /// A subagent of `session_id` changed state (agents.md section 3): `child` is a
    /// `ChildView` (id, title, profile, startedAt) and `status` one of `queued`, `running`,
    /// `done`, `failed`, `cancelled`.
    #[serde(rename_all = "camelCase")]
    SubagentUpdated { session_id: String, child: crate::agent::children::ChildView, status: crate::agent::children::ChildStatus },
}
