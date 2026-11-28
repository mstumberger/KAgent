//! One task of a run (workflows.md section 3.3): implement, review, send the findings back, until the reviewer passes it, the
//! rounds are used up (then the owner decides) or a worker fails. The loop reads the task's persisted state each time round, so
//! a task that was interrupted starts again where it was.

use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::definition::Role;
use super::engine::Engine;
use super::model::{Finding, Round, Run, Task, TaskState, Verdict};
use super::prompts;
use super::worker::{Prompt, Stop};

impl Engine {
    pub(super) async fn task_loop(self: &Arc<Self>, run_id: &str, task_id: &str, cancel: &CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                return;
            }
            let Some(run) = self.get(run_id) else { return };
            let Some(task) = run.task(task_id).cloned() else { return };
            let implementer = run.definition.step(Role::Implementer).is_some();
            let next = match task.state {
                TaskState::Waiting => {
                    let applied = self.set_task(run_id, task_id, cancel, |t| {
                        t.state = if implementer { TaskState::Implementing } else { TaskState::InReview };
                        t.round = 1;
                        t.started_at = Some(Utc::now());
                    });
                    if applied { Ok(()) } else { Err(Stop::Interrupted) }
                }
                TaskState::Implementing | TaskState::Changes => self.implement_stage(&run, &task, cancel).await,
                TaskState::InReview => self.review_stage(&run, &task, cancel).await,
                _ => return,
            };
            match next {
                Ok(()) => {}
                Err(Stop::Interrupted) => return,
                Err(Stop::Failed(why)) => {
                    self.set_task(run_id, task_id, cancel, |t| {
                        t.state = TaskState::Failed;
                        t.error = why.clone();
                        t.ended_at = Some(Utc::now());
                    });
                    return;
                }
            }
        }
    }

    async fn implement_stage(self: &Arc<Self>, run: &Run, task: &Task, cancel: &CancellationToken) -> Result<(), Stop> {
        let deps: Vec<(&Task, String)> = task.depends_on.iter().filter_map(|d| run.task(d)).map(|d| (d, d.last_report().to_string())).collect();
        let findings: Vec<&Finding> = if task.state == TaskState::Changes { task.open_findings() } else { Vec::new() };
        let restarted = task.workers.iter().any(|w| w.role == Role::Implementer.word() && w.round == task.round) && task.rounds.iter().all(|r| r.round != task.round);
        let fresh = prompts::implementer(&run.goal, task, &prompts::dependency_lines(&deps), &findings, restarted);
        // The same implementer session goes on with the findings (workflows.md section 3.3), when it still can.
        let resume = (task.state == TaskState::Changes).then(|| task.workers.iter().rev().find(|w| w.role == Role::Implementer.word()).map(|w| (w.session.clone(), prompts::findings_back(task.round, &findings)))).flatten();
        let what = format!("Implement {}: {}", task.id, task.title);
        let out = self.run_worker(&run.id, &task.id, Role::Implementer, &what, Prompt { fresh, resume }, task.round, cancel).await?;
        let round = task.round;
        let applied = self.set_task(&run.id, &task.id, cancel, |t| {
            t.rounds.push(Round { round, report: out.text.clone(), verdict: None, findings: Vec::new() });
            for f in &out.changed {
                if !t.changed_files.contains(f) {
                    t.changed_files.push(f.clone());
                }
            }
            t.state = TaskState::InReview;
        });
        if applied { Ok(()) } else { Err(Stop::Interrupted) }
    }

    async fn review_stage(self: &Arc<Self>, run: &Run, task: &Task, cancel: &CancellationToken) -> Result<(), Stop> {
        let files = if task.changed_files.is_empty() { task.files.clone() } else { task.changed_files.clone() };
        let (differ, project) = (self.differ.lock().unwrap().clone(), run.project.clone());
        let diff = tokio::task::spawn_blocking(move || differ(&project, &files)).await.unwrap_or_else(|_| "the diff could not be made".into());
        let fresh = prompts::reviewer(&run.goal, task, task.last_report(), &diff);
        let what = format!("Review {}: {}", task.id, task.title);
        let first = self.run_worker(&run.id, &task.id, Role::Reviewer, &what, Prompt { fresh: fresh.clone(), resume: None }, task.round, cancel).await?;
        let (verdict, findings) = match super::verdict::parse(&first.text) {
            Ok(v) => v,
            Err(problem) => {
                let retry = prompts::verdict_retry(&problem);
                let again = self.run_worker(&run.id, &task.id, Role::Reviewer, &what, Prompt { fresh: format!("{fresh}\n{retry}"), resume: Some((first.session.clone(), retry)) }, task.round, cancel).await?;
                super::verdict::parse(&again.text).map_err(|p| Stop::Failed(format!("the reviewer did not return a valid verdict: {p}")))?
            }
        };
        let (implementer, limit) = (run.definition.step(Role::Implementer).is_some(), run.definition.max_rounds() + task.extra_rounds);
        let applied = self.set_task(&run.id, &task.id, cancel, |t| {
            if t.rounds.last().is_none_or(|r| r.verdict.is_some()) {
                t.rounds.push(Round { round: t.round, report: String::new(), verdict: None, findings: Vec::new() });
            }
            let last = t.rounds.last_mut().expect("a round was just made");
            last.verdict = Some(verdict);
            last.findings = findings;
            match verdict {
                Verdict::Pass => {
                    t.state = TaskState::Done;
                    t.ended_at = Some(Utc::now());
                }
                // Nobody to send the findings to: the review is the product, and the report lists them.
                Verdict::Changes if !implementer => {
                    t.state = TaskState::Done;
                    t.ended_at = Some(Utc::now());
                }
                Verdict::Changes if t.round >= limit => t.state = TaskState::NeedsOwner,
                Verdict::Changes => {
                    t.round += 1;
                    t.state = TaskState::Changes;
                }
            }
        });
        if !applied {
            return Err(Stop::Interrupted);
        }
        if self.get(&run.id).and_then(|r| r.task(&task.id).map(|t| t.state)) == Some(TaskState::NeedsOwner) {
            self.needs_owner(&run.id, &format!("task {} still has problems after {limit} rounds: keep going, accept it or drop it", task.id));
        }
        Ok(())
    }
}
