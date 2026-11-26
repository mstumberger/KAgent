//! What the daemon hands to the agent (the config source and its `AgentConfig`, the
//! options of one turn, the dependencies) and what starting a turn answers.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::model::{ModelClient, ModelOptions};
use crate::permission::{AllowRule, PermissionBroker, PermissionEngine};
use crate::protocol::AgentEvent;
use crate::session::SessionStore;
use crate::tool::{PrivilegeMethod, ToolRegistry};

use super::turn::TurnHandle;

/// Options of one turn (`Agent::send_with`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnOpts {
    /// A block appended to the system prompt of every model request of the turn: the
    /// body of a skill the user named with `/<name>` (skills.md section 6), rendered
    /// by the daemon. Never stored in the session; gone with the turn.
    pub system_suffix: Option<String>,
    /// Start the turn in plan mode (plan-todo.md section 2): the session's `planning` flag
    /// is set before the turn starts, so the loop and the prompt see it from the first request.
    pub plan: bool,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model_ref: String,
    pub system_prompt: String,
    pub max_iterations: usize,
    pub options: ModelOptions,
    pub privilege: PrivilegeMethod,
    /// Model context window in tokens (0 = unknown, no trimming).
    pub context_tokens: usize,
    /// Used for the rest of the turn when the chat model is overloaded or out of quota
    /// (HTTP 429/503); empty = none.
    pub fallback_model_ref: String,
    pub fallback_context_tokens: usize,
    pub fallback_max_output: Option<u64>,
}

/// What the config source is told about the turn it builds the prompt for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnContext {
    /// The session the turn runs in.
    pub session_id: String,
    /// The message that starts the turn: the latest user message (memory.md section 2).
    pub user_text: String,
}

/// Read at the start of every turn. It gets the turn's context, so a source can look at
/// the message that started it: the daemon recalls memories by its words.
pub type ConfigSource = Arc<dyn Fn(TurnContext) -> AgentConfig + Send + Sync>;
/// Read before every model request (compaction.md section 1): `settings.compaction` and the fast model.
pub type CompactionSource = Arc<dyn Fn() -> crate::compact::CompactionConfig + Send + Sync>;
pub type RuleSink = Arc<dyn Fn(Vec<AllowRule>) + Send + Sync>;

#[derive(Debug, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SendOutcome {
    #[serde(rename_all = "camelCase")]
    Started {
        turn_id: String,
        /// Resolves once the turn has ended (`turn.rs`).
        #[serde(skip_serializing)]
        handle: TurnHandle,
    },
    /// A turn is running; the input will be injected at the next model request.
    Queued,
}

pub struct AgentDeps {
    /// Change journal + root helper for mutating tools (`None` in tests).
    pub services: Option<Arc<crate::tool::ToolServices>>,
    /// The daemon's hooks of the loop (hooks.md section 4); `None`: no hook runs.
    pub hooks: Option<Arc<dyn super::hooks::AgentHooks>>,
    pub audit: Option<Arc<crate::audit::AuditLog>>,
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<ToolRegistry>,
    pub permissions: Arc<PermissionEngine>,
    pub broker: Arc<PermissionBroker>,
    pub store: SessionStore,
    pub events: broadcast::Sender<AgentEvent>,
    pub config: ConfigSource,
    pub on_rules_changed: Option<RuleSink>,
}
