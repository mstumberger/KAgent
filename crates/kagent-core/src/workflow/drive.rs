//! The driver of a run (workflows.md section 3): one task per active run that plans, schedules the tasks and ends the run.
//! A stop or a pause cancels its token and the driver leaves; a new one starts on Continue, on an approval and on a rejection.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::definition::Role;
use super::engine::Engine;
use super::model::{Run, RunState, TaskState};
use super::prompts;
use super::worker::{Prompt, Stop};
use crate::protocol::AgentEvent;

impl Engine {
    /// Start the driver of `id` unless one runs.
    pub(super) fn spawn_driver(self: &Arc<Self>, id: &str) {
        let Ok(cell) = self.cell(id) else { return };
        if cell.driving.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        let this = self.clone();
        let id = id.to_string();
        tokio::spawn(async move {
            this.drive(&id).await;
            if let Ok(cell) = this.cell(&id) {
                cell.driving.store(false, Ordering::SeqCst);
            }
        });
    }

    async fn drive(self: &Arc<Self>, id: &str) {
        let Ok(cell) = self.cell(id) else { return };
        let cancel = cell.cancel.lock().unwrap().clone();
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let state = cell.run.lock().unwrap().state;
            match state {
                RunState::Planning => match self.plan_phase(id, &cancel).await {
                    Ok(()) => {}
                    Err(Stop::Interrupted) => return,
                    Err(Stop::Failed(why)) => {
                        self.end_run(id, RunState::Failed, &why, &cancel);
                        return;
                    }
                },
                RunState::Working => {
                    self.work_phase(id, &cancel).await;
                    return;
                }
                _ => return,
            }
        }
    }

    /// The planner makes the plan; a malformed one is sent back once (workflows.md section 3.2).
    async fn plan_phase(self: &Arc<Self>, id: &str, cancel: &CancellationToken) -> Result<(), Stop> {
        let run = self.get(id).ok_or_else(|| Stop::Failed("the run is gone".into()))?;
        let fresh = prompts::planner(&run.goal, &run.project, run.feedback.as_deref());
        let resume = run.planner_session.clone().zip(run.feedback.as_deref().map(prompts::rejected));
        let what = format!("Plan: {}", run.title);
        let out = self.run_worker(id, "", Role::Planner, &what, Prompt { fresh: fresh.clone(), resume }, 0, cancel).await?;
        let plan = match super::plan::parse(&out.text) {
            Ok(p) => p,
            Err(problems) => {
                let retry = prompts::plan_retry(&problems);
                let again = self.run_worker(id, "", Role::Planner, &what, Prompt { fresh: format!("{fresh}\n{retry}"), resume: Some((out.session.clone(), retry)) }, 0, cancel).await?;
                super::plan::parse(&again.text).map_err(|p| Stop::Failed(format!("the planner did not return a valid plan: {}", p.lines().next().unwrap_or_default())))?
            }
        };
        if let Err(e) = self.store.save_plan(id, &plan) {
            tracing::warn!(run = id, "the plan could not be saved: {e}");
        }
        let gated = run.definition.gated();
        let applied = self.modify_if(id, Some(cancel), |r| {
            r.plan_summary = plan.summary.clone();
            r.tasks = plan.tasks.clone();
            r.feedback = None;
            r.state = if gated { RunState::AwaitingApproval } else { RunState::Working };
        });
        match applied {
            Ok(Some(())) => {
                if gated {
                    self.needs_owner(id, "the plan is ready: approve it, or reject it with a reason");
                }
                Ok(())
            }
            _ => Err(Stop::Interrupted),
        }
    }

    /// Tasks in plan order, as many at once as the definition says.
    async fn work_phase(self: &Arc<Self>, id: &str, cancel: &CancellationToken) {
        let Ok(cell) = self.cell(id) else { return };
        let mut set: JoinSet<String> = JoinSet::new();
        let mut running: HashSet<String> = HashSet::new();
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let run = cell.run.lock().unwrap().clone();
            if run.state != RunState::Working {
                break;
            }
            self.block_dependents(id, cancel);
            let ready = pick_ready(&run, &running);
            for task in &ready {
                running.insert(task.clone());
                let (this, run_id, task, token) = (self.clone(), id.to_string(), task.clone(), cancel.clone());
                set.spawn(async move {
                    this.task_loop(&run_id, &task, &token).await;
                    task
                });
            }
            if running.is_empty() {
                let waiting_owner = run.tasks.iter().any(|t| t.state == TaskState::NeedsOwner);
                if !waiting_owner {
                    self.finish(id, cancel);
                    break;
                }
            }
            tokio::select! {
                joined = set.join_next(), if !set.is_empty() => {
                    if let Some(Ok(done)) = joined {
                        running.remove(&done);
                    }
                }
                _ = cell.wake.notified() => {}
                _ = cancel.cancelled() => break,
            }
        }
        // Workers that were cancelled answer quickly.
        while set.join_next().await.is_some() {}
    }

    /// A task whose dependency cannot be done any more is blocked.
    fn block_dependents(&self, id: &str, cancel: &CancellationToken) {
        loop {
            let Some(run) = self.get(id) else { return };
            let broken = |t: &str| run.task(t).is_none_or(|d| matches!(d.state, TaskState::Failed | TaskState::Dropped | TaskState::Blocked | TaskState::Stopped));
            let Some(blocked) = run.tasks.iter().find(|t| t.state == TaskState::Waiting && t.depends_on.iter().any(|d| broken(d))) else { return };
            let (tid, dep) = (blocked.id.clone(), blocked.depends_on.iter().find(|d| broken(d)).cloned().unwrap_or_default());
            if !self.set_task(id, &tid, cancel, |t| {
                t.state = TaskState::Blocked;
                t.error = format!("blocked: task {dep} did not get done");
            }) {
                return;
            }
        }
    }

    /// Every task is settled: the run's report and its end.
    fn finish(&self, id: &str, cancel: &CancellationToken) {
        let Some(run) = self.get(id) else { return };
        let (state, reason) = outcome(&run);
        let report = super::report::summary(&run);
        let _ = self.modify_if(id, Some(cancel), |r| r.report = report);
        self.end_run(id, state, &reason, cancel);
    }

    /// End a run `state` for `reason` and tell everyone who should know (the owner's channels: a notice, and the
    /// conversation that started it).
    pub(super) fn end_run(&self, id: &str, state: RunState, reason: &str, cancel: &CancellationToken) {
        let Ok(Some(())) = self.modify_if(id, Some(cancel), |r| {
            r.state = state;
            r.reason = reason.to_string();
        }) else {
            return;
        };
        let Some(run) = self.get(id) else { return };
        let level = if state == RunState::Done { "info" } else { "error" };
        let text = if reason.is_empty() { format!("Run \"{}\" is {}.", run.title, state.word()) } else { format!("Run \"{}\" is {}: {reason}", run.title, state.word()) };
        self.agent.emit(AgentEvent::keyed_notice(level, text.clone(), format!("run:{id}:end")));
        self.tell_origin(&run, &format!("[Run {id} \"{}\" {}] {reason}\n{}", run.title, state.word(), crate::agent::children::cut(&run.report, 3000)));
    }

    /// The run needs the owner (a plan to approve, a decision, a question): a notice and a message to the conversation.
    pub(super) fn needs_owner(&self, id: &str, why: &str) {
        let Some(run) = self.get(id) else { return };
        let text = format!("Run \"{}\" needs you: {why}", run.title);
        self.agent.emit(AgentEvent::keyed_notice("warn", text.clone(), format!("run:{id}:owner")));
        self.tell_origin(&run, &format!("[Run {id} \"{}\" needs the owner] {why}", run.title));
    }

    /// Store `text` in the conversation that started the run (it starts a turn when that conversation is idle at the chat).
    pub(super) fn tell_origin(&self, run: &Run, text: &str) {
        if let Some(origin) = run.origin_session.clone() {
            let agent = self.agent.clone();
            let text = text.to_string();
            tokio::spawn(async move {
                let _ = agent.notify(&origin, &text).await;
            });
        }
    }
}

/// The run's end from its tasks (workflows.md section 3.3).
pub(super) fn outcome(run: &Run) -> (RunState, String) {
    let count = |s: TaskState| run.tasks.iter().filter(|t| t.state == s).count();
    let (failed, blocked, done) = (count(TaskState::Failed), count(TaskState::Blocked), count(TaskState::Done));
    let planned_only = run.definition.step(Role::Implementer).is_none() && run.definition.step(Role::Reviewer).is_none();
    if failed > 0 || blocked > 0 {
        (RunState::Failed, format!("{failed} task(s) failed, {blocked} blocked"))
    } else if planned_only || done > 0 {
        (RunState::Done, String::new())
    } else {
        (RunState::Failed, "no task was done".into())
    }
}

/// The tasks to start now: those that were in flight when the run was paused, then waiting ones whose dependencies are done
/// and whose files do not overlap one that runs, up to `parallel` working at once.
fn pick_ready(run: &Run, running: &HashSet<String>) -> Vec<String> {
    let works = run.definition.step(Role::Implementer).is_some() || run.definition.step(Role::Reviewer).is_some();
    if !works {
        return Vec::new();
    }
    let parallel = run.definition.parallel() as usize;
    let working = |id: &str| run.task(id).is_some_and(|t| t.state != TaskState::NeedsOwner);
    let mut count = running.iter().filter(|id| working(id)).count();
    let mut picked: Vec<String> = Vec::new();
    let mut taken_files: Vec<&Vec<String>> = running.iter().filter_map(|id| run.task(id)).map(|t| &t.files).collect();
    // Tasks that were in flight come first, then the waiting ones in plan order.
    let in_flight = |t: &super::model::Task| matches!(t.state, TaskState::Implementing | TaskState::InReview | TaskState::Changes);
    for t in run.tasks.iter().filter(|t| in_flight(t)).chain(run.tasks.iter().filter(|t| !in_flight(t))) {
        if running.contains(&t.id) {
            continue;
        }
        let flying = in_flight(t);
        let ready = t.state == TaskState::Waiting && t.depends_on.iter().all(|d| run.task(d).is_some_and(|d| d.state == TaskState::Done));
        if !(flying || ready) || (count >= parallel && !flying) {
            continue;
        }
        if ready && taken_files.iter().any(|f| overlap(f, &t.files)) {
            continue;
        }
        picked.push(t.id.clone());
        taken_files.push(&t.files);
        count += 1;
    }
    picked
}

fn overlap(a: &[String], b: &[String]) -> bool {
    let norm = |p: &str| p.trim_end_matches('/').to_string();
    a.iter().any(|x| b.iter().any(|y| {
        let (x, y) = (norm(x), norm(y));
        x == y || x.starts_with(&format!("{y}/")) || y.starts_with(&format!("{x}/"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::model::Task;

    fn run_with(tasks: Vec<Task>, parallel: u32) -> Run {
        let text = include_str!("../../../../workflows/plan-implement-review.yaml").replace("parallel: 1", &format!("parallel: {parallel}"));
        let def = crate::workflow::Definition::parse(&text, "plan-implement-review", &|_| true).unwrap();
        let now = chrono::Utc::now();
        Run { id: "r".into(), workflow: "w".into(), definition: def, project: "/p".into(), goal: "g".into(), title: "g".into(), state: RunState::Working, reason: String::new(), origin_session: None, run_session: "s".into(), ticket: None, plan_summary: String::new(), tasks, questions: vec![], messages: vec![], planner_session: None, feedback: None, rejections: 0, report: String::new(), created_at: now, updated_at: now }
    }

    fn task(id: &str, deps: &[&str], files: &[&str]) -> Task {
        let mut t = Task::new(id, id);
        t.depends_on = deps.iter().map(|d| d.to_string()).collect();
        t.files = files.iter().map(|f| f.to_string()).collect();
        t
    }

    #[test]
    fn dependencies_the_limit_and_overlapping_files_decide_what_starts() {
        let run = run_with(vec![task("a", &[], &["src/a.rs"]), task("b", &[], &["src/b.rs"]), task("c", &["a"], &[]), task("d", &[], &["src"])], 2);
        assert_eq!(pick_ready(&run, &HashSet::new()), ["a", "b"], "two at once; c waits for a; d overlaps a");
        let one = run_with(run.tasks.clone(), 1);
        assert_eq!(pick_ready(&one, &HashSet::new()), ["a"]);
        let mut running = HashSet::new();
        running.insert("a".to_string());
        assert_eq!(pick_ready(&one, &running), Vec::<String>::new(), "a is running and the limit is 1");
        let mut after = run.clone();
        after.tasks[0].state = TaskState::Done;
        assert_eq!(pick_ready(&after, &HashSet::new()), ["b", "c"], "c and b: d overlaps b, which runs now");
    }

    #[test]
    fn a_task_in_flight_goes_first_and_one_that_needs_the_owner_holds_no_slot() {
        let mut run = run_with(vec![task("a", &[], &[]), task("b", &[], &[])], 1);
        run.tasks[1].state = TaskState::InReview;
        assert_eq!(pick_ready(&run, &HashSet::new()), ["b"]);
        run.tasks[1].state = TaskState::NeedsOwner;
        let mut running = HashSet::new();
        running.insert("b".to_string());
        assert_eq!(pick_ready(&run, &running), ["a"], "b waits for the owner and holds no slot");
    }

    #[test]
    fn the_end_of_a_run_follows_its_tasks() {
        let mut run = run_with(vec![task("a", &[], &[]), task("b", &["a"], &[])], 1);
        run.tasks[0].state = TaskState::Done;
        run.tasks[1].state = TaskState::Dropped;
        assert_eq!(outcome(&run).0, RunState::Done);
        run.tasks[1].state = TaskState::Failed;
        assert_eq!(outcome(&run).0, RunState::Failed);
        run.tasks[0].state = TaskState::Dropped;
        run.tasks[1].state = TaskState::Dropped;
        assert_eq!(outcome(&run), (RunState::Failed, "no task was done".into()));
    }
}
