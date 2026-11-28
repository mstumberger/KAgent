//! The rig of the workflow tests: a model scripted per job (by the description of the worker's job), the engine on a real
//! agent and session store in a temporary folder, and helpers that wait for what a run does.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use kagent_core::agent::children::{ChildStatus, Profile};
use kagent_core::agent::{Agent, AgentDeps};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{AgentEvent, Role, ToolCall};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;
use kagent_core::tools::agents::AgentAccess;
use kagent_core::workflow::{roles, shipped, Definition, Engine, Run, RunState, StartRequest, Task, TaskState};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_util::sync::CancellationToken;

use super::common::{self, ProbeTool, Spans};

#[derive(Clone)]
pub enum Step {
    Say(ModelResponse),
    /// Wait until released (or the turn is cancelled), then answer.
    Gate(Arc<Notify>, ModelResponse),
    Hold,
    Fail(String),
}

pub fn say(text: &str) -> Step {
    Step::Say(ModelResponse { text: text.into(), ..Default::default() })
}

pub fn tool(name: &str, input: Value) -> ModelResponse {
    ModelResponse { tool_calls: vec![ToolCall { id: format!("call_{name}"), name: name.into(), input }], ..Default::default() }
}

pub fn gate(release: &Arc<Notify>, text: &str) -> Step {
    Step::Gate(release.clone(), ModelResponse { text: text.into(), ..Default::default() })
}

pub fn gate_tool(release: &Arc<Notify>, name: &str) -> Step {
    Step::Gate(release.clone(), tool(name, json!({})))
}

pub fn call(name: &str) -> Step {
    Step::Say(tool(name, json!({})))
}

pub fn fail(why: &str) -> Step {
    Step::Fail(why.into())
}

/// A plan block. `tasks`: (id, depends_on, files).
pub fn plan(tasks: &[(&str, &[&str], &[&str])]) -> Step {
    let items: Vec<Value> = tasks
        .iter()
        .map(|(id, deps, files)| json!({"id": id, "title": format!("Title {id}"), "description": format!("do {id}"), "done_when": [format!("{id} works")], "depends_on": deps, "files": files}))
        .collect();
    say(&format!("Here is the plan.\n```json\n{}\n```", json!({"summary": "the summary", "tasks": items})))
}

pub fn pass() -> Step {
    say("Looks good.\n```json\n{\"verdict\": \"pass\", \"findings\": []}\n```")
}

/// `changes` with one `must` and one `should` finding.
pub fn changes(what: &str) -> Step {
    say(&format!("```json\n{}\n```", json!({"verdict": "changes", "findings": [{"severity": "must", "file": "a.rs", "line": 3, "what": what, "why": "because"}, {"severity": "should", "file": "b.rs", "what": "a nit"}]})))
}

#[derive(Debug, Clone)]
pub struct Seen {
    pub tag: String,
    pub system: String,
    pub users: Vec<String>,
    pub n_messages: usize,
    pub tools: Vec<String>,
}

#[derive(Clone, Default)]
pub struct RoleModel {
    queue: Arc<Mutex<HashMap<String, VecDeque<Step>>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn tag_of(request: &ModelRequest) -> String {
    request.system.split_once("You are a subagent. ").and_then(|(_, rest)| rest.split_once(". End with a complete final answer")).map_or_else(|| "parent".to_string(), |(tag, _)| tag.to_string())
}

impl RoleModel {
    pub fn script(&self, tag: &str, steps: Vec<Step>) {
        self.queue.lock().unwrap().insert(tag.to_string(), steps.into());
    }

    pub fn of(&self, tag: &str) -> Vec<Seen> {
        self.seen.lock().unwrap().iter().filter(|s| s.tag == tag).cloned().collect()
    }

    pub fn tags(&self) -> Vec<String> {
        let mut t: Vec<String> = self.seen.lock().unwrap().iter().map(|s| s.tag.clone()).collect();
        t.dedup();
        t
    }

    pub fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl ModelClient for RoleModel {
    async fn complete(&self, request: ModelRequest, deltas: mpsc::UnboundedSender<ModelDelta>, cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        let tag = tag_of(&request);
        self.seen.lock().unwrap().push(Seen {
            tag: tag.clone(),
            system: request.system.clone(),
            users: request.messages.iter().filter(|m| m.role == Role::User).map(|m| m.content.clone()).collect(),
            n_messages: request.messages.len(),
            tools: request.tools.iter().map(|t| t.name.clone()).collect(),
        });
        let step = self.queue.lock().unwrap().get_mut(&tag).and_then(|s| s.pop_front());
        let _ = deltas.send(ModelDelta::Text(format!("request of {tag}")));
        match step {
            Some(Step::Say(r)) => Ok(r),
            Some(Step::Gate(release, r)) => tokio::select! {
                _ = release.notified() => Ok(r),
                _ = cancel.cancelled() => Err(anyhow::anyhow!("cancelled")),
            },
            Some(Step::Hold) => {
                cancel.cancelled().await;
                Err(anyhow::anyhow!("cancelled"))
            }
            Some(Step::Fail(why)) => Err(anyhow::anyhow!(why)),
            None => Ok(ModelResponse { text: format!("no step left for {tag}"), ..Default::default() }),
        }
    }
}

pub struct Wf {
    pub engine: Arc<Engine>,
    pub agent: Arc<Agent>,
    pub model: RoleModel,
    pub store: SessionStore,
    pub dir: tempfile::TempDir,
    pub project: std::path::PathBuf,
    pub defs: Arc<Mutex<HashMap<String, String>>>,
    pub events: Arc<Mutex<Vec<AgentEvent>>>,
    pub spans: Arc<Mutex<Spans>>,
}

pub fn lookup_profile(name: &str) -> Option<Profile> {
    roles::profile(name).or_else(|| Profile::builtin(name))
}

pub fn definitions(extra: Arc<Mutex<HashMap<String, String>>>) -> kagent_core::workflow::DefinitionLookup {
    Arc::new(move |name: &str| {
        if let Some(text) = extra.lock().unwrap().get(name) {
            return Definition::parse(text, name, &|p| lookup_profile(p).is_some());
        }
        shipped::all(&|p| lookup_profile(p).is_some()).into_iter().find(|d| d.name == name).ok_or_else(|| format!("no workflow '{name}'"))
    })
}

pub async fn wf() -> Wf {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let (etx, erx) = broadcast::channel(8192);
    let spans = Arc::new(Mutex::new(Spans::default()));
    let access = AgentAccess::default();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    access.register(&mut tools);
    let model = RoleModel::default();
    let mut cfg = common::config();
    cfg.system_prompt = "BASE PROMPT.".into();
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Full, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    access.attach(&agent);
    let defs = Arc::new(Mutex::new(HashMap::new()));
    let engine = Engine::new(agent.clone(), dir.path().join("runs"), Arc::new(lookup_profile), definitions(defs.clone()));
    // The reviewer's diff is not under test here: git is not needed.
    engine.set_differ(Arc::new(|_, files| format!("diff of {}", files.join(","))));
    let events = Arc::new(Mutex::new(Vec::new()));
    {
        let (log, mut rx) = (events.clone(), erx);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => log.lock().unwrap().push(ev),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    Wf { engine, agent, model, store, dir, project, defs, events, spans }
}

impl Wf {
    pub async fn start(&self, workflow: &str, goal: &str) -> Run {
        self.start_with(workflow, goal, None).await
    }

    pub async fn start_with(&self, workflow: &str, goal: &str, origin: Option<&str>) -> Run {
        self.engine
            .start_run(StartRequest { workflow: workflow.into(), project: self.project.to_string_lossy().to_string(), goal: goal.into(), origin_session: origin.map(str::to_string), ticket: None, models: HashMap::new() })
            .await
            .unwrap()
    }

    pub fn run(&self, id: &str) -> Run {
        self.engine.get(id).unwrap()
    }

    pub fn task(&self, id: &str, task: &str) -> Task {
        self.run(id).task(task).unwrap().clone()
    }

    pub async fn until_state(&self, id: &str, state: RunState) -> Run {
        until(10, &format!("run {id} is {state:?} (now {:?}: {})", self.run(id).state, self.run(id).reason), || self.run(id).state == state).await;
        self.run(id)
    }

    pub async fn until_task(&self, id: &str, task: &str, state: TaskState) -> Task {
        until(10, &format!("task {task} is {state:?} (now {:?})", self.run(id).task(task).map(|t| t.state)), || self.run(id).task(task).is_some_and(|t| t.state == state)).await;
        self.task(id, task)
    }

    /// The workers of the run's session, in the order they started.
    pub fn workers(&self, id: &str) -> Vec<kagent_core::agent::children::ChildView> {
        self.agent.children_of(&self.run(id).run_session)
    }

    pub fn running_workers(&self, id: &str) -> usize {
        self.workers(id).iter().filter(|c| c.status == ChildStatus::Running).count()
    }

    pub fn session_of(&self, id: &str, task: &str, role: &str) -> String {
        self.run(id).task(task).unwrap().workers.iter().rev().find(|w| w.role == role).unwrap().session.clone()
    }

    pub fn updates(&self, id: &str) -> usize {
        self.events.lock().unwrap().iter().filter(|e| matches!(e, AgentEvent::RunUpdated { run_id, .. } if run_id == id)).count()
    }

    pub fn notices(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().filter_map(|e| if let AgentEvent::Notice { text, .. } = e { Some(text.clone()) } else { None }).collect()
    }
}

pub async fn until(secs: u64, what: &str, mut pred: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !pred() {
        assert!(Instant::now() < end, "deadline: {what}");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}
