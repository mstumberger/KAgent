//! Tickets (tickets.md): a project's list of work, kept per project in `<data>/tickets/<id>.json`. The types
//! and limits are here; `store.rs` is the files, `ops.rs` the changes and their history, `http.rs` the routes,
//! `tool.rs` the tool of the agent.

pub mod http;
mod ops;
mod store;
pub mod tool;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_tool;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chrono::{DateTime, Utc};
use kagent_core::agent::Agent;
use kagent_core::protocol::AgentEvent;
use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::broadcast;

pub use ops::{Create, Filter, Update};

pub const MAX_TITLE: usize = 200;
pub const MAX_DESCRIPTION: usize = 20_000;
pub const MAX_TICKETS: usize = 500;
pub const MAX_LABELS: usize = 20;
pub const MAX_LABEL: usize = 40;
pub const MAX_DONE_WHEN: usize = 20;
pub const MAX_DONE_WHEN_LINE: usize = 300;
pub const MAX_LINKS: usize = 50;
pub const MAX_LINK_VALUE: usize = 500;
pub const MAX_NOTES: usize = 200;
pub const MAX_NOTE: usize = 2_000;
pub const MAX_HISTORY: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Done,
    Dropped,
}

impl State {
    pub const ALL: [State; 6] = [Self::Backlog, Self::Todo, Self::InProgress, Self::InReview, Self::Done, Self::Dropped];

    pub fn word(self) -> &'static str {
        match self {
            Self::Backlog => "backlog",
            Self::Todo => "todo",
            Self::InProgress => "in_progress",
            Self::InReview => "in_review",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    None,
    Low,
    Medium,
    High,
    Urgent,
}

impl Priority {
    pub fn word(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Conversation,
    Commit,
    Range,
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Link {
    pub kind: LinkKind,
    pub value: String,
    #[serde(default)]
    pub label: String,
}

/// Who made a change: the owner (the API, the window, his voice) or the agent (the tool).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum By {
    Owner,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub at: DateTime<Utc>,
    pub by: By,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub at: DateTime<Utc>,
    pub by: By,
    pub what: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub key: String,
    /// The folder of the project; `None` for the Inbox.
    #[serde(default)]
    pub project: Option<String>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub state: State,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub links: Vec<Link>,
    /// `None` or `"owner"`: a workflow worker is for a later stage.
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub history: Vec<Entry>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One project in `GET /api/tickets`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub path: Option<String>,
    pub name: String,
    pub prefix: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TicketError {
    /// 400.
    Invalid(String),
    /// 404.
    NotFound(String),
    /// 409.
    Limit(String),
    /// 500.
    Failed(String),
}

impl TicketError {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m) | Self::NotFound(m) | Self::Limit(m) | Self::Failed(m) => m,
        }
    }
}

pub(crate) fn invalid(m: impl Into<String>) -> TicketError {
    TicketError::Invalid(m.into())
}

/// A field that may be given as `null` to clear it: `Some(None)` is null, `None` is absent.
pub(crate) fn nullable<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

/// The tickets of this daemon.
pub struct Tickets {
    dir: PathBuf,
    events: broadcast::Sender<AgentEvent>,
    /// Serializes every change (and the read before it).
    lock: Mutex<()>,
    agent: OnceLock<Weak<Agent>>,
}

impl Tickets {
    pub fn new(dir: impl Into<PathBuf>, events: broadcast::Sender<AgentEvent>) -> Arc<Self> {
        Arc::new(Self { dir: dir.into(), events, lock: Mutex::new(()), agent: OnceLock::new() })
    }

    /// The agent whose sessions' folders the tool reads.
    pub fn attach(&self, agent: &Arc<Agent>) {
        let _ = self.agent.set(Arc::downgrade(agent));
    }

    pub(crate) fn agent(&self) -> Option<Arc<Agent>> {
        self.agent.get().and_then(Weak::upgrade)
    }

    fn announce(&self, key: &str, project: Option<&str>, removed: bool) {
        let _ = self.events.send(AgentEvent::TicketChanged { key: key.into(), project: project.map(str::to_string), removed });
    }
}
