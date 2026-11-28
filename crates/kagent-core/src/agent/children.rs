//! Subagents (agents.md sections 2 and 3): a second agent loop with its own session,
//! system prompt, model and tools, started by the main agent for one job. This file holds
//! the orchestration: what a child session may do (its tool groups, read-only, no
//! subagents of its own), starting children under the concurrency limit, telling the
//! parent how a child ended, and the path a background task's completion takes too.
//!
//! The tools themselves are `tools::agents`; the profiles are the daemon's
//! (`kagentd/src/subagents`) and reach the tools as `Profile` records. The file holds
//! the types and the text of a job; `registry` is what a session may do and the children
//! of this run; `run` is the orchestration on `Agent`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::protocol::{Message, Via};
use crate::session::{SessionMeta, SessionOrigin};

mod registry;
mod run;

pub(crate) use registry::{Children, Restriction};

/// The built-in profiles (agents.md section 1).
pub const GENERAL: &str = "general";
pub const EXPLORE: &str = "explore";
/// At most this many characters in the description of a job (tools.md section 21).
pub const MAX_DESCRIPTION_CHARS: usize = 80;
/// At most this many characters in the prompt of a job.
pub const MAX_PROMPT_CHARS: usize = 16_000;
/// The output a notification carries (agents.md section 3); `agent_wait` answers more.
pub const MAX_OUTPUT_CHARS: usize = 4_000;
pub const WAIT_OUTPUT_CHARS: usize = 6_000;
/// The digest of the parent the job is preceded by (agents.md section 2).
pub const DIGEST_CHARS: usize = 2_000;
/// How many subagents run at once unless `subagents.maxConcurrent` says otherwise
/// (`config.md` section 11: 1..=8, default 4).
pub const DEFAULT_MAX_CHILDREN: usize = 4;
/// `agent_run` with `background: false` waits this long for the answer (tools.md section 21).
pub const RUN_WAIT: Duration = Duration::from_secs(120);
/// Notifications in a row that may start a turn of an idle parent without a message of
/// the owner in between. A child that fails at once (no chat model) would otherwise wake
/// the parent again for every attempt.
const MAX_AUTO_TURNS: usize = 5;
/// How a subagent is doing (`subagentUpdated`, api.md section 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl ChildStatus {
    pub fn ended(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The child an event or a tool result names (api.md `subagentUpdated.child`, with the
/// profile and the start time so the card needs no second request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildView {
    /// The child's session id.
    pub id: String,
    /// The description of the job, and the title of its session.
    pub title: String,
    pub profile: String,
    pub status: ChildStatus,
    /// Set once the child's turn started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

impl ChildView {
    /// Seconds since the child's turn started (0 before and after).
    pub fn running_for(&self) -> u64 {
        match (&self.started_at, &self.ended_at) {
            (Some(start), Some(end)) => (*end - *start).num_seconds().max(0) as u64,
            (Some(start), None) => (Utc::now() - start).num_seconds().max(0) as u64,
            _ => 0,
        }
    }
}

/// How a child ended: what the parent is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct End {
    pub status: ChildStatus,
    pub seconds: u64,
    /// The child's final answer (its partial text when the turn failed); may be empty.
    pub output: String,
    /// Why a failed turn failed.
    pub error: Option<String>,
}

/// A profile (agents.md section 1): what a child may do and how it is prompted. The
/// daemon reads the files of `<config>/agents` into these; `general` and `explore` are
/// built in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub description: String,
    /// A model ref; empty = the chat model.
    pub model: String,
    /// Empty = every enabled group.
    pub tool_groups: Vec<String>,
    /// 1..=80 (the daemon checks); the loop takes at least 1.
    pub max_iterations: usize,
    /// Appended to the child's system prompt; may be empty.
    pub prompt: String,
    /// `explore`: every call that is not read-only is refused.
    pub read_only: bool,
}

pub const DEFAULT_MAX_ITERATIONS: usize = 25;

impl Profile {
    pub fn general() -> Self {
        Self {
            name: GENERAL.into(),
            description: "Every tool the main agent has.".into(),
            model: String::new(),
            tool_groups: Vec::new(),
            max_iterations: DEFAULT_MAX_ITERATIONS,
            prompt: String::new(),
            read_only: false,
        }
    }

    pub fn explore() -> Self {
        Self { name: EXPLORE.into(), description: "Read-only research: nothing it does may change the machine.".into(), read_only: true, ..Self::general() }
    }

    pub fn builtin(name: &str) -> Option<Self> {
        match name {
            GENERAL => Some(Self::general()),
            EXPLORE => Some(Self::explore()),
            _ => None,
        }
    }
}

/// What a model is (the window and the output limit of a profile's model), by ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub context_tokens: usize,
    pub max_output_tokens: Option<u64>,
    /// The efforts the model offers, in order (`effective.reasoningLevels`); empty: none.
    pub reasoning_levels: Vec<String>,
}

pub type ModelWindow = Arc<dyn Fn(&str) -> Option<ModelInfo> + Send + Sync>;
/// What a conversation's choice of model is asked of (picker.md section 4.3): `Ok` for a
/// model that can be used, `Err(reason)` otherwise.
pub type UsableModels = Arc<dyn Fn(&str) -> Result<ModelInfo, &'static str> + Send + Sync>;

/// One job for a subagent.
#[derive(Debug, Clone)]
pub struct ChildRequest {
    /// The session that starts the child.
    pub parent: String,
    /// At most `MAX_DESCRIPTION_CHARS` characters.
    pub description: String,
    pub prompt: String,
    pub profile: Profile,
    /// The model the call named; it wins over the profile's.
    pub model: Option<String>,
}

/// The block of a child's system prompt: the sentence of agents.md section 2, what it may
/// do, and the profile's own prompt.
pub fn subagent_block(description: &str, profile: &Profile) -> String {
    let description = description.trim().trim_end_matches('.');
    let mut block = format!("\nYou are a subagent. {description}. End with a complete final answer: it is returned to the agent that started you.\n");
    if profile.read_only {
        block.push_str("You are read-only: every call that would change something is refused. Research, then answer.\n");
    } else if !profile.tool_groups.is_empty() {
        block.push_str(&format!("Your tool groups are only: {}. Tools outside them are refused.\n", profile.tool_groups.join(", ")));
    }
    if !profile.prompt.trim().is_empty() {
        block.push('\n');
        block.push_str(profile.prompt.trim());
        block.push('\n');
    }
    block
}

/// The message that starts the child: the parent's digest (so it knows what the
/// conversation is about) and the job.
pub fn job_text(digest: &str, prompt: &str) -> String {
    let mut text = String::new();
    if !digest.trim().is_empty() {
        text.push_str("What the conversation you were started from is about (a digest):\n");
        text.push_str(digest.trim());
        text.push_str("\n\n");
    }
    text.push_str("Your job:\n");
    text.push_str(prompt.trim());
    text
}

/// The first `limit` characters.
pub fn cut(text: &str, limit: usize) -> &str {
    text.char_indices().nth(limit).map(|(i, _)| &text[..i]).unwrap_or(text)
}

/// What a tool or a notification says about one child: `[Agent "<title>" <status> ...]`,
/// its id and profile, and the output when it ended (`limit` characters).
pub fn render(child: &ChildView, end: Option<&End>, limit: usize) -> String {
    let mut out = match end {
        Some(end) => format!("[Agent \"{}\" {} in {} s]\n", child.title, end.status.label(), end.seconds),
        None if child.status == ChildStatus::Queued => format!("[Agent \"{}\" queued]\n", child.title),
        None => format!("[Agent \"{}\" running for {} s]\n", child.title, child.running_for()),
    };
    out.push_str(&format!("agent {}, profile {}\n", child.id, child.profile));
    if let Some(end) = end {
        if let Some(error) = &end.error {
            out.push_str(&format!("error: {error}\n"));
        }
        let text = end.output.trim();
        if text.is_empty() {
            if end.status == ChildStatus::Done {
                out.push_str("(it gave no answer)\n");
            }
        } else if text.chars().count() > limit {
            out.push_str(cut(text, limit));
            out.push_str("\n(cut: call agent_wait with its id for more)\n");
        } else {
            out.push_str(text);
            out.push('\n');
        }
    }
    out
}

/// Notifications at the end of `messages`, in a row: turns a parent ran on its own.
fn auto_turns(messages: &[Message]) -> usize {
    messages.iter().rev().filter(|m| m.is_user_turn()).take_while(|m| m.via == Via::Agent).count()
}

/// Only a session the owner is at is woken by a notification: a task, an alert
/// investigation or a Telegram chat has nobody to answer an unattended turn.
fn interactive(meta: &SessionMeta) -> bool {
    matches!(meta.origin, SessionOrigin::Chat | SessionOrigin::Voice)
}

#[cfg(test)]
mod tests;
