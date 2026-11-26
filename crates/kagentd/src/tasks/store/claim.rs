//! One pass of the runner (automations.md sections 2 and 3): which tasks start now, how
//! a claimed task is marked, and how a run that is too late is recorded `missed`.

use super::*;
use crate::tasks::schedule::Schedule;

impl TaskStore {
    /// One pass of the runner: the tasks to start now. Each is marked running and gets
    /// its next `nextRun`, computed from the slot it runs, never from when it ends. A
    /// scheduled run that is late (over a minute for `once`, over five minutes
    /// otherwise) is recorded `missed` and never replayed. At most `max_agent` agent
    /// tasks run at once; the others wait for a later pass.
    pub fn claim(&self, now: DateTime<Local>, max_agent: u32) -> Vec<Task> {
        let mut inner = self.inner.lock().unwrap();
        let busy = inner.running.values().filter(|r| r.agent).count();
        let mut free = (max_agent.max(1) as usize).saturating_sub(busy);
        let mut claimed = Vec::new();
        let mut missed = Vec::new();
        for i in 0..inner.tasks.len() {
            let task = &inner.tasks[i];
            let id = task.id.clone();
            if inner.running.contains_key(&id) {
                continue;
            }
            let requested = inner.requested.contains(&id);
            let due = task.next_run.filter(|_| task.enabled && !task.done).filter(|slot| *slot <= now);
            if !requested && due.is_none() {
                continue;
            }
            let agent = task.kind == "agent";
            let limit = if matches!(task.schedule, Some(Schedule::Once { .. })) { ON_TIME_ONCE } else { ON_TIME };
            // "Run now" was asked: the task runs, however late its slot is.
            if due.is_some_and(|slot| now - slot > limit) && !requested {
                self.miss(&mut inner, i, now, &mut missed);
                continue;
            }
            if agent && free == 0 {
                continue;
            }
            inner.requested.remove(&id);
            if agent {
                free -= 1;
            }
            if let Some(task) = self.start(&mut inner, i, now, due) {
                claimed.push(task);
            }
        }
        if !claimed.is_empty() || !missed.is_empty() {
            self.save(&inner);
        }
        for (id, run) in missed {
            self.emit_run(&id, &run);
        }
        claimed
    }

    /// Claim a task: marked running, a run record opened, and the next `nextRun` set.
    fn start(&self, inner: &mut Inner, i: usize, now: DateTime<Local>, slot: Option<DateTime<Local>>) -> Option<Task> {
        let task = &mut inner.tasks[i];
        task.last_run = Some(now);
        task.last_status = Some(RunStatus::Running);
        Self::keep(task, TaskRun::running(now));
        match (&task.schedule, slot) {
            (Some(schedule), Some(slot)) if schedule.repeats() => {
                // From the slot that runs, so later runs do not drift; from now when it
                // is long past (a "run now").
                task.next_run = schedule.next_run(if now - slot > ON_TIME { now } else { slot });
            }
            (Some(_), Some(_)) => {
                task.next_run = None;
                task.done = true;
            }
            _ => {
                // "Run now" outside its schedule (a to-do, a paused task, a run before
                // the scheduled time): a slot still ahead is kept.
                match task.schedule.as_ref() {
                    Some(schedule) if schedule.repeats() => task.next_run = task.next_run.filter(|n| *n > now).or_else(|| schedule.next_run(now)),
                    Some(_) => task.next_run = task.next_run.filter(|n| *n > now),
                    None => {}
                }
                task.done = task.schedule.is_some() && task.next_run.is_none();
            }
        }
        let token = CancellationToken::new();
        let agent = task.kind == "agent";
        let claimed = task.clone();
        inner.running.insert(claimed.id.clone(), Running { token, agent });
        Some(claimed)
    }

    /// A scheduled run that is too late: recorded, not replayed.
    fn miss(&self, inner: &mut Inner, i: usize, now: DateTime<Local>, missed: &mut Vec<(String, TaskRun)>) {
        let slot = inner.tasks[i].next_run.unwrap_or(now);
        let task = &mut inner.tasks[i];
        let run = TaskRun { at: slot, status: RunStatus::Missed, duration_sec: None, session_id: None, result: None };
        Self::keep(task, run.clone());
        task.last_run = Some(slot);
        task.last_status = Some(RunStatus::Missed);
        task.last_result = Some("missed: the daemon was down or busy".into());
        match task.schedule.as_ref() {
            Some(schedule) if schedule.repeats() => task.next_run = schedule.next_run(now),
            _ => {
                task.next_run = None;
                task.done = true;
            }
        }
        let id = task.id.clone();
        inner.requested.remove(&id);
        tracing::warn!(task = %id, "the run scheduled for {} was missed; it is not replayed", slot.format("%a %d %b %H:%M"));
        missed.push((id, run));
    }
}
