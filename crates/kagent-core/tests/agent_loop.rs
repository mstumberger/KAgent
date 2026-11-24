//! Scenarios S0-S1 items 5-7: the loop's parallel scheduling, denial handling,
//! cancellation and history repair, driven by a scripted fake model.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{Agent, AgentConfig, AgentDeps, SendOutcome};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{ApprovalDecision, PermissionEngine, PermissionMode, PermissionBroker};
use kagent_core::protocol::{AgentEvent, Message, Role, ToolCallStatus};
use kagent_core::session::SessionStore;
use kagent_core::tool::{Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry};

// ------------------------------------------------------------------ fake model

#[derive(Clone)]
struct ScriptedModel {
    queue: Arc<Mutex<Vec<ModelResponse>>>,
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(responses)),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ModelClient for ScriptedModel {
    async fn complete(
        &self,
        request: ModelRequest,
        deltas: mpsc::UnboundedSender<ModelDelta>,
        _cancel: CancellationToken,
    ) -> anyhow::Result<ModelResponse> {
        self.requests.lock().unwrap().push(request.messages.clone());
        let _ = deltas.send(ModelDelta::Text(request.system.clone()));
        Ok(self.queue.lock().unwrap().remove(0))
    }
}

// ------------------------------------------------------------------ test tools

#[derive(Default)]
struct Spans {
    entries: Vec<(&'static str, Instant, Instant)>,
}

struct ProbeTool {
    contract: ToolContract,
    name: &'static str,
    spans: Arc<Mutex<Spans>>,
    sleep: Duration,
}

impl ProbeTool {
    fn ro(name: &'static str, spans: &Arc<Mutex<Spans>>) -> Self {
        Self {
            contract: ToolContract::read_only(name, "probe", json!({"type": "object", "properties": {}})),
            name,
            spans: spans.clone(),
            sleep: Duration::from_millis(150),
        }
    }
    fn mut_(name: &'static str, spans: &Arc<Mutex<Spans>>) -> Self {
        Self {
            contract: ToolContract::read_only(name, "probe", json!({"type": "object", "properties": {}}))
                .mutating(SideEffect::System, Risk::Medium),
            name,
            spans: spans.clone(),
            sleep: Duration::ZERO,
        }
    }
}

#[async_trait]
impl Tool for ProbeTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let start = Instant::now();
        tokio::time::sleep(self.sleep).await;
        self.spans.lock().unwrap().entries.push((self.name, start, Instant::now()));
        Ok(ToolOutput::ok(format!("{} done", self.name)))
    }
}

// ------------------------------------------------------------------ harness

struct Rig {
    agent: Arc<Agent>,
    model: ScriptedModel,
    store: SessionStore,
    broker: Arc<PermissionBroker>,
    events: broadcast::Receiver<AgentEvent>,
    session: String,
    _dir: tempfile::TempDir,
}

fn config() -> AgentConfig {
    AgentConfig {
        model_ref: "fake::test".into(),
        system_prompt: String::new(),
        max_iterations: 8,
        options: Default::default(),
        privilege: kagent_core::tool::PrivilegeMethod::None,
        context_tokens: 0,
        fallback_model_ref: String::new(),
        fallback_context_tokens: 0,
        fallback_max_output: None,
    }
}

async fn rig(responses: Vec<ModelResponse>, spans: Arc<Mutex<Spans>>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let (etx, erx) = broadcast::channel(256);
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::ro("probe_b", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    let model = ScriptedModel::new(responses);
    let cfg = config();
    let agent = Agent::new(AgentDeps {
        services: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move || cfg.clone()),
        on_rules_changed: None,
    });
    Rig { agent: agent.clone(), model, store, broker: agent.broker().clone(), events: erx, session: meta.id, _dir: dir }
}

async fn wait_event(rx: &mut broadcast::Receiver<AgentEvent>, secs: u64, pred: impl Fn(&AgentEvent) -> bool) -> AgentEvent {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let ev = tokio::time::timeout(timeout, rx.recv()).await.expect("event deadline").expect("channel open");
        if pred(&ev) {
            return ev;
        }
    }
}

fn call(id: &str, name: &str) -> kagent_core::protocol::ToolCall {
    kagent_core::protocol::ToolCall { id: id.into(), name: name.into(), input: json!({}) }
}

// ------------------------------------------------------------------ scenarios

#[tokio::test]
async fn parallel_reads_then_denied_mutation() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let second = ModelResponse { text: "all done".into(), ..Default::default() };
    let first = ModelResponse {
        tool_calls: vec![call("1", "probe_a"), call("2", "probe_b"), call("3", "probe_c")],
        ..Default::default()
    };
    let mut r = rig(vec![first, second], spans.clone()).await;

    // Deny the approval as soon as it is asked.
    let broker = r.broker.clone();
    let mut erx = r.events.resubscribe();
    tokio::spawn(async move {
        let ev = wait_event(&mut erx, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await;
        if let AgentEvent::ApprovalRequested(req) = ev {
            broker.resolve(&req.request_id, ApprovalDecision::Deny { reason: None });
        }
    });

    let outcome = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    assert!(matches!(outcome, SendOutcome::Started { .. }));
    let fin = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    let AgentEvent::TurnFinished { status, .. } = fin else { panic!() };
    assert_eq!(status, kagent_core::protocol::TurnStatus::Completed);

    // Two read-only probes overlapped (ran in parallel).
    let s = spans.lock().unwrap();
    let a = s.entries.iter().find(|e| e.0 == "probe_a").expect("probe_a ran");
    let b = s.entries.iter().find(|e| e.0 == "probe_b").expect("probe_b ran");
    assert!(b.1 < a.2 && a.1 < b.2, "read-only calls must overlap: a {:?}-{:?} b {:?}-{:?}", a.1, a.2, b.1, b.2);
    drop(s);

    // Second model request: assistant message followed by 3 tool results in call order.
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let tail = &requests[1][requests[1].len() - 4..];
    assert_eq!(tail[0].role, Role::Assistant);
    assert_eq!(tail[0].tool_calls.len(), 3);
    for (i, m) in tail[1..].iter().enumerate() {
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some(["1", "2", "3"][i]));
    }
    assert!(tail[3].is_error);
    assert_eq!(tail[3].content, "not executed: the user denied this action");
}

#[tokio::test]
async fn cancel_during_approval_leaves_no_dangling_call() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let second = ModelResponse { text: "unreached".into(), ..Default::default() };
    let first = ModelResponse { tool_calls: vec![call("1", "probe_c")], ..Default::default() };
    let mut r = rig(vec![first, second], spans).await;

    let _ = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    let ev = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await;
    let AgentEvent::ApprovalRequested(_req) = ev else { panic!() };
    // Cancel while the approval is pending; never resolve it.
    assert!(r.agent.cancel(&r.session).await);
    let fin = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    let AgentEvent::TurnFinished { status, .. } = fin else { panic!() };
    assert_eq!(status, kagent_core::protocol::TurnStatus::Cancelled);

    let messages = r.store.load_messages(&r.session).await.unwrap();
    let assistant = messages.iter().find(|m| m.role == Role::Assistant && !m.tool_calls.is_empty()).expect("assistant with calls");
    for c in &assistant.tool_calls {
        let has_result = messages.iter().any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(&c.id));
        assert!(has_result, "call {} must have a result", c.id);
    }
    assert_eq!(r.model.requests.lock().unwrap().len(), 1, "no second model request after cancel");
}

#[tokio::test]
async fn sanitize_history_repairs_dangling_call() {
    let mut messages = vec![Message::user("hi")];
    let mut assistant = Message::assistant("working", vec![call("1", "probe_a")]);
    assistant.provider_state = None;
    messages.push(assistant);
    messages.push(Message::user("hello again"));

    let fixed = kagent_core::agent::sanitize_history(&messages);
    // user, assistant, synthetic tool result, user
    assert_eq!(fixed.len(), 4);
    assert_eq!(fixed[2].role, Role::Tool);
    assert_eq!(fixed[2].tool_call_id.as_deref(), Some("1"));
    assert!(fixed[2].is_error);

    // A clean history passes through unchanged.
    let with_result = kagent_core::agent::sanitize_history(&fixed);
    assert_eq!(with_result.len(), 4);
}

// Silence the unused warning for ToolCallStatus import used via full path.
#[allow(dead_code)]
fn _touch() -> Option<ToolCallStatus> {
    None
}

#[tokio::test]
async fn disabled_tool_group_is_refused_not_just_hidden() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let r = rig(vec![], spans.clone()).await;
    // probe_* tools map to group "other"; enabling only "system" disables them.
    r.agent.set_tool_groups(Some(vec!["system".into()]));
    let cancel = tokio_util::sync::CancellationToken::new();
    let (msg, status) = r
        .agent
        .execute_call(&r.session, "t", &call("x", "probe_a"), &cancel, kagent_core::tool::PrivilegeMethod::None)
        .await;
    assert_eq!(status, kagent_core::protocol::ToolCallStatus::Denied);
    assert!(msg.content.contains("disabled"), "{}", msg.content);
    assert!(spans.lock().unwrap().entries.is_empty(), "disabled tool must not run");
    // Re-enabling all groups lets it run.
    r.agent.set_tool_groups(None);
    let (_, status) = r
        .agent
        .execute_call(&r.session, "t", &call("y", "probe_a"), &cancel, kagent_core::tool::PrivilegeMethod::None)
        .await;
    assert_eq!(status, kagent_core::protocol::ToolCallStatus::Ok);
}
