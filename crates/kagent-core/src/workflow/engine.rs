//! The engine of workflow runs (workflows.md section 3): the runs of this daemon, their lifecycle (start, stop, pause,
//! continue, the owner's approval and decisions) and what the other files of this folder share: reading and changing a run
//! (always saved, then announced). The driver of a run is `drive.rs`, a task's loop `task.rs`, a worker's turn `worker.rs`,
//! questions and the mailbox `mailbox.rs`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::definition::{Definition, Role};
use super::model::{title_of, Run, RunState, Task};
use super::store::RunStore;
use crate::agent::children::Profile;
use crate::agent::Agent;
use crate::protocol::AgentEvent;
use crate::session::SessionOrigin;

/// The profile a name stands for (`None`: unknown).
pub type ProfileLookup = Arc<dyn Fn(&str) -> Option<Profile> + Send + Sync>;
/// The definition a workflow name stands for.
pub type DefinitionLookup = Arc<dyn Fn(&str) -> Result<Definition, String> + Send + Sync>;
/// The diff of a task's files in a project, as text (the reviewer's view); runs on a blocking thread.
pub type Differ = Arc<dyn Fn(&str, &[String]) -> String + Send + Sync>;

pub const MAX_GOAL_CHARS: usize = 8_000;
/// Rejected plans before the run stops.
pub const MAX_REJECTIONS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    Invalid(String),
    NotFound(String),
    Conflict(String),
    Failed(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::Invalid(m) | Self::NotFound(m) | Self::Conflict(m) | Self::Failed(m)) = self;
        f.write_str(m)
    }
}

impl std::error::Error for EngineError {}

pub(super) struct Cell {
    pub run: Mutex<Run>,
    pub wake: Notify,
    pub cancel: Mutex<CancellationToken>,
    pub driving: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Active runs at once (`workflows.maxRuns`).
    pub max_runs: usize,
    /// How long the Brain has to answer a worker's question before it goes to the owner.
    pub answer_wait: Duration,
    /// How long a blocking `ask` waits, and how long a message waits for a running worker.
    pub ask_wait: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { max_runs: 2, answer_wait: Duration::from_secs(120), ask_wait: Duration::from_secs(30 * 60) }
    }
}

pub struct StartRequest {
    pub workflow: String,
    pub project: String,
    pub goal: String,
    /// The conversation that starts it (the Brain that answers questions).
    pub origin_session: Option<String>,
    /// A ticket key to hang the run's tasks under (tickets.md section 9).
    pub ticket: Option<String>,
    /// A model ref per role word that replaces the definition's for this run.
    pub models: HashMap<String, String>,
}

pub struct Engine {
    pub(super) agent: Arc<Agent>,
    pub(super) store: RunStore,
    pub(super) cells: Mutex<HashMap<String, Arc<Cell>>>,
    /// Worker session -> (run id, task id; empty for the planner).
    pub(super) workers: Mutex<HashMap<String, (String, String)>>,
    pub(super) profiles: ProfileLookup,
    pub(super) definitions: DefinitionLookup,
    pub(super) differ: Mutex<Differ>,
    pub(super) cfg: Mutex<EngineConfig>,
}

impl Engine {
    pub fn new(agent: Arc<Agent>, dir: impl Into<PathBuf>, profiles: ProfileLookup, definitions: DefinitionLookup) -> Arc<Self> {
        Arc::new(Self {
            agent,
            store: RunStore::new(dir),
            cells: Mutex::new(HashMap::new()),
            workers: Mutex::new(HashMap::new()),
            profiles,
            definitions,
            differ: Mutex::new(Arc::new(super::git::diff)),
            cfg: Mutex::new(EngineConfig::default()),
        })
    }

    pub fn set_config(&self, cfg: EngineConfig) {
        *self.cfg.lock().unwrap() = cfg;
    }

    pub fn config(&self) -> EngineConfig {
        self.cfg.lock().unwrap().clone()
    }

    /// The definition of a workflow by name.
    pub fn definition(&self, name: &str) -> Result<Definition, String> {
        (self.definitions)(name)
    }

    pub fn set_differ(&self, differ: Differ) {
        *self.differ.lock().unwrap() = differ;
    }

    pub fn agent(&self) -> &Arc<Agent> {
        &self.agent
    }

    pub(super) fn cell(&self, id: &str) -> Result<Arc<Cell>, EngineError> {
        self.cells.lock().unwrap().get(id).cloned().ok_or_else(|| EngineError::NotFound(format!("no run '{id}'")))
    }

    pub fn get(&self, id: &str) -> Option<Run> {
        self.cell(id).ok().map(|c| c.run.lock().unwrap().clone())
    }

    /// Every run, newest first; with `project`, only that folder's.
    pub fn list(&self, project: Option<&str>) -> Vec<Run> {
        let mut runs: Vec<Run> = self.cells.lock().unwrap().values().map(|c| c.run.lock().unwrap().clone()).filter(|r| project.is_none_or(|p| r.project == p)).collect();
        runs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        runs
    }

    /// Change a run, save it, announce it. The closure runs under the run's lock.
    pub(super) fn modify<R>(&self, id: &str, f: impl FnOnce(&mut Run) -> R) -> Result<R, EngineError> {
        self.modify_if(id, None, f).map(|r| r.expect("no token, so it applies"))
    }

    /// As `modify`, but nothing happens (`Ok(None)`) when `cancel` was cancelled or the run ended: the check is made under
    /// the run's lock, so a stop that came first is never overwritten by a worker that finished a moment later.
    pub(super) fn modify_if<R>(&self, id: &str, cancel: Option<&CancellationToken>, f: impl FnOnce(&mut Run) -> R) -> Result<Option<R>, EngineError> {
        let cell = self.cell(id)?;
        let (out, snapshot) = {
            let mut run = cell.run.lock().unwrap();
            if cancel.is_some() && (cancel.is_some_and(|c| c.is_cancelled()) || run.state.is_final()) {
                return Ok(None);
            }
            let out = f(&mut run);
            run.updated_at = Utc::now();
            if let Err(e) = self.store.save(&run) {
                tracing::warn!(run = %run.id, "the run could not be saved: {e}");
            }
            (out, run.clone())
        };
        self.announce(&snapshot);
        Ok(Some(out))
    }

    pub(super) fn announce(&self, run: &Run) {
        let (done, total) = run.counts();
        self.agent.emit(AgentEvent::RunUpdated { run_id: run.id.clone(), project: run.project.clone(), state: state_word(run.state), done, total });
    }

    /// Set a task's fields; false when the run was stopped meanwhile.
    pub(super) fn set_task(&self, id: &str, task: &str, cancel: &CancellationToken, f: impl FnOnce(&mut Task)) -> bool {
        self.modify_if(id, Some(cancel), |run| run.task_mut(task).map(f).is_some()).ok().flatten().unwrap_or(false)
    }

    /// Load the runs of the folder at daemon start (workflows.md section 3.5): a run that was planning or working comes back
    /// paused, never continuing by itself. How many runs were read.
    pub fn recover(self: &Arc<Self>) -> usize {
        let (runs, _) = self.store.load_all();
        let n = runs.len();
        for mut run in runs {
            if matches!(run.state, RunState::Planning | RunState::Working) {
                run.state = RunState::Paused;
                run.reason = "the daemon restarted".into();
                run.updated_at = Utc::now();
                if let Err(e) = self.store.save(&run) {
                    tracing::warn!(run = %run.id, "the run could not be saved: {e}");
                }
            }
            self.insert_cell(run);
        }
        n
    }

    fn insert_cell(&self, run: Run) -> Arc<Cell> {
        let cell = Arc::new(Cell { run: Mutex::new(run.clone()), wake: Notify::new(), cancel: Mutex::new(CancellationToken::new()), driving: AtomicBool::new(false) });
        self.cells.lock().unwrap().insert(run.id.clone(), cell.clone());
        cell
    }

    /// Start a run (workflows.md section 3).
    pub async fn start_run(self: &Arc<Self>, req: StartRequest) -> Result<Run, EngineError> {
        let mut def = (self.definitions)(&req.workflow).map_err(|e| EngineError::Invalid(format!("workflow '{}': {e}", req.workflow)))?;
        for (role, model) in &req.models {
            let step = def.steps.iter_mut().find(|s| s.role.word() == role).ok_or_else(|| EngineError::Invalid(format!("models.{role}: the workflow has no {role} step")))?;
            if step.worker.is_none() {
                return Err(EngineError::Invalid(format!("models.{role}: the {role} step has no worker")));
            }
            step.model = (!model.trim().is_empty()).then(|| model.trim().to_string());
        }
        let profiles = self.profiles.clone();
        def.check(&|p| profiles(p).is_some()).map_err(EngineError::Invalid)?;
        let goal = req.goal.trim().to_string();
        if goal.is_empty() || goal.chars().count() > MAX_GOAL_CHARS {
            return Err(EngineError::Invalid(format!("goal: 1 to {MAX_GOAL_CHARS} characters")));
        }
        let project = std::fs::canonicalize(req.project.trim()).ok().filter(|p| p.is_dir()).ok_or_else(|| EngineError::Invalid(format!("project: '{}' is not a folder", req.project)))?.to_string_lossy().to_string();
        let max = self.config().max_runs;
        if self.list(None).iter().filter(|r| r.state.is_active()).count() >= max {
            return Err(EngineError::Conflict(format!("{max} runs are active already (workflows.maxRuns): wait for one to end, or stop one")));
        }
        let title = title_of(&goal);
        let mut meta = self.agent.store().create_with(&format!("Run: {title}"), SessionOrigin::Run, None).await.map_err(|e| EngineError::Failed(e.to_string()))?;
        meta.project = Some(project.clone());
        self.agent.store().save_meta(&meta).await.map_err(|e| EngineError::Failed(e.to_string()))?;
        let now = Utc::now();
        let planned = def.step(Role::Planner).is_some();
        let mut run = Run {
            id: format!("run_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            workflow: def.name.clone(),
            definition: def,
            project,
            goal: goal.clone(),
            title,
            state: if planned { RunState::Planning } else { RunState::Working },
            reason: String::new(),
            origin_session: req.origin_session.filter(|s| !s.is_empty()),
            run_session: meta.id,
            ticket: req.ticket.filter(|t| !t.trim().is_empty()),
            plan_summary: String::new(),
            tasks: Vec::new(),
            questions: Vec::new(),
            messages: Vec::new(),
            planner_session: None,
            feedback: None,
            rejections: 0,
            report: String::new(),
            created_at: now,
            updated_at: now,
        };
        if !planned {
            let plan = super::plan::single(&goal);
            run.plan_summary = plan.summary.clone();
            run.tasks = plan.tasks;
        }
        self.store.save(&run).map_err(|e| EngineError::Failed(format!("the run cannot be saved: {e}")))?;
        let out = run.clone();
        self.insert_cell(run);
        self.announce(&out);
        self.spawn_driver(&out.id);
        Ok(out)
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    MoreRound,
    Accept,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskEdit {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub fn state_word(state: RunState) -> String {
    serde_json::to_value(state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}
