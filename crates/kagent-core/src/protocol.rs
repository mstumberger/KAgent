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
    /// A message of the screen watch: its hit, its end (screen-watch.md section 3).
    Watch,
    /// A note of the daemon into a running turn (agent-loop.md C): stored as a message of role user, never the owner's words.
    Note,
}

/// The pixels of an image while a request is built (`agent/vision.rs`): never stored in the
/// session file and never sent to the UI.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Pixels(pub std::sync::Arc<Vec<u8>>);

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pixels({} bytes)", self.0.len())
    }
}

/// An image of a message (vision.md section 1). The pixels are a file beside the session
/// (`session/images.rs`); this is the reference to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub id: String,
    /// `image/png` or `image/jpeg`: what the store kept after re-encoding.
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// What the vision model said the image shows (vision.md section 2), made once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The model that wrote `description`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_by: Option<String>,
    /// Set on the copy that goes into one model request.
    #[serde(skip)]
    pub data: Option<Pixels>,
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
    /// The images of a user message, at most 4 (vision.md section 1); empty is left out of the file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageRef>,
}

fn is_false(v: &bool) -> bool {
    !*v
}
fn is_text(v: &Via) -> bool {
    *v == Via::Text
}

impl Message {
    /// A message of the owner or of a caller that stands in for them: it starts a turn. A note of the daemon does not
    /// (compaction cuts and the hint of the tool budget look for these).
    pub fn is_user_turn(&self) -> bool {
        self.role == Role::User && self.via != Via::Note
    }

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
    pub fn with_images(mut self, images: Vec<ImageRef>) -> Self {
        self.images = images;
        self
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
            images: Vec::new(),
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
    /// Nothing changed; the result says what to do instead (agent-loop.md B.1).
    NotDone,
    /// Only shown to the owner; nothing was changed.
    Shown,
    /// Part of the job is done.
    Partial,
}

impl ToolCallStatus {
    /// Does the model read the result as a failure (`is_error` of its tool result)? `Shown` and `Partial` are information.
    pub fn is_failure(self) -> bool {
        !matches!(self, Self::Ok | Self::Shown | Self::Partial)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// The button of a notice: the window posts `body` to `path` (an `/api/...` route of the daemon).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoticeAction {
    pub label: String,
    pub path: String,
    pub body: serde_json::Value,
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
    /// The session's meta changed (picker.md section 3): the whole meta, so a window applies it
    /// without reading anything.
    #[serde(rename_all = "camelCase")]
    SessionMetaChanged { session_id: String, meta: crate::session::SessionMeta },
    #[serde(rename_all = "camelCase")]
    SettingsChanged { settings: serde_json::Value },
    /// A change was journaled or its status changed (undo).
    ChangeRecorded { change: serde_json::Value },
    AlertRaised { alert: serde_json::Value },
    AlertUpdated { alert: serde_json::Value },
    #[serde(rename_all = "camelCase")]
    LiveStatus { session_id: String, status: String, detail: String },
    #[serde(rename_all = "camelCase")]
    Notice {
        level: String,
        text: String,
        /// What makes two notices the same one (notices.md section 1): `provider:<id>:<class>`, `hook:<name>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        /// The session whose turn the notice belongs to.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// `error`, `recovery` or `info`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// A button of the toast: a request the window makes when it is pressed (notices.md section 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<NoticeAction>,
    },
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
    /// The Google Calendar connection changed state (calendar.md section 7): `state` is
    /// `off`, `connecting`, `connected`, `reconnect` or `error`.
    CalendarChanged { state: String },
    /// The downloads of local models changed (local-models.md section 3): `pulls` is the
    /// list of `GET /api/local-models` (queued, pulling or failed rows), at most twice a second.
    LocalModelsChanged { pulls: serde_json::Value },
    /// The limits of a provider changed level, or (at most every 30 s while answers keep coming) its counters did (usage.md section 4).
    #[serde(rename_all = "camelCase")]
    UsageChanged { provider: String },
    /// The screen watch changed state, or (at most every 5 s) its `looks` and `lastNote` (screen-watch.md
    /// section 4): `watch` is the watch as `GET /api/screen-watch` shows it, with its final state when it ended.
    ScreenWatchChanged { watch: serde_json::Value },
    /// A subagent of `session_id` changed state (agents.md section 3): `child` is a
    /// `ChildView` (id, title, profile, startedAt) and `status` one of `queued`, `running`,
    /// `done`, `failed`, `cancelled`.
    #[serde(rename_all = "camelCase")]
    SubagentUpdated { session_id: String, child: crate::agent::children::ChildView, status: crate::agent::children::ChildStatus },
    /// A background shell task of `session_id` changed status (work-windows.md section 3):
    /// `running` at the start, `killed` on a stop, then the final `done`, `failed` or `killed`.
    #[serde(rename_all = "camelCase")]
    BackgroundTask { task_id: String, session_id: String, status: String },
    /// A ticket was created, changed or deleted (tickets.md section 4); `project` is the folder, `None` for the Inbox.
    #[serde(rename_all = "camelCase")]
    TicketChanged { key: String, project: Option<String>, removed: bool },
    /// A workflow run or one of its tasks changed (workflows.md section 6); a window reads the run again. `state` is the
    /// run's state word (`working`, `awaiting_approval`, ...), `done` and `total` count its tasks.
    #[serde(rename_all = "camelCase")]
    RunUpdated { run_id: String, project: String, state: String, done: usize, total: usize },
}

impl AgentEvent {
    /// A notice with no key: it is never merged with another (notices.md section 1). `kind`
    /// follows the level: `error` and `warn` are `error`, `info` is `info`.
    pub fn notice(level: &str, text: impl Into<String>) -> Self {
        let kind = if level == "info" { "info" } else { "error" };
        AgentEvent::Notice { level: level.into(), text: text.into(), key: None, session_id: None, kind: Some(kind.into()), action: None }
    }

    /// The same notice, merged by `key` while its toast is open.
    pub fn keyed_notice(level: &str, text: impl Into<String>, key: impl Into<String>) -> Self {
        match Self::notice(level, text) {
            AgentEvent::Notice { level, text, session_id, kind, action, .. } => AgentEvent::Notice { level, text, key: Some(key.into()), session_id, kind, action },
            other => other,
        }
    }

    /// The toast gets a button: `label` posts `body` to `path` of the daemon's API when pressed.
    pub fn with_action(mut self, label: &str, path: &str, body: serde_json::Value) -> Self {
        if let AgentEvent::Notice { action, .. } = &mut self {
            *action = Some(NoticeAction { label: label.into(), path: path.into(), body });
        }
        self
    }

    /// The notice belongs to a turn of this session.
    pub fn in_session(mut self, id: &str) -> Self {
        if let AgentEvent::Notice { session_id, .. } = &mut self {
            *session_id = Some(id.to_string());
        }
        self
    }

    /// The thing that went wrong works again: it closes the open error toast of the same `key`.
    pub fn recovery(text: impl Into<String>, key: impl Into<String>) -> Self {
        AgentEvent::Notice { level: "info".into(), text: text.into(), key: Some(key.into()), session_id: None, kind: Some("recovery".into()), action: None }
    }
}
