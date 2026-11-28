//! A worker's turn (workflows.md section 3): start a subagent for a role, or continue one that ended, wait for it, and say
//! how it ended. The run's cancellation ends it.

use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::definition::Role;
use super::engine::Engine;
use super::model::WorkerRecord;
use crate::agent::children::{cut, ChildRequest, ChildStatus, MAX_DESCRIPTION_CHARS};

/// Why a worker gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Stop {
    /// The run was stopped or paused: nothing is recorded.
    Interrupted,
    Failed(String),
}

pub(super) struct Outcome {
    pub session: String,
    pub text: String,
    /// Files its calls changed.
    pub changed: Vec<String>,
}

/// What to say to the worker: a new one gets `fresh`; with `resume` (a session of a worker that ended, and the text to go on
/// with) the same session goes on, and when that is not possible a new one starts with `fresh`.
pub(super) struct Prompt {
    pub fresh: String,
    pub resume: Option<(String, String)>,
}

impl Engine {
    /// Run one worker of `run` for `role` (of `task`, empty for the planner) and wait for its answer.
    pub(super) async fn run_worker(self: &Arc<Self>, run_id: &str, task: &str, role: Role, what: &str, prompt: Prompt, round: u32, cancel: &CancellationToken) -> Result<Outcome, Stop> {
        let run = self.get(run_id).ok_or_else(|| Stop::Failed("the run is gone".into()))?;
        let step = run.definition.step(role).ok_or_else(|| Stop::Failed(format!("the workflow has no {} step", role.word())))?;
        let worker = step.worker.as_ref().ok_or_else(|| Stop::Failed(format!("the {} step has no worker", role.word())))?;
        let mut profile = (self.profiles)(&worker.profile).ok_or_else(|| Stop::Failed(format!("no profile '{}'", worker.profile)))?;
        // A planner and a reviewer cannot change anything, whatever their profile says (workflows.md section 3.1).
        if matches!(role, Role::Planner | Role::Reviewer) {
            profile.read_only = true;
        }
        let model = step.model.clone();
        if cancel.is_cancelled() {
            return Err(Stop::Interrupted);
        }
        let description = cut(what, MAX_DESCRIPTION_CHARS).to_string();
        let mut resumed = None;
        if let Some((session, text)) = &prompt.resume {
            if self.agent.resume_child(&run.run_session, session, text).await.is_ok() {
                resumed = Some(session.clone());
            }
        }
        let session = match resumed {
            Some(s) => s,
            None => {
                let req = ChildRequest { parent: run.run_session.clone(), description, prompt: prompt.fresh.clone(), profile, model, worker: true };
                match self.agent.start_child(req).await {
                    Ok(view) => view.id,
                    Err(e) => return Err(Stop::Failed(format!("the {} could not start: {e:#}", role.word()))),
                }
            }
        };
        self.workers.lock().unwrap().insert(session.clone(), (run_id.to_string(), task.to_string()));
        let record = WorkerRecord { role: role.word().into(), session: session.clone(), round, started_at: Utc::now(), ended_at: None };
        let known = self
            .modify_if(run_id, Some(cancel), |r| {
                if role == Role::Planner {
                    r.planner_session = Some(session.clone());
                } else if let Some(t) = r.task_mut(task) {
                    if !t.workers.iter().any(|w| w.session == session) {
                        t.workers.push(record);
                    }
                }
            })
            .map_err(|e| Stop::Failed(e.to_string()))?;
        if known.is_none() {
            let _ = self.agent.cancel_child(&run.run_session, &session).await;
            return Err(Stop::Interrupted);
        }
        // Messages that waited for this worker go in now.
        self.flush_messages(run_id, task);
        let Some((_, _, mut rx)) = self.agent.child_of(&run.run_session, &session) else { return Err(Stop::Failed("the worker is gone".into())) };
        let end = tokio::select! {
            r = wait_end(&mut rx) => match r {
                Some(end) => end,
                None => return Err(Stop::Failed("the worker is gone".into())),
            },
            _ = cancel.cancelled() => {
                let _ = self.agent.cancel_child(&run.run_session, &session).await;
                return Err(Stop::Interrupted);
            }
        };
        let changed: Vec<String> = self.agent.child_of(&run.run_session, &session).map(|(v, _, _)| v.files.into_iter().filter(|f| f.changed).map(|f| f.path).collect()).unwrap_or_default();
        let _ = self.modify_if(run_id, Some(cancel), |r| {
            if let Some(w) = r.task_mut(task).and_then(|t| t.workers.iter_mut().find(|w| w.session == session)) {
                w.ended_at = Some(Utc::now());
            }
        });
        match end.status {
            ChildStatus::Done if end.output.trim().is_empty() => Err(Stop::Failed(format!("the {} gave no answer", role.word()))),
            ChildStatus::Done => Ok(Outcome { session, text: end.output, changed }),
            ChildStatus::Failed => Err(Stop::Failed(end.error.filter(|e| !e.is_empty()).or_else(|| Some(cut(end.output.trim(), 300).to_string()).filter(|o| !o.is_empty())).unwrap_or_else(|| "the worker failed".into()))),
            _ if cancel.is_cancelled() => Err(Stop::Interrupted),
            _ => Err(Stop::Failed(format!("the {} was stopped before it answered", role.word()))),
        }
    }
}

/// Wait until the child has ended. A function of its own: the guard of `wait_for` must not live in the caller's future.
async fn wait_end(rx: &mut tokio::sync::watch::Receiver<Option<crate::agent::children::End>>) -> Option<crate::agent::children::End> {
    loop {
        if let Some(end) = rx.borrow_and_update().clone() {
            return Some(end);
        }
        rx.changed().await.ok()?;
    }
}
