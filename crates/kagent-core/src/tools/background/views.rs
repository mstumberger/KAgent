//! What the API and the UI see of the background tasks (work-windows.md sections 2 and 4.1):
//! a task with its command and start time, the list of a session, and the sink that is told
//! when a status changes.

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{Background, Inner, State, Task, TaskStatus};

/// At most this many tasks in the list of a session.
pub const LIST_MAX: usize = 50;

/// Told (task id, session id, status) where a status changes: `running` at the start,
/// `killed` on a stop, and the final `done`, `failed` or `killed` at the end.
pub type TaskEventSink = Arc<dyn Fn(&str, &str, TaskStatus) + Send + Sync>;

/// A task as `GET /api/sessions/:id/background` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundTaskView {
    pub id: String,
    pub session_id: String,
    pub command: String,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    pub started_at: DateTime<Utc>,
    /// Seconds from the start to now, or to the end of an ended task.
    pub seconds: u64,
    pub output_file: String,
}

impl Task {
    pub(super) fn view(&self, state: &State) -> BackgroundTaskView {
        let until = state.ended.unwrap_or_else(Instant::now);
        BackgroundTaskView {
            id: self.id.clone(),
            session_id: self.session.clone(),
            command: self.command.clone(),
            status: state.status,
            exit_code: state.exit_code,
            started_at: self.started_at,
            seconds: until.saturating_duration_since(state.started).as_secs(),
            output_file: self.file.display().to_string(),
        }
    }
}

impl Inner {
    /// Tell the sink, when there is one, that `task` is now `status`.
    pub(super) fn emit(&self, task: &Task, status: TaskStatus) {
        let sink = self.events.lock().unwrap().clone();
        if let Some(sink) = sink {
            sink(&task.id, &task.session, status);
        }
    }
}

impl Background {
    /// Where the changes of status go (the daemon sends them as `backgroundTask` events).
    pub fn set_event_sink(&self, sink: TaskEventSink) {
        *self.0.events.lock().unwrap() = Some(sink);
    }

    pub(super) fn emit(&self, task: &Task, status: TaskStatus) {
        self.0.emit(task, status);
    }

    /// The tasks of `session`: those running first, in the order they started, then those
    /// that ended, the one that ended last first; at most `LIST_MAX`.
    pub fn list(&self, session: &str) -> Vec<BackgroundTaskView> {
        let ended: Vec<String> = self.0.ended.lock().unwrap().iter().cloned().collect();
        let tasks = self.0.tasks.lock().unwrap();
        let mut running: Vec<BackgroundTaskView> = Vec::new();
        for task in tasks.values().filter(|t| t.session == session) {
            let state = task.state.lock().unwrap();
            if state.status == TaskStatus::Running {
                running.push(task.view(&state));
            }
        }
        running.sort_by(|a, b| a.started_at.cmp(&b.started_at).then_with(|| a.id.cmp(&b.id)));
        // A task stopped by hand is ended before its supervisor has queued it: it comes first, newest start first.
        let mut stopped: Vec<BackgroundTaskView> = tasks
            .values()
            .filter(|t| t.session == session && !ended.contains(&t.id))
            .filter_map(|t| {
                let state = t.state.lock().unwrap();
                (state.status != TaskStatus::Running).then(|| t.view(&state))
            })
            .collect();
        stopped.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        let mut out = running;
        out.extend(stopped);
        for id in ended.iter().rev() {
            if let Some(task) = tasks.get(id).filter(|t| t.session == session) {
                out.push(task.view(&task.state.lock().unwrap()));
            }
        }
        out.truncate(LIST_MAX);
        out
    }

    /// What a task is now, as the API shows it.
    pub fn view(&self, id: &str) -> Option<BackgroundTaskView> {
        let task = self.0.tasks.lock().unwrap().get(id).cloned()?;
        let view = task.view(&task.state.lock().unwrap());
        Some(view)
    }

    /// A task and the last `tail` characters of its output.
    pub async fn output_view(&self, id: &str, tail: usize) -> Option<(BackgroundTaskView, String)> {
        let task = self.0.tasks.lock().unwrap().get(id).cloned()?;
        let view = task.view(&task.state.lock().unwrap());
        Some((view, task.tail(tail).await))
    }
}
