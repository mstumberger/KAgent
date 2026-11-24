//! Permission engine (static decision) and broker (asks a human, awaits the answer).
//!
//! Port of ZCode `core/src/permission/{service,broker}.ts` with fewer modes:
//! `read-only` denies anything mutating, `ask` asks for approval-gated calls, `trusted`
//! auto-allows unprivileged side effects. `always_ask` / privileged calls ask in every
//! mode and can never be satisfied by a stored rule.

use std::collections::HashMap;
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
    /// "Allow for this session" rules: memory only, gone when the daemon stops.
    session_rules: RwLock<Vec<AllowRule>>,
}

impl PermissionEngine {
    pub fn new(mode: PermissionMode, rules: Vec<AllowRule>) -> Self {
        Self {
            overrides: RwLock::new(Default::default()),
            mode: RwLock::new(mode),
            rules: RwLock::new(rules),
            session_rules: RwLock::new(Vec::new()),
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
    pub fn add_session_rule(&self, rule: AllowRule) {
        let mut rules = self.session_rules.write().unwrap();
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }

    pub fn set_overrides(&self, overrides: std::collections::HashMap<String, ToolOverride>) {
        *self.overrides.write().unwrap() = overrides;
    }

    pub fn evaluate(&self, tool: &str, call: &CallAssessment) -> Decision {
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
        // Destructive: asks in every mode (full control too), before overrides and rules.
        if call.destructive {
            return Decision::Ask { reason: "potentially destructive: needs your explicit confirmation".into() };
        }
        if mode == PermissionMode::Full && !call.privileged && ovr != Some(ToolOverride::Ask) {
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
            || self
                .session_rules
                .read()
                .unwrap()
                .iter()
                .any(|r| r.tool == tool && call.rule_key.starts_with(&r.prefix));
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub request_id: String,
    pub session_id: String,
    pub call_id: String,
    pub tool: String,
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

type Pending = HashMap<String, (ApprovalRequest, oneshot::Sender<ApprovalDecision>)>;

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

    pub async fn request(
        &self,
        request: ApprovalRequest,
        cancel: &CancellationToken,
    ) -> ApprovalDecision {
        let (tx, rx) = oneshot::channel();
        let id = request.request_id.clone();
        self.pending.lock().unwrap().insert(id.clone(), (request.clone(), tx));
        let _ = self.events.send(AgentEvent::ApprovalRequested(request));
        let decision = tokio::select! {
            r = rx => r.unwrap_or(ApprovalDecision::Deny { reason: Some("approval channel closed".into()) }),
            _ = cancel.cancelled() => ApprovalDecision::Deny { reason: Some("turn cancelled".into()) },
            _ = tokio::time::sleep(self.timeout) => ApprovalDecision::Deny { reason: Some("approval timed out".into()) },
        };
        self.pending.lock().unwrap().remove(&id);
        let _ = self.events.send(AgentEvent::ApprovalResolved {
            request_id: id,
            decision: decision.label().into(),
        });
        decision
    }

    /// Returns false when the request is unknown (already resolved or expired).
    pub fn resolve(&self, request_id: &str, decision: ApprovalDecision) -> bool {
        match self.pending.lock().unwrap().remove(request_id) {
            Some((_, tx)) => tx.send(decision).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(read_only: bool, needs_approval: bool, privileged: bool) -> CallAssessment {
        CallAssessment {
            read_only,
            needs_approval,
            always_ask: privileged,
            privileged,
            concurrency_safe: read_only,
            risk: Risk::Medium,
            summary: String::new(),
            rule_key: "systemctl restart nginx".into(),
            destructive: false,
        }
    }

    #[test]
    fn destructive_always_asks_even_with_full_control_and_rules() {
        let e = PermissionEngine::new(PermissionMode::Full, vec![AllowRule { tool: "system_power".into(), prefix: "".into() }]);
        let mut ovr = std::collections::HashMap::new();
        ovr.insert("system_power".to_string(), ToolOverride::Allow);
        e.set_overrides(ovr);
        let reboot = call(false, true, false).destructive();
        assert!(matches!(e.evaluate("system_power", &reboot), Decision::Ask { .. }), "full control + allow rule + allow override still ask");
        e.set_mode(PermissionMode::ReadOnly);
        assert!(matches!(e.evaluate("system_power", &reboot), Decision::Deny { .. }));
    }

    #[test]
    fn modes_and_rules() {
        let e = PermissionEngine::new(PermissionMode::Ask, vec![]);
        assert!(matches!(e.evaluate("systemd", &call(true, false, false)), Decision::Allow { .. }));
        assert!(matches!(e.evaluate("systemd", &call(false, true, false)), Decision::Ask { .. }));
        e.add_rule(AllowRule { tool: "systemd".into(), prefix: "systemctl restart".into() });
        assert!(matches!(e.evaluate("systemd", &call(false, true, false)), Decision::Allow { .. }));
        // Privileged calls ignore rules and trusted mode.
        e.set_mode(PermissionMode::Trusted);
        assert!(matches!(e.evaluate("systemd", &call(false, true, true)), Decision::Ask { .. }));
        e.set_mode(PermissionMode::ReadOnly);
        assert!(matches!(e.evaluate("systemd", &call(false, true, false)), Decision::Deny { .. }));
    }

    #[test]
    fn overrides() {
        let e = PermissionEngine::new(PermissionMode::Ask, vec![]);
        let mut o = std::collections::HashMap::new();
        o.insert("systemd".to_string(), ToolOverride::Deny);
        o.insert("journal".to_string(), ToolOverride::Ask);
        o.insert("shell".to_string(), ToolOverride::Allow);
        e.set_overrides(o);
        assert!(matches!(e.evaluate("systemd", &call(true, false, false)), Decision::Deny { .. }));
        assert!(matches!(e.evaluate("journal", &call(true, false, false)), Decision::Ask { .. }));
        assert!(matches!(e.evaluate("shell", &call(false, true, false)), Decision::Allow { .. }));
        // allow never relaxes privileged calls, and read-only mode still wins.
        assert!(matches!(e.evaluate("shell", &call(false, true, true)), Decision::Ask { .. }));
        e.set_mode(PermissionMode::ReadOnly);
        assert!(matches!(e.evaluate("shell", &call(false, true, false)), Decision::Deny { .. }));
    }

    #[tokio::test]
    async fn broker_resolves_and_cancels() {
        let (tx, _rx) = broadcast::channel(16);
        let broker = PermissionBroker::new(tx, Duration::from_secs(5));
        let req = ApprovalRequest {
            request_id: "r1".into(),
            session_id: "s".into(),
            call_id: "c".into(),
            tool: "t".into(),
            input: Value::Null,
            summary: String::new(),
            reason: String::new(),
            risk: Risk::Low,
            privileged: false,
            destructive: false,
            can_remember: true,
            rule_key: String::new(),
            preview: None,
        };
        let b2 = broker.clone();
        let waiter = tokio::spawn({
            let req = req.clone();
            async move { b2.request(req, &CancellationToken::new()).await }
        });
        while broker.pending().is_empty() {
            tokio::task::yield_now().await;
        }
        assert!(broker.resolve("r1", ApprovalDecision::AllowOnce));
        assert_eq!(waiter.await.unwrap(), ApprovalDecision::AllowOnce);

        let cancel = CancellationToken::new();
        cancel.cancel();
        let d = broker.request(ApprovalRequest { request_id: "r2".into(), ..req }, &cancel).await;
        assert!(matches!(d, ApprovalDecision::Deny { .. }));
    }

    #[test]
    fn full_control_allows_everything_but_root() {
        let e = PermissionEngine::new(PermissionMode::Full, vec![]);
        let mut always = call(false, true, false);
        always.always_ask = true;
        assert!(matches!(e.evaluate("terminal", &always), Decision::Allow { .. }));
        assert!(matches!(e.evaluate("package", &call(false, true, true)), Decision::Ask { .. }), "root still asks");
        let mut ovr = std::collections::HashMap::new();
        ovr.insert("shell".to_string(), ToolOverride::Deny);
        e.set_overrides(ovr);
        assert!(matches!(e.evaluate("shell", &call(false, true, false)), Decision::Deny { .. }), "deny overrides win");
    }
}
