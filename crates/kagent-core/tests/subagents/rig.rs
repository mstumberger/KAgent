//! The rig of the subagent tests (`tests/subagents.rs`), on the probes and the scripted
//! model of `tests/common/mod.rs`: a model scripted per job (the parent, or a subagent by
//! the description of its job) that can also hold a request until the test releases it or
//! the turn is cancelled; the probes, the four subagent tools and the task tools; and a
//! log of every event the agent emits.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use kagent_core::agent::children::ChildStatus;
use kagent_core::agent::{Agent, AgentDeps, SendOutcome, TurnResult};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{AgentEvent, Message, Role, ToolCall, ToolCallStatus, Via};
use kagent_core::session::SessionStore;
use kagent_core::tool::{PrivilegeMethod, ToolRegistry};
use kagent_core::tools::agents::AgentAccess;
use kagent_core::tools::background::Background;
use kagent_core::tools::shell::Shell;
use kagent_core::tools::tasktools::{TaskOutput, TaskStop};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_util::sync::CancellationToken;

use super::common::{self, ProbeTool, Spans};

// ------------------------------------------------------------------ the model

/// What the model does on one request of the script it was given.
#[derive(Clone)]
pub enum Step {
    Say(ModelResponse),
    /// Wait until `release` is notified, then answer.
    Gate { release: Arc<Notify>, response: ModelResponse },
    /// Block until the turn is cancelled.
    Hold,
}

/// A plain answer.
pub fn say(text: &str) -> Step {
    Step::Say(ModelResponse { text: text.into(), ..Default::default() })
}

/// One tool call.
pub fn call(name: &str, input: Value) -> Step {
    Step::Say(ModelResponse { tool_calls: vec![ToolCall { id: format!("call_{name}"), name: name.into(), input }], ..Default::default() })
}

/// `agent_run` for a job with this description (`background: true`, the `general` profile).
pub fn run(description: &str) -> Step {
    call("agent_run", json!({ "description": description, "prompt": "look around" }))
}

/// Two calls in one answer, as a model that starts two subagents at once.
pub fn pair(a: (&str, Value), b: (&str, Value)) -> Step {
    let call = |(name, input): (&str, Value), id: &str| ToolCall { id: id.into(), name: name.into(), input };
    Step::Say(ModelResponse { tool_calls: vec![call(a, "call_a"), call(b, "call_b")], ..Default::default() })
}

/// `agent_run` with a profile.
pub fn run_as(description: &str, profile: &str) -> Step {
    call("agent_run", json!({ "description": description, "prompt": "look around", "profile": profile }))
}

/// Hold the request until `release`, then answer `text`.
pub fn gated(release: &Arc<Notify>, text: &str) -> Step {
    Step::Gate { release: release.clone(), response: ModelResponse { text: text.into(), ..Default::default() } }
}

/// Block until cancelled.
pub fn hold() -> Step {
    Step::Hold
}

/// One request the model saw.
#[derive(Debug, Clone)]
pub struct Seen {
    /// `parent`, or the description of the subagent's job.
    pub tag: String,
    pub system: String,
    pub model_ref: String,
    /// The names of the tools the request offered.
    pub tools: Vec<String>,
    pub last_user: String,
}

/// The description of the job in the block of a subagent's system prompt; `parent` else.
fn tag_of(request: &ModelRequest) -> String {
    const END: &str = ". End with a complete final answer";
    request
        .system
        .split_once("You are a subagent. ")
        .and_then(|(_, rest)| rest.split_once(END))
        .map_or_else(|| "parent".to_string(), |(tag, _)| tag.to_string())
}

#[derive(Clone, Default)]
pub struct JobModel {
    queue: Arc<Mutex<HashMap<String, VecDeque<Step>>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl JobModel {
    pub fn new(scripts: &[(&str, Vec<Step>)]) -> Self {
        Self {
            queue: Arc::new(Mutex::new(scripts.iter().cloned().map(|(tag, steps)| (tag.to_string(), steps.into())).collect())),
            seen: Default::default(),
        }
    }

    /// The requests so far, in order.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// The requests of one script (`parent`, or a job's description), in order.
    pub fn of(&self, tag: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.tag == tag).collect()
    }
}

#[async_trait]
impl ModelClient for JobModel {
    async fn complete(&self, request: ModelRequest, deltas: mpsc::UnboundedSender<ModelDelta>, cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        let tag = tag_of(&request);
        let last_user = request.messages.iter().rev().find(|m| m.role == Role::User).map_or_else(String::new, |m| m.content.clone());
        self.seen.lock().unwrap().push(Seen {
            tag: tag.clone(),
            system: request.system.clone(),
            model_ref: request.model_ref.clone(),
            tools: request.tools.iter().map(|t| t.name.clone()).collect(),
            last_user,
        });
        let step = self.queue.lock().unwrap().get_mut(&tag).and_then(|steps| steps.pop_front());
        let _ = deltas.send(ModelDelta::Text(format!("request of {tag}")));
        match step {
            Some(Step::Say(response)) => Ok(response),
            Some(Step::Gate { release, response }) => tokio::select! {
                _ = release.notified() => Ok(response),
                _ = cancel.cancelled() => Err(anyhow::anyhow!("cancelled")),
            },
            Some(Step::Hold) => {
                cancel.cancelled().await;
                Err(anyhow::anyhow!("cancelled"))
            }
            None => Ok(ModelResponse { text: format!("no step left for {tag}"), ..Default::default() }),
        }
    }
}

// ------------------------------------------------------------------ the rig

pub struct Rig {
    pub agent: Arc<Agent>,
    pub model: JobModel,
    pub store: SessionStore,
    pub access: AgentAccess,
    pub background: Background,
    pub events: broadcast::Receiver<AgentEvent>,
    pub session: String,
    pub spans: Arc<Mutex<Spans>>,
    /// Every event, to look back at.
    log: Arc<Mutex<Vec<AgentEvent>>>,
    _dir: tempfile::TempDir,
}

/// A chat session with the probes (group `other`), the subagent tools, the task tools and
/// the `shell` tool over a background registry of its own, in `mode`. The system prompt is
/// `BASE PROMPT.`.
pub async fn rig(scripts: &[(&str, Vec<Step>)], mode: PermissionMode) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let session = store.create("t").await.unwrap().id;
    let (etx, erx) = broadcast::channel(4096);
    let spans = Arc::new(Mutex::new(Spans::default()));
    let access = AgentAccess::default();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::ro("probe_b", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    access.register(&mut tools);
    let background = Background::new(dir.path().join("task-out"));
    tools.register(Arc::new(TaskOutput::new(background.clone())));
    tools.register(Arc::new(TaskStop::new(background.clone())));
    tools.register(Arc::new(Shell::with_tasks(background.clone())));
    let model = JobModel::new(scripts);
    let mut cfg = common::config();
    cfg.system_prompt = "BASE PROMPT.".into();
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(mode, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    access.attach(&agent);
    // As the daemon wires it: a task that ends tells its session through the agent.
    let tell = agent.clone();
    background.configure(dir.path().join("task-out"), Arc::new(move |session, text| {
        let tell = tell.clone();
        tokio::spawn(async move {
            let _ = tell.notify(&session, &text).await;
        });
    }));
    let log = Arc::new(Mutex::new(Vec::new()));
    {
        let (log, mut rx) = (log.clone(), erx.resubscribe());
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => log.lock().unwrap().push(ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    Rig { agent, model, store, access, background, events: erx, session, spans, log, _dir: dir }
}

// ------------------------------------------------------------------ what a test reads

/// Poll until `pred` holds; panics with `what` after `secs`.
pub async fn until(secs: u64, what: &str, mut pred: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !pred() {
        assert!(Instant::now() < end, "deadline: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Send `text` as the owner and wait for the turn.
pub async fn turn(r: &Rig, text: &str) -> TurnResult {
    match r.agent.send(&r.session, text.into(), Via::Text).await.unwrap() {
        SendOutcome::Started { handle, .. } => handle.wait().await,
        SendOutcome::Queued => panic!("nothing runs, so the input starts a turn"),
    }
}

/// Start the turn and wait only until its first request is in progress.
pub async fn started(r: &Rig, text: &str) -> kagent_core::agent::TurnHandle {
    match r.agent.send(&r.session, text.into(), Via::Text).await.unwrap() {
        SendOutcome::Started { handle, .. } => handle,
        SendOutcome::Queued => panic!("nothing runs, so the input starts a turn"),
    }
}

pub fn children(r: &Rig) -> Vec<kagent_core::agent::children::ChildView> {
    r.agent.children_of(&r.session)
}

/// The children of the session, once there are `n` (the calls that start them ran).
pub async fn kids(r: &Rig, n: usize) -> Vec<kagent_core::agent::children::ChildView> {
    until(10, &format!("{n} children of the session"), || children(r).len() == n).await;
    children(r)
}

/// The states the UI was told about for one child, in order.
pub fn statuses(r: &Rig, id: &str) -> Vec<ChildStatus> {
    r.log
        .lock()
        .unwrap()
        .iter()
        .filter_map(|ev| match ev {
            AgentEvent::SubagentUpdated { child, status, .. } if child.id == id => Some(*status),
            _ => None,
        })
        .collect()
}

/// How many turns of `session` ended.
pub fn finished(r: &Rig, session: &str) -> usize {
    r.log.lock().unwrap().iter().filter(|ev| matches!(ev, AgentEvent::TurnFinished { session_id, .. } if session_id == session)).count()
}

/// How many turns of `session` started.
pub fn started_turns(r: &Rig, session: &str) -> usize {
    r.log.lock().unwrap().iter().filter(|ev| matches!(ev, AgentEvent::TurnStarted { session_id, .. } if session_id == session)).count()
}

/// Wait until the UI was told that `id` is `status`.
pub async fn wait_status(r: &Rig, id: &str, status: ChildStatus) {
    until(10, &format!("subagent {id} is {status:?}"), || statuses(r, id).contains(&status)).await;
}

pub async fn wait_turns(r: &Rig, session: &str, n: usize) {
    until(10, &format!("{n} turns of {session} ended"), || finished(r, session) >= n).await;
}

pub async fn messages(r: &Rig, session: &str) -> Vec<Message> {
    r.store.load_messages(session).await.unwrap()
}

/// The results of the calls of `tool`, in order.
pub async fn results(r: &Rig, session: &str, tool: &str) -> Vec<Message> {
    messages(r, session).await.into_iter().filter(|m| m.role == Role::Tool && m.tool_name.as_deref() == Some(tool)).collect()
}

/// The notifications stored in `session` (role user, `Via::Agent`).
pub async fn notes(r: &Rig, session: &str) -> Vec<String> {
    messages(r, session).await.into_iter().filter(|m| m.role == Role::User && m.via == Via::Agent).map(|m| m.content).collect()
}

/// Run one call in `session` through the full path, as the loop would.
pub async fn run_tool(r: &Rig, session: &str, name: &str, input: Value) -> (String, ToolCallStatus) {
    let call = ToolCall { id: format!("test_{name}"), name: name.into(), input };
    let (msg, status) = r.agent.execute_call(session, "test", &call, &CancellationToken::new(), PrivilegeMethod::None).await;
    (msg.content, status)
}
