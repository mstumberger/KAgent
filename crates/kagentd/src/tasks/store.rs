//! The task record and its file (automations.md section 1): `<data>/tasks.json`, an array
//! in camelCase, written atomically. The store behind the tasks API, the `tasks` tool
//! and the runner: every change is saved at once and said with `tasksChanged`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Local};
use kagent_core::protocol::AgentEvent;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::templates;
use crate::sections::TasksSection;

#[cfg(test)]
mod tests;

mod claim;
mod record;

use record::from_stored;
pub use record::{RunStatus, SessionPolicy, Task, TaskError, TaskInput, TaskRun, What};
pub(crate) use record::chars;

/// How many runs a task remembers.
const MAX_RUNS: usize = 50;
/// The longest `result` of a run, in characters.
const RUN_CHARS: usize = 500;
/// The longest `lastResult`, in characters.
const LAST_RESULT_CHARS: usize = 2000;
/// How late a scheduled run may be and still run; a `once` has a minute.
const ON_TIME: Duration = Duration::minutes(5);
const ON_TIME_ONCE: Duration = Duration::seconds(60);

type Defaults = Arc<dyn Fn() -> TasksSection + Send + Sync>;

#[derive(Debug, Default)]
struct Inner {
    tasks: Vec<Task>,
    /// Asked to run now and not yet started (also paused tasks and to-dos).
    requested: HashSet<String>,
    /// Each running task: what cancels it, and whether it holds a slot of the limit.
    running: HashMap<String, Running>,
}

#[derive(Debug, Clone)]
struct Running {
    token: CancellationToken,
    agent: bool,
}

pub struct TaskStore {
    path: PathBuf,
    inner: Mutex<Inner>,
    events: broadcast::Sender<AgentEvent>,
    /// settings.tasks, read whenever a request leaves a policy field out.
    defaults: Mutex<Option<Defaults>>,
}

/// The records of the file as they are stored since S28. A file from before (schedules
/// `At`, `Every`, `Daily`; `runs` a count; no policy fields) is converted. A record that
/// cannot be read is left out with an error in the log; no file is an empty list.
fn read(path: &Path, defaults: &TasksSection) -> Vec<Task> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let Ok(Value::Array(records)) = serde_json::from_slice::<Value>(&bytes) else {
        tracing::error!("{}: not a task list; starting empty", path.display());
        return Vec::new();
    };
    records.into_iter().filter_map(|record| from_stored(record, defaults).map_err(|e| tracing::error!("{}: a task is not loaded: {e}", path.display())).ok()).collect()
}

impl TaskStore {
    pub fn load(data: &Path, events: broadcast::Sender<AgentEvent>, defaults: TasksSection) -> Arc<Self> {
        let path = data.join("tasks.json");
        let tasks = read(&path, &defaults);
        Arc::new(Self { path, inner: Mutex::new(Inner { tasks, ..Default::default() }), events, defaults: Mutex::new(None) })
    }

    /// Where the default of a policy field comes from: `settings.tasks`, read at each
    /// request.
    pub fn set_defaults(&self, read: Defaults) {
        *self.defaults.lock().unwrap() = Some(read);
    }

    fn defaults(&self) -> TasksSection {
        self.defaults.lock().unwrap().as_ref().map_or_else(TasksSection::default, |read| read())
    }

    /// A task as the API answers it: the record and `scheduleText`.
    pub fn view(task: &Task) -> Value {
        let mut view = serde_json::to_value(task).unwrap_or(Value::Null);
        if let Value::Object(fields) = &mut view {
            fields.insert("scheduleText".into(), json!(task.schedule_text()));
        }
        view
    }

    pub fn list(&self) -> Vec<Task> {
        self.inner.lock().unwrap().tasks.clone()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.index_of(id).ok().map(|i| self.inner.lock().unwrap().tasks[i].clone())
    }

    /// The runs of a task, oldest first.
    pub fn runs(&self, id: &str) -> Result<Vec<TaskRun>, TaskError> {
        Ok(self.index_of(id).map(|i| self.inner.lock().unwrap().tasks[i].runs.clone())?)
    }

    pub fn add(&self, input: TaskInput, now: DateTime<Local>) -> Result<Task, TaskError> {
        let task = input.into_task(&self.defaults(), now)?;
        let mut inner = self.inner.lock().unwrap();
        inner.tasks.push(task.clone());
        self.save(&inner);
        Ok(task)
    }

    /// A template of the Tasks tab: the task it describes, paused.
    pub fn add_template(&self, id: &str, now: DateTime<Local>) -> Result<Task, TaskError> {
        let template = templates::get(id).ok_or_else(|| TaskError::Invalid(format!("unknown template '{id}'")))?;
        self.add(
            TaskInput {
                title: Some(template.title.into()),
                what: Some(What::Text(template.what.into())),
                kind: Some("agent".into()),
                schedule: Some(Value::String(template.schedule.into())),
                enabled: Some(false),
                ..Default::default()
            },
            now,
        )
    }

    pub fn update(&self, id: &str, patch: Value, now: DateTime<Local>) -> Result<Task, TaskError> {
        let patch = TaskInput::parse(patch)?;
        let mut inner = self.inner.lock().unwrap();
        let i = Self::find(&inner.tasks, id)?;
        // Changed on a copy: a field that is refused leaves the task as it was, whole.
        let mut next = inner.tasks[i].clone();
        patch.apply(&mut next, now)?;
        inner.tasks[i] = next;
        let task = inner.tasks[i].clone();
        self.save(&inner);
        Ok(task)
    }

    pub fn delete(&self, id: &str) -> Result<(), TaskError> {
        let mut inner = self.inner.lock().unwrap();
        let i = Self::find(&inner.tasks, id)?;
        let removed = inner.tasks.remove(i);
        inner.requested.remove(&removed.id);
        // A run that is going ends by itself and finds nothing to record.
        inner.running.remove(&removed.id);
        self.save(&inner);
        Ok(())
    }

    /// "Run now": the next pass of the runner starts the task (also a paused task or a
    /// to-do), whatever its schedule says.
    pub fn request_run(&self, id: &str) -> Result<(), TaskError> {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.tasks[Self::find(&inner.tasks, id)?].id.clone();
        if inner.running.contains_key(&id) {
            return Err(TaskError::Busy("the task is already running".into()));
        }
        inner.requested.insert(id);
        Ok(())
    }

    /// Cancel the run of a task. True when a run was stopped, or a "run now" that had
    /// not started yet was dropped.
    pub fn cancel(&self, id: &str) -> Result<bool, TaskError> {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.tasks[Self::find(&inner.tasks, id)?].id.clone();
        if let Some(running) = inner.running.get(&id) {
            running.token.cancel();
            return Ok(true);
        }
        Ok(inner.requested.remove(&id))
    }

    /// What cancels the run of a claimed task.
    pub fn cancel_token(&self, id: &str) -> CancellationToken {
        self.inner.lock().unwrap().running.get(id).map_or_else(CancellationToken::new, |r| r.token.clone())
    }

    /// A run starts in `session`: a task with `reuse` keeps it for its next runs.
    pub fn attach_session(&self, id: &str, session: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Ok(i) = Self::find(&inner.tasks, id) {
            let task = &mut inner.tasks[i];
            if task.session == SessionPolicy::Reuse && task.session_id.as_deref() != Some(session) {
                task.session_id = Some(session.to_string());
                self.save(&inner);
            }
        }
    }

    /// Record how a run ended. `seconds` is how long it took.
    pub fn finish(&self, id: &str, status: RunStatus, result: &str, session: Option<&str>, seconds: u64, now: DateTime<Local>) -> Option<TaskRun> {
        let mut inner = self.inner.lock().unwrap();
        inner.running.remove(id);
        let i = self.index_of_locked(&inner, id).ok()?;
        let task = &mut inner.tasks[i];
        let run = TaskRun {
            at: task.runs.iter().rposition(|r| r.status == RunStatus::Running).map_or(now, |k| task.runs[k].at),
            status,
            duration_sec: Some(seconds),
            session_id: session.map(str::to_string),
            result: Some(chars(result, RUN_CHARS).to_string()),
        };
        match task.runs.iter().rposition(|r| r.status == RunStatus::Running) {
            Some(k) => task.runs[k] = run.clone(),
            None => Self::keep(task, run.clone()),
        }
        task.last_run = Some(run.at);
        task.last_status = Some(status);
        task.last_result = Some(chars(result, LAST_RESULT_CHARS).to_string());
        if let Some(session) = session {
            task.last_session = Some(session.to_string());
        }
        // A to-do is done after a run that went well.
        if task.schedule.is_none() {
            task.done = status == RunStatus::Ok;
        }
        let (id, run) = (task.id.clone(), run);
        self.save(&inner);
        drop(inner);
        self.emit_run(&id, &run);
        Some(run)
    }

    /// At daemon start: the runs a stopped daemon left `running` did not finish. How
    /// many tasks were changed.
    pub fn fail_stale(&self) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let mut stale = 0;
        for task in &mut inner.tasks {
            let mut changed = false;
            for run in &mut task.runs {
                if run.status == RunStatus::Running {
                    run.status = RunStatus::Failed;
                    run.result = Some("daemon restarted".into());
                    changed = true;
                }
            }
            if task.last_status == Some(RunStatus::Running) {
                task.last_status = Some(RunStatus::Failed);
                task.last_result = Some("daemon restarted".into());
                changed = true;
            }
            stale += usize::from(changed);
        }
        if stale > 0 {
            self.save(&inner);
        }
        stale
    }

    fn index_of(&self, id: &str) -> Result<usize, TaskError> {
        Self::find(&self.inner.lock().unwrap().tasks, id)
    }

    fn index_of_locked(&self, inner: &Inner, id: &str) -> Result<usize, TaskError> {
        Self::find(&inner.tasks, id)
    }

    /// The task an id names: exactly, or as the one id it starts (four characters at least).
    fn find(tasks: &[Task], id: &str) -> Result<usize, TaskError> {
        if let Some(i) = tasks.iter().position(|t| t.id == id) {
            return Ok(i);
        }
        let mut hits = tasks.iter().enumerate().filter(|(_, t)| id.len() >= 4 && t.id.starts_with(id));
        match (hits.next(), hits.next()) {
            (Some((i, _)), None) => Ok(i),
            _ => Err(TaskError::NotFound(id.into())),
        }
    }

    /// Add a run record and keep the last `MAX_RUNS`.
    fn keep(task: &mut Task, run: TaskRun) {
        task.runs.push(run);
        let over = task.runs.len().saturating_sub(MAX_RUNS);
        task.runs.drain(..over);
    }

    fn emit_run(&self, id: &str, run: &TaskRun) {
        let _ = self.events.send(AgentEvent::TaskRun { task_id: id.to_string(), run: serde_json::to_value(run).unwrap_or(Value::Null) });
    }

    fn save(&self, inner: &Inner) {
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(&inner.tasks).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &self.path)).is_err() {
            tracing::warn!("could not save {}", self.path.display());
        }
        let _ = self.events.send(AgentEvent::TasksChanged);
    }
}
