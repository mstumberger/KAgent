//! Tool contracts (port of ZCode `contracts/src/tools/contract.ts`), the `Tool` trait
//! and the registry.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// What a tool can touch. Drives scheduling and permission decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SideEffect {
    None,
    Workspace,
    System,
    Kernel,
    Network,
    UserInteraction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolContract {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub side_effect: SideEffect,
    pub read_only: bool,
    pub destructive: bool,
    pub concurrency_safe: bool,
    pub needs_approval: bool,
    /// Ask in every permission mode, even `trusted`; never satisfied by a stored rule.
    pub always_ask: bool,
    pub privileged: bool,
    pub risk: Risk,
    pub default_timeout_ms: u64,
    pub max_timeout_ms: u64,
    pub max_output_bytes: usize,
}

impl ToolContract {
    /// Read-only, unprivileged, parallel-safe defaults. Builders below tighten them.
    pub fn read_only(name: &str, description: &str, input_schema: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            side_effect: SideEffect::None,
            read_only: true,
            destructive: false,
            concurrency_safe: true,
            needs_approval: false,
            always_ask: false,
            privileged: false,
            risk: Risk::Low,
            default_timeout_ms: 15_000,
            max_timeout_ms: 60_000,
            max_output_bytes: 24_000,
        }
    }

    pub fn mutating(mut self, side_effect: SideEffect, risk: Risk) -> Self {
        self.side_effect = side_effect;
        self.read_only = false;
        self.concurrency_safe = false;
        self.needs_approval = true;
        self.risk = risk;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn privileged(mut self) -> Self {
        self.privileged = true;
        self.always_ask = true;
        self
    }

    pub fn timeout(mut self, default_ms: u64, max_ms: u64) -> Self {
        self.default_timeout_ms = default_ms;
        self.max_timeout_ms = max_ms;
        self
    }
}

/// Capability resolved for one concrete call. Many tools are read-only for some inputs
/// (`systemd status`) and mutating for others (`systemd restart`), so the permission
/// engine and the scheduler look at this, not only at the static contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallAssessment {
    pub read_only: bool,
    pub needs_approval: bool,
    pub always_ask: bool,
    pub privileged: bool,
    pub concurrency_safe: bool,
    pub risk: Risk,
    /// Human readable one-liner shown on tool and approval cards.
    pub summary: String,
    /// Key an "always allow" rule matches against, e.g. `systemctl restart nginx`.
    pub rule_key: String,
    /// Potentially destructive (reboot, kill, delete...): always asks, in every mode,
    /// never covered by a remembered rule; by voice it needs an explicit confirmation.
    #[serde(default)]
    pub destructive: bool,
}

impl CallAssessment {
    pub fn from_contract(contract: &ToolContract, summary: String, rule_key: String) -> Self {
        Self {
            read_only: contract.read_only,
            needs_approval: contract.needs_approval,
            always_ask: contract.always_ask,
            privileged: contract.privileged,
            concurrency_safe: contract.concurrency_safe,
            risk: contract.risk,
            summary,
            rule_key,
            destructive: contract.destructive,
        }
    }

    /// Mark this call as potentially destructive (see `CallAssessment::destructive`).
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self.read_only = false;
        self.needs_approval = true;
        self
    }

    /// Mark this call as a read-only variant of a mutating tool.
    pub fn as_read_only(mut self) -> Self {
        self.read_only = true;
        self.needs_approval = false;
        self.always_ask = false;
        self.privileged = false;
        self.concurrency_safe = true;
        self.risk = Risk::Low;
        self.destructive = false;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PrivilegeMethod {
    #[default]
    Sudo,
    Pkexec,
    None,
}

/// Runtime context handed to every tool call.
#[derive(Clone)]
pub struct ToolContext {
    pub cancel: CancellationToken,
    pub timeout: Duration,
    pub privilege: PrivilegeMethod,
    pub max_output_bytes: usize,
    pub session_id: String,
    pub call_id: String,
    /// Daemon services (change journal, root helper). `None` in unit tests.
    pub services: Option<Arc<ToolServices>>,
}

/// Shared services tools may use for side effects that must be journaled.
pub struct ToolServices {
    pub journal: Arc<crate::journal::ChangeJournal>,
    pub priv_client: Arc<crate::tools::privop::PrivClient>,
    /// settings.desktop, updated when settings change.
    pub desktop: std::sync::RwLock<DesktopPrefs>,
    /// The daemon's hooks of the tool calls (hooks.md section 4); `None`: no hook runs.
    pub hooks: std::sync::RwLock<Option<Arc<dyn crate::agent::hooks::ToolHooks>>>,
    /// The daemon's redactor of every tool output (token-guard.md): it takes the API token
    /// out; `None`: outputs pass as they are.
    pub redact: std::sync::RwLock<Option<Arc<dyn Fn(&str) -> String + Send + Sync>>>,
    /// Undo of changes that only the daemon can reverse (a calendar event): `undo_change`
    /// asks the handler of the journal record's `action`.
    pub undo: std::sync::RwLock<Vec<Arc<dyn UndoHandler>>>,
}

/// Reverses a journaled change of one kind (`{"kind":"user","action":"<action>",...}`).
#[async_trait]
pub trait UndoHandler: Send + Sync {
    /// The `action` this handler reverses.
    fn action(&self) -> &'static str;
    /// Reverse it: whether it worked, and what to tell the owner.
    async fn undo(&self, undo: &Value) -> (bool, String);
}

impl ToolServices {
    pub fn new(journal: Arc<crate::journal::ChangeJournal>, priv_client: Arc<crate::tools::privop::PrivClient>) -> Self {
        Self { journal, priv_client, desktop: Default::default(), hooks: Default::default(), redact: Default::default(), undo: Default::default() }
    }
}

/// User preferences for desktop tools (settings.json `desktop`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesktopPrefs {
    /// Preferred editor id (`vscode`, `kate`, `idea`, ...); empty = auto.
    pub editor: String,
    /// Folders whose children are projects; empty = built-in defaults.
    pub project_roots: Vec<String>,
}

impl ToolContext {
    /// Context for direct tool runs (tests, examples, `/api/system`).
    pub fn standalone(timeout: Duration, max_output_bytes: usize) -> Self {
        Self {
            cancel: CancellationToken::new(),
            timeout,
            privilege: PrivilegeMethod::None,
            max_output_bytes,
            session_id: String::new(),
            call_id: String::new(),
            services: None,
        }
    }

    pub fn desktop_prefs(&self) -> DesktopPrefs {
        self.services.as_ref().map(|s| s.desktop.read().unwrap().clone()).unwrap_or_default()
    }

    pub fn services(&self) -> anyhow::Result<&ToolServices> {
        self.services.as_deref().ok_or_else(|| anyhow::anyhow!("no daemon services in this context"))
    }
}

/// What kind of result a tool gave (agent-loop.md B.1): the loop and the UI tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResultKind {
    #[default]
    Ok,
    Error,
    /// Nothing changed and the model must act differently (refused, queued, asked).
    NotDone,
    /// Only displayed to the owner; nothing was changed.
    Shown,
    /// Part of the job is done.
    Partial,
}

#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    pub kind: ResultKind,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, kind: ResultKind::Ok }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, kind: ResultKind::Error }
    }
    pub fn json(value: &Value) -> Self {
        Self::ok(serde_json::to_string_pretty(value).unwrap_or_default())
    }
    /// Nothing was done, and the model is told what to do instead: `not done: <what>. Next: <next>.`
    pub fn not_done(what: &str, next: &str) -> Self {
        Self { content: format!("not done: {}. Next: {}.", sentence(what), sentence(next)), is_error: true, kind: ResultKind::NotDone }
    }
    /// Only shown to the owner (a page, a window, a notification): `shown to the owner: <what>. Nothing was changed.`
    pub fn shown(what: &str) -> Self {
        Self { content: format!("shown to the owner: {}. Nothing was changed.", sentence(what)), is_error: false, kind: ResultKind::Shown }
    }
    /// Part of the job is done: `partly done: <done>. Missing: <missing>. Next: <next>.`
    pub fn partial(done: &str, missing: &str, next: &str) -> Self {
        Self {
            content: format!("partly done: {}. Missing: {}. Next: {}.", sentence(done), sentence(missing), sentence(next)),
            is_error: false,
            kind: ResultKind::Partial,
        }
    }
}

/// `text` without surrounding space and without a trailing period (the helpers add one).
fn sentence(text: &str) -> &str {
    text.trim().trim_end_matches('.')
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn contract(&self) -> &ToolContract;

    /// Resolve the per-call capability. Default: the static contract.
    fn assess(&self, input: &Value) -> CallAssessment {
        let c = self.contract();
        let summary = format!("{} {}", c.name, compact_json(input, 120));
        CallAssessment::from_contract(c, summary, c.name.clone())
    }

    /// Look up what the card or the spoken question has to say, before `assess` (which is
    /// not async and cannot read from the network). Called before every `assess` of a call
    /// that runs, and by the voice path before it asks. Default: nothing to look up.
    async fn prime(&self, _input: &Value) {}

    /// Dry-run text shown on the approval card before the user decides (apt simulation,
    /// old -> new value, file diff). Only called when the call needs approval.
    async fn preview(&self, _input: &Value, _ctx: &ToolContext) -> Option<String> {
        None
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput>;
}

pub fn compact_json(value: &Value, max: usize) -> String {
    let s = value.to_string();
    if s.len() <= max {
        s
    } else {
        format!("{}...", crate::util::truncate_str(&s, max))
    }
}

/// Tool specification as sent to models (name, description, JSON schema).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.contract().name.clone();
        assert!(!self.tools.contains_key(&name), "duplicate tool {name}");
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn contracts(&self) -> Vec<ToolContract> {
        self.tools.values().map(|t| t.contract().clone()).collect()
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|t| {
                let c = t.contract();
                ToolSpec {
                    name: c.name.clone(),
                    description: c.description.clone(),
                    input_schema: c.input_schema.clone(),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_have_a_fixed_wording_and_a_kind() {
        let n = ToolOutput::not_done("the setting", "ask the owner");
        assert_eq!((n.content.as_str(), n.is_error, n.kind), ("not done: the setting. Next: ask the owner.", true, ResultKind::NotDone));
        let s = ToolOutput::shown("the settings page");
        assert_eq!((s.content.as_str(), s.is_error, s.kind), ("shown to the owner: the settings page. Nothing was changed.", false, ResultKind::Shown));
        let p = ToolOutput::partial("3 of 5 files", "2 files are read-only", "ask the owner");
        assert_eq!(
            (p.content.as_str(), p.is_error, p.kind),
            ("partly done: 3 of 5 files. Missing: 2 files are read-only. Next: ask the owner.", false, ResultKind::Partial)
        );
        // A trailing period is not doubled.
        assert_eq!(ToolOutput::not_done("it failed.", "retry. ").content, "not done: it failed. Next: retry.");
        assert_eq!((ToolOutput::ok("x").kind, ToolOutput::error("x").kind), (ResultKind::Ok, ResultKind::Error));
    }
}
