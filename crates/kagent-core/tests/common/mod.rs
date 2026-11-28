//! The scripted-model rig of the loop's tests (moved unchanged from `agent_loop.rs` when
//! the compaction tests needed it too): a model that answers from a script and records
//! its requests, two read-only probes and one that needs an approval, and a ready agent.

#![allow(dead_code)] // each test binary uses a part of the rig

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{Agent, AgentConfig, AgentDeps};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{AgentEvent, Message};
use kagent_core::session::SessionStore;
use kagent_core::tool::{Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry};

// ------------------------------------------------------------------ fake model

#[derive(Clone)]
pub struct ScriptedModel {
    /// The answers in order; `Err` fails its request with that text.
    pub queue: Arc<Mutex<Vec<Result<ModelResponse, String>>>>,
    pub requests: Arc<Mutex<Vec<Vec<Message>>>>,
    /// The system prompt of every request, in order.
    pub systems: Arc<Mutex<Vec<String>>>,
    /// The model and the output limit of every request, in order.
    pub model_refs: Arc<Mutex<Vec<(String, Option<u64>)>>>,
    /// The reasoning level of every request, in order.
    pub levels: Arc<Mutex<Vec<Option<String>>>>,
    /// Who each request was made for (`CALL_SOURCE` of the turn).
    pub sources: Arc<Mutex<Vec<kagent_core::model::CallSource>>>,
}

impl ScriptedModel {
    pub fn new(responses: Vec<ModelResponse>) -> Self {
        Self::scripted(responses.into_iter().map(Ok).collect())
    }

    /// Answers from a script: a response, or the text of the error the request fails with.
    pub fn scripted(script: Vec<Result<ModelResponse, String>>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(script)),
            requests: Arc::new(Mutex::new(Vec::new())),
            systems: Arc::new(Mutex::new(Vec::new())),
            model_refs: Arc::new(Mutex::new(Vec::new())),
            levels: Arc::new(Mutex::new(Vec::new())),
            sources: Arc::new(Mutex::new(Vec::new())),
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
        self.systems.lock().unwrap().push(request.system.clone());
        self.model_refs.lock().unwrap().push((request.model_ref.clone(), request.options.max_output_tokens));
        self.levels.lock().unwrap().push(request.options.reasoning_level.clone());
        self.sources.lock().unwrap().push(kagent_core::model::current_source());
        let reply = self.queue.lock().unwrap().remove(0).map_err(anyhow::Error::msg)?;
        let _ = deltas.send(ModelDelta::Text(request.system.clone()));
        Ok(reply)
    }
}

// ------------------------------------------------------------------ test tools

#[derive(Default)]
pub struct Spans {
    pub entries: Vec<(&'static str, Instant, Instant)>,
}

pub struct ProbeTool {
    pub contract: ToolContract,
    pub name: &'static str,
    pub spans: Arc<Mutex<Spans>>,
    pub sleep: Duration,
}

impl ProbeTool {
    pub fn ro(name: &'static str, spans: &Arc<Mutex<Spans>>) -> Self {
        Self {
            contract: ToolContract::read_only(name, "probe", json!({"type": "object", "properties": {}})),
            name,
            spans: spans.clone(),
            sleep: Duration::from_millis(150),
        }
    }
    pub fn mut_(name: &'static str, spans: &Arc<Mutex<Spans>>) -> Self {
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

pub struct Rig {
    pub agent: Arc<Agent>,
    pub model: ScriptedModel,
    pub store: SessionStore,
    pub broker: Arc<PermissionBroker>,
    pub events: broadcast::Receiver<AgentEvent>,
    pub session: String,
    pub _dir: tempfile::TempDir,
}

pub fn config() -> AgentConfig {
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

pub async fn rig(responses: Vec<ModelResponse>, spans: Arc<Mutex<Spans>>) -> Rig {
    rig_with(ScriptedModel::new(responses), spans, config()).await
}

/// The rig with a model of one's own script and the config of every turn.
pub async fn rig_with(model: ScriptedModel, spans: Arc<Mutex<Spans>>, cfg: AgentConfig) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let (etx, erx) = broadcast::channel(256);
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::ro("probe_b", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    Rig { agent: agent.clone(), model, store, broker: agent.broker().clone(), events: erx, session: meta.id, _dir: dir }
}

pub async fn wait_event(rx: &mut broadcast::Receiver<AgentEvent>, secs: u64, pred: impl Fn(&AgentEvent) -> bool) -> AgentEvent {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let ev = tokio::time::timeout(timeout, rx.recv()).await.expect("event deadline").expect("channel open");
        if pred(&ev) {
            return ev;
        }
    }
}

pub fn call(id: &str, name: &str) -> kagent_core::protocol::ToolCall {
    kagent_core::protocol::ToolCall { id: id.into(), name: name.into(), input: json!({}) }
}
