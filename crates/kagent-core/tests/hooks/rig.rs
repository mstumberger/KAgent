//! The rig of the hook tests of the loop, in two files: this one has the daemon's hooks as
//! a rule (what they were asked and what they answer), the tools, and the harness, on
//! `../common`; `main.rs` has the tests (hooks.md sections 1, 3 and 4; scenario S32 item 2,
//! and the rule of items 4 and 5).

use crate::common::{config, ProbeTool, Spans};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use kagent_core::agent::{Agent, AgentDeps, AgentHooks, HookEvent, HookInput, HookVerdict, SendOutcome, ToolHooks, TurnOpts, TurnResult};
use kagent_core::audit::AuditLog;
use kagent_core::journal::ChangeJournal;
use kagent_core::model::ModelResponse;
use kagent_core::permission::{AllowRule, ApprovalDecision, ApprovalRequest, PermissionBroker, PermissionEngine, PermissionMode, ToolOverride};
use kagent_core::protocol::{AgentEvent, ToolCall, TurnStatus, Via};
use kagent_core::session::SessionStore;
use kagent_core::tool::{CallAssessment, SideEffect, Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry, ToolServices};
use kagent_core::tools::{privop::PrivClient, shell::Shell};

// ------------------------------------------------------------------ the daemon's hooks, as a rule

/// What the hooks answer.
#[derive(Clone, Default)]
pub enum Rule {
    /// Nothing to say: allow, no context, the input as it is (what a failed hook counts as).
    #[default]
    Silent,
    Allow,
    Ask,
    /// Deny, with `reason`, a command that holds `word`.
    Deny { word: &'static str, reason: &'static str },
    /// Allow, with `updatedInput` in place of the model's input.
    Replace(Value),
}

/// The daemon's hooks: what they were asked, in order, and the rule they answer by.
#[derive(Default)]
pub struct Fake {
    pub log: Mutex<Vec<String>>,
    pub rule: Mutex<Rule>,
    /// How long the context of a `PreToolUse` hook is (0: a short sentence).
    pub context_chars: std::sync::atomic::AtomicUsize,
}

impl Fake {
    pub fn set(&self, rule: Rule) {
        *self.rule.lock().unwrap() = rule;
    }
    pub fn context(&self) -> String {
        let n = self.context_chars.load(std::sync::atomic::Ordering::SeqCst);
        if n == 0 { "kept a note".to_string() } else { "x".repeat(n) }
    }
}

#[async_trait]
impl ToolHooks for Fake {
    async fn pre(&self, _session_id: &str, call: &ToolCall, assessment: &CallAssessment) -> HookInput {
        self.log.lock().unwrap().push(format!("pre {} {} read_only={}", call.name, call.input, assessment.read_only));
        self.log.lock().unwrap().push(format!("assessment {}", serde_json::to_string(assessment).unwrap()));
        let allowed = HookInput { context: Some(self.context()), ..HookInput::ALLOW };
        match self.rule.lock().unwrap().clone() {
            Rule::Silent => HookInput::ALLOW,
            Rule::Allow => allowed,
            Rule::Ask => HookInput { verdict: HookVerdict::Ask, reason: "the owner wants a look at this".into(), ..HookInput::ALLOW },
            Rule::Deny { word, reason } if call.input.to_string().contains(word) => HookInput { verdict: HookVerdict::Deny, reason: reason.into(), ..HookInput::ALLOW },
            Rule::Deny { .. } => allowed,
            Rule::Replace(input) => HookInput { input: Some(input), ..HookInput::ALLOW },
        }
    }
    async fn post(&self, _session_id: &str, call: &ToolCall, output: &str, is_error: bool, duration_ms: u64) -> Option<String> {
        self.log.lock().unwrap().push(format!("post {} output={output:?} error={is_error} duration={duration_ms}", call.name));
        (!matches!(*self.rule.lock().unwrap(), Rule::Silent)).then(|| "after the call".to_string())
    }
}

#[async_trait]
impl AgentHooks for Fake {
    async fn event(&self, event: HookEvent<'_>) -> Option<String> {
        self.log.lock().unwrap().push(format!("event {}", event.name()));
        // Under `Silent` a hook says nothing at all.
        (matches!(event, HookEvent::UserPromptSubmit { .. }) && !matches!(*self.rule.lock().unwrap(), Rule::Silent)).then(|| "mind the printer".to_string())
    }
}

// ------------------------------------------------------------------ test tools

/// Answers with its input, as the call ended up handing it over.
pub struct Echo(pub ToolContract);

#[async_trait]
impl Tool for Echo {
    fn contract(&self) -> &ToolContract {
        &self.0
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::ok(input.to_string()))
    }
}

// ------------------------------------------------------------------ harness

pub struct Rig {
    pub agent: Arc<Agent>,
    pub model: crate::common::ScriptedModel,
    pub store: SessionStore,
    pub broker: Arc<PermissionBroker>,
    pub events: broadcast::Receiver<AgentEvent>,
    pub session: String,
    /// The probes that ran, in order.
    pub spans: Arc<Mutex<Spans>>,
    pub fake: Arc<Fake>,
    pub _dir: tempfile::TempDir,
}

/// A session with the probes, `echo` and `shell`, a scripted model, in `mode`, with the
/// daemon's hooks (`fake`) when `with_hooks`.
#[allow(clippy::too_many_arguments)]
pub async fn rig(mode: PermissionMode, rules: Vec<AllowRule>, overrides: &[(&str, ToolOverride)], with_hooks: bool, responses: Vec<ModelResponse>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let session = store.create("t").await.unwrap().id;
    let (etx, erx) = broadcast::channel(256);
    let spans = Arc::new(Mutex::new(Spans::default()));
    let destructive = ToolContract::read_only("probe_d", "probe", json!({"type": "object", "properties": {}})).mutating(SideEffect::System, kagent_core::tool::Risk::High).destructive();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    tools.register(Arc::new(ProbeTool { contract: destructive, name: "probe_d", spans: spans.clone(), sleep: Duration::ZERO }));
    tools.register(Arc::new(Echo(ToolContract::read_only("echo", "answers with its input", json!({"type": "object", "properties": {"text": {"type": "string"}, "n": {"type": "integer", "minimum": 0}}})))));
    tools.register(Arc::new(Shell::new()));
    let model = crate::common::ScriptedModel::new(responses);
    let cfg = config();
    let permissions = Arc::new(PermissionEngine::new(mode, rules));
    permissions.set_overrides(overrides.iter().map(|(tool, o)| (tool.to_string(), *o)).collect());
    let audit = Arc::new(AuditLog::open(&dir.path().join("audit.jsonl")).unwrap());
    let journal = Arc::new(ChangeJournal::new(dir.path().join("changes")).unwrap());
    let services = Arc::new(ToolServices::new(journal, Arc::new(PrivClient::with_path("/nonexistent/kagent-priv"))));
    let fake = Arc::new(Fake::default());
    if with_hooks {
        *services.hooks.write().unwrap() = Some(fake.clone());
    }
    let agent = Agent::new(AgentDeps {
        services: Some(services),
        hooks: with_hooks.then(|| fake.clone() as Arc<dyn AgentHooks>),
        audit: Some(audit.clone()),
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions,
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(30)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    Rig { broker: agent.broker().clone(), agent, model, store, events: erx, session, spans, fake, _dir: dir }
}

/// A model answer that calls one tool.
pub fn calls(name: &str, input: Value) -> ModelResponse {
    ModelResponse { tool_calls: vec![ToolCall { id: format!("call_{name}"), name: name.into(), input }], ..Default::default() }
}

pub fn says(text: &str) -> ModelResponse {
    ModelResponse { text: text.into(), ..Default::default() }
}

pub async fn turn(r: &Rig, text: &str, opts: TurnOpts) -> (TurnResult, Vec<AgentEvent>) {
    let mut events = r.events.resubscribe();
    let outcome = r.agent.send_with(&r.session, text.into(), Via::Text, opts).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!("nothing runs, so the input starts a turn") };
    let result = handle.wait().await;
    let mut emitted = Vec::new();
    while let Ok(ev) = events.try_recv() {
        emitted.push(ev);
    }
    (result, emitted)
}

pub fn completed(result: &TurnResult) {
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
}

pub fn types(emitted: &[AgentEvent]) -> Vec<String> {
    emitted.iter().map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_string()).collect()
}

pub async fn stored(r: &Rig) -> Vec<Value> {
    r.store.load_messages(&r.session).await.unwrap().iter().map(|m| json!([m.role, m.content, m.is_error])).collect()
}

pub async fn next_card(events: &mut broadcast::Receiver<AgentEvent>) -> ApprovalRequest {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let AgentEvent::ApprovalRequested(request) = events.recv().await.expect("event channel open") {
                break request;
            }
        }
    })
    .await
    .expect("a card was raised")
}

/// A side task that answers the next `answers.len()` cards of the session, in order; the cards.
pub fn answering(r: &Rig, answers: Vec<ApprovalDecision>) -> tokio::task::JoinHandle<Vec<ApprovalRequest>> {
    let mut events = r.events.resubscribe();
    let broker = r.broker.clone();
    tokio::spawn(async move {
        let mut cards = Vec::new();
        for decision in answers {
            let request = next_card(&mut events).await;
            assert!(broker.resolve(&request.request_id, decision, kagent_core::permission::ApprovalVia::Click));
            cards.push(request);
        }
        cards
    })
}

/// What the hooks were asked, without the assessments they were shown; a duration is `N`.
pub fn asked(r: &Rig) -> Vec<String> {
    r.fake.log.lock().unwrap().iter().filter(|l| !l.starts_with("assessment ")).map(|l| l.split(" duration=").next().unwrap().to_string()).collect()
}

/// What the tools answered: `(content, is_error)`, in the order the calls were made.
pub async fn tool_results(r: &Rig) -> Vec<(String, bool)> {
    r.store.load_messages(&r.session).await.unwrap().iter().filter(|m| m.tool_call_id.is_some()).map(|m| (m.content.clone(), m.is_error)).collect()
}

/// The text of the hook messages stored in the session, without their `[hook <event>]` label.
pub async fn hook_texts(r: &Rig) -> Vec<String> {
    stored(r).await.iter().filter_map(|m| m[1].as_str().and_then(|c| c.strip_prefix("[hook ")).and_then(|c| c.split_once("] ").map(|(_, text)| text.to_string()))).collect()
}

/// The records of the audit log of one kind.
pub fn audit(r: &Rig, kind: &str) -> Vec<Value> {
    std::fs::read_to_string(r._dir.path().join("audit.jsonl")).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|v| v["kind"] == kind).map(|v| v["data"].clone()).collect()
}

pub fn ran(r: &Rig) -> Vec<&'static str> {
    r.spans.lock().unwrap().entries.iter().map(|e| e.0).collect()
}

#[allow(dead_code)]
fn everything_runs(rules: Vec<AllowRule>) -> Vec<AllowRule> {
    rules
}

