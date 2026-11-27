//! Permission engine (static decision) and broker (asks a human, awaits the answer).
//!
//! Port of ZCode `core/src/permission/{service,broker}.ts` with fewer modes:
//! `read-only` denies anything mutating, `ask` asks for approval-gated calls, `trusted`
//! auto-allows unprivileged side effects. `always_ask` / privileged calls ask in every
//! mode and can never be satisfied by a stored rule.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;

use crate::protocol::AgentEvent;
use crate::tool::{CallAssessment, Risk};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    Ask,
    Trusted,
    /// Full control: every unprivileged action runs without a card, including tools that
    /// normally always ask (terminal send, messages, reboot, browser eval). Root
    /// (privileged) actions still ask. Per-tool `deny`/`ask` overrides still apply.
    Full,
}

/// "Always allow" rule: tool name + prefix of the call's rule key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowRule {
    pub tool: String,
    pub prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow { reason: String },
    Deny { reason: String },
    Ask { reason: String },
}

/// Per-tool override from settings (`toolOverrides`), applied on top of the mode.
/// It can make a tool stricter (`ask`, `deny`) or looser (`allow`), but `allow` never
/// relaxes privileged or always-ask calls, and read-only mode still wins over `allow`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolOverride {
    Allow,
    Ask,
    Deny,
}

impl ToolOverride {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

pub struct PermissionEngine {
    overrides: RwLock<std::collections::HashMap<String, ToolOverride>>,
    mode: RwLock<PermissionMode>,
    rules: RwLock<Vec<AllowRule>>,
    /// "Allow for this session" rules per session id: memory only, gone when the daemon
    /// stops, and never seen from another session.
    session_rules: RwLock<HashMap<String, Vec<AllowRule>>>,
    /// The sessions in plan mode (plan-todo.md section 2): memory only. The agent sets it
    /// with the session's meta, as it does the session rules; the plan step of `evaluate`
    /// reads it, so no mode, rule or override gets past it.
    planning: RwLock<HashSet<String>>,
}

impl PermissionEngine {
    pub fn new(mode: PermissionMode, rules: Vec<AllowRule>) -> Self {
        Self {
            overrides: RwLock::new(Default::default()),
            mode: RwLock::new(mode),
            rules: RwLock::new(rules),
            session_rules: RwLock::new(HashMap::new()),
            planning: RwLock::new(HashSet::new()),
        }
    }

    pub fn mode(&self) -> PermissionMode {
        *self.mode.read().unwrap()
    }
    pub fn set_mode(&self, mode: PermissionMode) {
        *self.mode.write().unwrap() = mode;
    }
    pub fn rules(&self) -> Vec<AllowRule> {
        self.rules.read().unwrap().clone()
    }
    pub fn set_rules(&self, rules: Vec<AllowRule>) {
        *self.rules.write().unwrap() = rules;
    }
    pub fn add_rule(&self, rule: AllowRule) {
        let mut rules = self.rules.write().unwrap();
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    /// Remember an allowance for the rest of this daemon run, in `session_id` only.
    pub fn add_session_rule(&self, session_id: &str, rule: AllowRule) {
        let mut rules = self.session_rules.write().unwrap();
        let list = rules.entry(session_id.to_string()).or_default();
        if !list.contains(&rule) {
            list.push(rule);
        }
    }

    /// Put `session_id` in or out of plan mode for `evaluate`: the agent does this with
    /// the session's `planning` flag.
    pub fn set_planning(&self, session_id: &str, planning: bool) {
        let mut set = self.planning.write().unwrap();
        if planning {
            set.insert(session_id.to_string());
        } else {
            set.remove(session_id);
        }
    }

    pub fn is_planning(&self, session_id: &str) -> bool {
        self.planning.read().unwrap().contains(session_id)
    }

    pub fn set_overrides(&self, overrides: std::collections::HashMap<String, ToolOverride>) {
        *self.overrides.write().unwrap() = overrides;
    }

    /// The decision for a call made in `session_id` (session allowances are looked up
    /// there and nowhere else), as a call of the chat.
    pub fn evaluate(&self, session_id: &str, tool: &str, call: &CallAssessment) -> Decision {
        self.evaluate_from(session_id, tool, call, Origin::Chat)
    }

    /// `evaluate` for a call of this origin. A call from voice that is `always_ask` asks in
    /// every mode, `full` included (voice-control.md section 4): `full` lets those through
    /// in chat only.
    pub fn evaluate_from(&self, session_id: &str, tool: &str, call: &CallAssessment, origin: Origin) -> Decision {
        let ovr = self.overrides.read().unwrap().get(tool).copied();
        if ovr == Some(ToolOverride::Deny) {
            return Decision::Deny { reason: format!("'{tool}' is set to deny in tool overrides") };
        }
        if call.read_only && !call.privileged {
            if ovr == Some(ToolOverride::Ask) {
                return Decision::Ask { reason: format!("'{tool}' is set to always ask") };
            }
            return Decision::Allow { reason: "read-only call".into() };
        }
        let mode = self.mode();
        if mode == PermissionMode::ReadOnly {
            return Decision::Deny {
                reason: "KAgent is in read-only mode; switch the permission mode to run this".into(),
            };
        }
        // Plan mode (plan-todo.md section 2): before anything that could allow the call.
        if let Some(reason) = crate::plan::plan_denial(self.is_planning(session_id), tool, call) {
            return Decision::Deny { reason: reason.into() };
        }
        // Destructive: asks in every mode (full control too), before overrides and rules.
        if call.destructive {
            return Decision::Ask { reason: "potentially destructive: needs your explicit confirmation".into() };
        }
        let voice_asks = origin == Origin::Voice && call.always_ask;
        if mode == PermissionMode::Full && !call.privileged && ovr != Some(ToolOverride::Ask) && !voice_asks {
            return Decision::Allow { reason: "full control".into() };
        }
        if call.always_ask || call.privileged {
            return Decision::Ask {
                reason: if call.privileged {
                    "privileged (root) action".into()
                } else {
                    "this tool always asks".into()
                },
            };
        }
        match ovr {
            Some(ToolOverride::Ask) => return Decision::Ask { reason: format!("'{tool}' is set to always ask") },
            Some(ToolOverride::Allow) => return Decision::Allow { reason: format!("'{tool}' is set to allow") },
            _ => {}
        }
        let matched = self.rules.read().unwrap().iter().any(|r| r.tool == tool && call.rule_key.starts_with(&r.prefix))
            || (!session_id.is_empty()
                && self
                    .session_rules
                    .read()
                    .unwrap()
                    .get(session_id)
                    .is_some_and(|rules| rules.iter().any(|r| r.tool == tool && call.rule_key.starts_with(&r.prefix))));
        if matched {
            return Decision::Allow { reason: "matched an always-allow rule".into() };
        }
        if mode == PermissionMode::Trusted {
            return Decision::Allow { reason: "trusted mode".into() };
        }
        if call.needs_approval {
            Decision::Ask { reason: "changes system state".into() }
        } else {
            Decision::Allow { reason: "no approval needed".into() }
        }
    }
}

/// Where a call comes from: the chat, or voice (a live tool call, or a call inside a turn
/// that was sent with `Via::Voice`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Origin {
    #[default]
    Chat,
    Voice,
}

impl Origin {
    pub fn of_via(via: crate::protocol::Via) -> Self {
        if via == crate::protocol::Via::Voice { Self::Voice } else { Self::Chat }
    }
}

/// What a card is about: a tool call, or a plan to approve (plan mode, `plan_exit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalKind {
    #[default]
    Tool,
    Plan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: String,
    pub session_id: String,
    pub call_id: String,
    pub tool: String,
    /// A tool call, or a plan to approve; a card from before S24 is a tool call.
    #[serde(default)]
    pub kind: ApprovalKind,
    pub input: Value,
    pub summary: String,
    pub reason: String,
    pub risk: Risk,
    pub privileged: bool,
    /// Potentially destructive: voice confirmation needs an explicit phrase.
    #[serde(default)]
    pub destructive: bool,
    /// Whether "always allow" may be offered (not for always-ask/privileged calls).
    pub can_remember: bool,
    pub rule_key: String,
    /// Dry-run output from `Tool::preview` (apt simulation, diff, old -> new value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "camelCase")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowSession,
    AllowAlways,
    Deny {
        #[serde(default)]
        reason: Option<String>,
    },
}

impl ApprovalDecision {
    pub fn label(&self) -> &'static str {
        match self {
            Self::AllowOnce => "allowOnce",
            Self::AllowSession => "allowSession",
            Self::AllowAlways => "allowAlways",
            Self::Deny { .. } => "deny",
        }
    }
}

/// How a card was answered: the `via` of its audit record (config.md section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalVia {
    /// The web UI or the API.
    Click,
    Voice,
    Telegram,
    /// A policy answered it, not a person (a task's `onApproval: deny`).
    Rule,
    /// Nobody answered within the approval timeout.
    Timeout,
    /// The turn was cancelled while the card waited.
    Cancel,
}

impl ApprovalVia {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Voice => "voice",
            Self::Telegram => "telegram",
            Self::Rule => "rule",
            Self::Timeout => "timeout",
            Self::Cancel => "cancel",
        }
    }
}

type Pending = HashMap<String, (ApprovalRequest, oneshot::Sender<(ApprovalDecision, ApprovalVia)>)>;

/// Holds pending approvals. The UI answers through `resolve`; a cancelled turn or a
/// timeout resolves the wait as a denial so the loop can always finish.
pub struct PermissionBroker {
    pending: Mutex<Pending>,
    events: broadcast::Sender<AgentEvent>,
    timeout: Duration,
}

impl PermissionBroker {
    pub fn new(events: broadcast::Sender<AgentEvent>, timeout: Duration) -> Arc<Self> {
        Arc::new(Self { pending: Mutex::new(HashMap::new()), events, timeout })
    }

    pub fn pending(&self) -> Vec<ApprovalRequest> {
        self.pending.lock().unwrap().values().map(|(r, _)| r.clone()).collect()
    }

    pub async fn request(&self, request: ApprovalRequest, cancel: &CancellationToken) -> ApprovalDecision {
        self.request_via(request, cancel).await.0
    }

    /// `request`, and how the card was answered.
    pub async fn request_via(&self, request: ApprovalRequest, cancel: &CancellationToken) -> (ApprovalDecision, ApprovalVia) {
        let (tx, rx) = oneshot::channel();
        let id = request.request_id.clone();
        self.pending.lock().unwrap().insert(id.clone(), (request.clone(), tx));
        let _ = self.events.send(AgentEvent::ApprovalRequested(request));
        let (decision, via) = tokio::select! {
            r = rx => r.unwrap_or((ApprovalDecision::Deny { reason: Some("approval channel closed".into()) }, ApprovalVia::Cancel)),
            _ = cancel.cancelled() => (ApprovalDecision::Deny { reason: Some("turn cancelled".into()) }, ApprovalVia::Cancel),
            _ = tokio::time::sleep(self.timeout) => (ApprovalDecision::Deny { reason: Some("approval timed out".into()) }, ApprovalVia::Timeout),
        };
        self.pending.lock().unwrap().remove(&id);
        let _ = self.events.send(AgentEvent::ApprovalResolved {
            request_id: id,
            decision: decision.label().into(),
        });
        (decision, via)
    }

    /// Answer a card, saying how (`via`). Returns false when the request is unknown
    /// (already resolved or expired).
    pub fn resolve(&self, request_id: &str, decision: ApprovalDecision, via: ApprovalVia) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some((_, tx)) => tx.send((decision, via)).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests;
