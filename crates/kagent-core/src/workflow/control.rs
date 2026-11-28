//! What the owner and the daemon do to a run once it exists (workflows.md sections 3.3 and 3.4): stop, pause, continue, the
//! plan's approval and rejection, the decision about a task, the keys of its tickets.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::engine::{Decision, Engine, EngineError, TaskEdit, MAX_REJECTIONS};
use super::model::{PlanDoc, Run, RunState, Task, TaskState};

impl Engine {
    /// Stop a run for good: workers are cancelled, tasks that were not finished are `stopped`.
    pub async fn stop_run(&self, id: &str) -> Result<Run, EngineError> {
        let cell = self.cell(id)?;
        if cell.run.lock().unwrap().state.is_final() {
            return Err(EngineError::Conflict("the run has ended already".into()));
        }
        cell.cancel.lock().unwrap().cancel();
        self.modify(id, |run| {
            run.state = RunState::Stopped;
            run.reason = "stopped by the owner".into();
            for t in run.tasks.iter_mut().filter(|t| matches!(t.state, TaskState::Waiting | TaskState::Implementing | TaskState::InReview | TaskState::Changes | TaskState::NeedsOwner)) {
                t.state = TaskState::Stopped;
                t.ended_at = Some(Utc::now());
            }
        })?;
        self.cancel_workers(id).await;
        cell.wake.notify_one();
        Ok(self.get(id).expect("the run exists"))
    }

    /// Cancel every worker of the run that is still running.
    pub(super) async fn cancel_workers(&self, id: &str) {
        let Some(run) = self.get(id) else { return };
        let sessions: Vec<String> = self.workers.lock().unwrap().iter().filter(|(_, (r, _))| r == id).map(|(s, _)| s.clone()).collect();
        for s in sessions {
            let _ = self.agent.cancel_child(&run.run_session, &s).await;
        }
    }

    /// Pause (workflows.md section 3.4): the workers are cancelled, the tasks keep their states.
    pub async fn pause_run(&self, id: &str) -> Result<Run, EngineError> {
        let cell = self.cell(id)?;
        let state = cell.run.lock().unwrap().state;
        if !matches!(state, RunState::Planning | RunState::Working) {
            return Err(EngineError::Conflict(format!("a run that is {} cannot be paused", state.word())));
        }
        cell.cancel.lock().unwrap().cancel();
        self.modify(id, |run| {
            run.state = RunState::Paused;
            run.reason = "paused by the owner".into();
        })?;
        self.cancel_workers(id).await;
        cell.wake.notify_one();
        Ok(self.get(id).expect("the run exists"))
    }

    /// Continue a paused run: the interrupted stage of each task starts again with a new worker.
    pub async fn continue_run(self: &Arc<Self>, id: &str) -> Result<Run, EngineError> {
        let cell = self.cell(id)?;
        if cell.run.lock().unwrap().state != RunState::Paused {
            return Err(EngineError::Conflict("only a paused run can continue".into()));
        }
        // The driver that was cancelled leaves by itself; the new one must not find the old one's flag.
        for _ in 0..500 {
            if !cell.driving.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        *cell.cancel.lock().unwrap() = CancellationToken::new();
        self.modify(id, |run| {
            run.state = if run.tasks.is_empty() { RunState::Planning } else { RunState::Working };
            run.reason = String::new();
        })?;
        self.spawn_driver(id);
        Ok(self.get(id).expect("the run exists"))
    }

    /// The owner approves the plan, perhaps edited: `edits` is the new list of tasks in order with their titles (a task left
    /// out is removed).
    pub fn approve_plan(self: &Arc<Self>, id: &str, edits: Option<Vec<TaskEdit>>) -> Result<Run, EngineError> {
        let cell = self.cell(id)?;
        let run = cell.run.lock().unwrap().clone();
        if run.state != RunState::AwaitingApproval {
            return Err(EngineError::Conflict(format!("the run is {}, there is no plan to approve", run.state.word())));
        }
        let tasks = match edits {
            None => run.tasks.clone(),
            Some(edits) => apply_edits(&run.tasks, &edits)?,
        };
        let plan = PlanDoc { summary: run.plan_summary.clone(), tasks: tasks.clone() };
        if let Err(e) = self.store.save_plan(id, &plan) {
            tracing::warn!(run = id, "the plan could not be saved: {e}");
        }
        self.modify(id, |run| {
            run.tasks = tasks;
            run.state = RunState::Working;
            run.reason = String::new();
        })?;
        self.spawn_driver(id);
        Ok(self.get(id).expect("the run exists"))
    }

    /// The owner rejects the plan with a reason: the planner makes a new one (at most `MAX_REJECTIONS` times).
    pub fn reject_plan(self: &Arc<Self>, id: &str, reason: &str) -> Result<Run, EngineError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > 2000 {
            return Err(EngineError::Invalid("reason: 1 to 2000 characters, so the planner knows what to change".into()));
        }
        let cell = self.cell(id)?;
        if cell.run.lock().unwrap().state != RunState::AwaitingApproval {
            return Err(EngineError::Conflict("there is no plan to reject".into()));
        }
        let stop = self.modify(id, |run| {
            run.rejections += 1;
            if run.rejections >= MAX_REJECTIONS {
                run.state = RunState::Stopped;
                run.reason = format!("plan rejected {MAX_REJECTIONS} times");
                true
            } else {
                run.state = RunState::Planning;
                run.feedback = Some(reason.to_string());
                run.tasks.clear();
                false
            }
        })?;
        if !stop {
            *cell.cancel.lock().unwrap() = CancellationToken::new();
            self.spawn_driver(id);
        }
        Ok(self.get(id).expect("the run exists"))
    }

    /// What the owner decides about a task that needs him (workflows.md section 3.3).
    pub fn decide(&self, id: &str, task: &str, choice: Decision) -> Result<Run, EngineError> {
        let cell = self.cell(id)?;
        {
            let run = cell.run.lock().unwrap();
            if run.state != RunState::Working {
                return Err(EngineError::Conflict(format!("the run is {}: continue it first", run.state.word())));
            }
            match run.task(task) {
                None => return Err(EngineError::NotFound(format!("no task '{task}' in this run"))),
                Some(t) if t.state != TaskState::NeedsOwner => return Err(EngineError::Conflict(format!("task {task} is {}, it does not need a decision", t.state.word()))),
                Some(_) => {}
            }
        }
        self.modify(id, |run| {
            let t = run.task_mut(task).expect("checked");
            match choice {
                Decision::MoreRound => {
                    t.extra_rounds += 1;
                    t.round += 1;
                    t.state = TaskState::Changes;
                }
                Decision::Accept => {
                    t.state = TaskState::Done;
                    t.accepted = true;
                    t.ended_at = Some(Utc::now());
                }
                Decision::Drop => {
                    t.state = TaskState::Dropped;
                    t.ended_at = Some(Utc::now());
                }
            }
        })?;
        cell.wake.notify_one();
        Ok(self.get(id).expect("the run exists"))
    }

    /// Record the keys of the tickets of a run (tickets.md section 9); `None` leaves a key as it is.
    pub fn set_tickets(&self, id: &str, run_key: Option<&str>, tasks: &HashMap<String, String>) -> Result<(), EngineError> {
        let changed = {
            let cell = self.cell(id)?;
            let run = cell.run.lock().unwrap();
            run_key.is_some_and(|k| run.ticket.as_deref() != Some(k)) || tasks.iter().any(|(t, k)| run.task(t).is_some_and(|t| t.ka_ticket.as_deref() != Some(k)))
        };
        if changed {
            self.modify(id, |run| {
                if let Some(k) = run_key {
                    run.ticket = Some(k.to_string());
                }
                for (t, k) in tasks {
                    if let Some(t) = run.task_mut(t) {
                        t.ka_ticket = Some(k.clone());
                    }
                }
            })?;
        }
        Ok(())
    }
}

/// The tasks after the owner's edits: order and titles, removed tasks gone; a task another one depends on cannot go.
fn apply_edits(tasks: &[Task], edits: &[TaskEdit]) -> Result<Vec<Task>, EngineError> {
    if edits.is_empty() {
        return Err(EngineError::Invalid("tasks: at least one task must stay".into()));
    }
    let mut out: Vec<Task> = Vec::new();
    for e in edits {
        let Some(t) = tasks.iter().find(|t| t.id == e.id) else { return Err(EngineError::Invalid(format!("tasks: no task '{}' in the plan", e.id))) };
        if out.iter().any(|o| o.id == e.id) {
            return Err(EngineError::Invalid(format!("tasks: '{}' is listed twice", e.id)));
        }
        let mut t = t.clone();
        if let Some(title) = e.title.as_deref().map(str::trim) {
            if title.is_empty() || title.chars().count() > 200 {
                return Err(EngineError::Invalid(format!("tasks: the title of '{}' is 1 to 200 characters", e.id)));
            }
            t.title = title.to_string();
        }
        out.push(t);
    }
    for t in &out {
        if let Some(d) = t.depends_on.iter().find(|d| !out.iter().any(|o| &o.id == *d)) {
            return Err(EngineError::Invalid(format!("tasks: '{}' depends on '{d}', which was removed", t.id)));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, deps: &[&str]) -> Task {
        let mut t = Task::new(id, &format!("Title {id}"));
        t.depends_on = deps.iter().map(|d| d.to_string()).collect();
        t
    }

    #[test]
    fn edits_reorder_rename_and_remove() {
        let tasks = vec![t("a", &[]), t("b", &["a"]), t("c", &[])];
        let out = apply_edits(&tasks, &[TaskEdit { id: "c".into(), title: Some("First".into()) }, TaskEdit { id: "a".into(), title: None }, TaskEdit { id: "b".into(), title: None }]).unwrap();
        assert_eq!(out.iter().map(|t| (t.id.as_str(), t.title.as_str())).collect::<Vec<_>>(), [("c", "First"), ("a", "Title a"), ("b", "Title b")]);
        assert!(apply_edits(&tasks, &[TaskEdit { id: "b".into(), title: None }]).unwrap_err().to_string().contains("depends on 'a', which was removed"));
        assert!(apply_edits(&tasks, &[TaskEdit { id: "zz".into(), title: None }]).is_err());
        assert!(apply_edits(&tasks, &[]).is_err());
        assert!(apply_edits(&tasks, &[TaskEdit { id: "a".into(), title: Some(" ".into()) }]).is_err());
        assert!(apply_edits(&tasks, &[TaskEdit { id: "a".into(), title: None }, TaskEdit { id: "a".into(), title: None }]).is_err());
    }
}
