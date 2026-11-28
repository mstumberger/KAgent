//! The registry behind the subagents (agents.md section 2): what a restricted session may
//! do (its tool groups, read-only, no tools over subagents) and the children of this daemon
//! run with their queue. The orchestration that uses it is `run.rs`.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

use chrono::Utc;
use tokio::sync::watch;

use super::*;
use crate::tool::CallAssessment;

/// Tools a subagent does not have: it cannot start or manage subagents (the depth limit).
const ORCHESTRATION: [&str; 5] = ["agent_run", "agent_wait", "agent_stop", "agent_list", "agent_tell"];

/// What a session may do beyond the enabled groups: a subagent's profile.
#[derive(Debug, Clone, Default)]
pub(crate) struct Restriction {
    /// The groups its tools must be in; `None` = every enabled group.
    pub groups: Option<Vec<String>>,
    /// Every call that is not read-only is refused (`explore`).
    pub read_only: bool,
    /// A subagent: it has no tools over subagents.
    pub child: bool,
}

impl Restriction {
    pub(crate) fn allows(&self, tool: &str) -> bool {
        !(self.child && ORCHESTRATION.contains(&tool))
            && self.groups.as_ref().is_none_or(|groups| groups.iter().any(|g| g == crate::tools::group_of(tool)))
    }
}

#[derive(Clone)]
pub(super) struct Job {
    pub(super) model_ref: String,
    pub(super) max_iterations: usize,
    /// The block of the system prompt (agents.md section 2).
    pub(super) block: String,
    /// The message that starts the child; kept while the child waits its turn.
    pub(super) text: Option<String>,
}

pub(super) struct Child {
    pub(super) parent: String,
    pub(super) view: ChildView,
    pub(super) job: Job,
    pub(super) started: Option<Instant>,
    /// `Some` once the child ended; a late subscriber sees it at once.
    pub(super) end: watch::Sender<Option<End>>,
    /// When the progress of its tool calls was last announced (at most every `PROGRESS_EVERY`).
    pub(super) last_emit: Option<Instant>,
    /// The owner interrupted it: its cancelled turn is followed by one that holds the owner's message (steering.md section 4).
    pub(super) interrupted: bool,
}

/// The summary of a call without the name of the job `named` put in front of it.
fn plain(title: &str, summary: &str) -> String {
    summary.strip_prefix(&format!("subagent \"{title}\": ")).unwrap_or(summary).to_string()
}

/// Progress of a running tool call is announced at most this often per child; the end of a call never waits.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Default)]
struct Inner {
    max: usize,
    running: usize,
    /// Children waiting for a running slot, oldest first.
    queue: VecDeque<String>,
    order: Vec<String>,
    children: HashMap<String, Child>,
    restrictions: HashMap<String, Restriction>,
}

/// The subagents of this daemon run, and what each restricted session may do.
pub(crate) struct Children {
    inner: Mutex<Inner>,
    model_window: Mutex<Option<ModelWindow>>,
    usable_models: Mutex<Option<UsableModels>>,
}

impl Default for Children {
    /// Four slots, as `subagents.maxConcurrent` defaults (config.md section 11): the
    /// daemon sets what the owner chose.
    fn default() -> Self {
        Self { inner: Mutex::new(Inner { max: DEFAULT_MAX_CHILDREN, ..Default::default() }), model_window: Mutex::new(None), usable_models: Mutex::new(None) }
    }
}

impl Children {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub(super) fn restriction(&self, session_id: &str) -> Option<Restriction> {
        self.lock().restrictions.get(session_id).cloned()
    }

    /// The job a session runs (a subagent's), for the start of each of its turns.
    pub(super) fn job(&self, session_id: &str) -> Option<Job> {
        self.lock().children.get(session_id).map(|c| c.job.clone())
    }

    /// Where the window of a profile's model is read (`None`: the chat model's is kept).
    pub(super) fn set_model_window(&self, source: ModelWindow) {
        *self.model_window.lock().unwrap() = Some(source);
    }

    pub(super) fn model_window(&self) -> Option<ModelWindow> {
        self.model_window.lock().unwrap().clone()
    }

    /// Where a conversation's choice of model is asked of (`None`: taken as it is).
    pub(crate) fn set_usable_models(&self, source: UsableModels) {
        *self.usable_models.lock().unwrap() = Some(source);
    }

    pub(crate) fn usable_models(&self) -> Option<UsableModels> {
        self.usable_models.lock().unwrap().clone()
    }

    pub(super) fn restrict(&self, session_id: &str, restriction: Restriction) {
        self.lock().restrictions.insert(session_id.to_string(), restriction);
    }

    pub(crate) fn forget(&self, session_id: &str) {
        let mut inner = self.lock();
        inner.restrictions.remove(session_id);
        if let Some(child) = inner.children.remove(session_id) {
            if child.view.status == ChildStatus::Running {
                inner.running = inner.running.saturating_sub(1);
            }
            inner.queue.retain(|id| id != session_id);
            inner.order.retain(|id| id != session_id);
        }
    }

    /// Why a call of `session_id` is refused, when its restriction refuses it.
    pub(super) fn refusal(&self, session_id: &str, tool: &str, call: &CallAssessment) -> Option<String> {
        let inner = self.lock();
        let r = inner.restrictions.get(session_id)?;
        if r.child && ORCHESTRATION.contains(&tool) {
            return Some("a subagent cannot start or manage subagents: do the work with your own tools and end with a complete answer".into());
        }
        if let Some(groups) = r.groups.as_ref() {
            if !groups.iter().any(|g| g == crate::tools::group_of(tool)) {
                return Some(format!("tool '{tool}' is not in this subagent's tool groups ({}): call only tools of those groups", groups.join(", ")));
            }
        }
        if r.read_only && !call.read_only {
            return Some("explore is read-only: this call would change something. Research with read-only tools, then answer".into());
        }
        None
    }

    pub(super) fn insert(&self, mut child: Child) -> (bool, ChildView) {
        let mut inner = self.lock();
        let run_now = inner.running < inner.max;
        let id = child.view.id.clone();
        inner.order.push(id.clone());
        if run_now {
            // The slot is taken now, so a turn that never starts gives it back too.
            inner.running += 1;
            child.view.status = ChildStatus::Running;
        } else {
            inner.queue.push_back(id.clone());
        }
        let view = child.view.clone();
        inner.children.insert(id, child);
        (run_now, view)
    }

    pub(super) fn take_text(&self, id: &str) -> Option<String> {
        self.lock().children.get_mut(id)?.job.text.take()
    }

    pub(super) fn started(&self, id: &str) -> Option<ChildView> {
        let mut inner = self.lock();
        let child = inner.children.get_mut(id)?;
        child.view.started_at = Some(Utc::now());
        child.started = Some(Instant::now());
        Some(child.view.clone())
    }

    /// Record how the child ended. `None` when it is unknown or had ended already.
    pub(super) fn finish(&self, id: &str, status: ChildStatus, error: Option<String>, output: String) -> Option<(String, ChildView, End)> {
        let mut inner = self.lock();
        let (was_running, parent, view, end) = {
            let child = inner.children.get_mut(id)?;
            if child.view.status.ended() {
                return None;
            }
            let was_running = child.view.status == ChildStatus::Running;
            let seconds = child.started.map_or(0, |started| started.elapsed().as_secs());
            let end = End { status, seconds, output, error };
            child.view.status = status;
            child.view.ended_at = Some(Utc::now());
            let _ = child.end.send_replace(Some(end.clone()));
            (was_running, child.parent.clone(), child.view.clone(), end)
        };
        // Ended while it waited its turn (`agent_stop`, the kill switch): out of the queue.
        inner.queue.retain(|queued| queued != id);
        if was_running {
            inner.running = inner.running.saturating_sub(1);
        }
        Some((parent, view, end))
    }

    /// The child now: its parent, its view, how it ended, and a receiver that sees the end.
    pub(super) fn get(&self, id: &str) -> Option<(String, ChildView, Option<End>, watch::Receiver<Option<End>>)> {
        let inner = self.lock();
        let child = inner.children.get(id)?;
        let ended = child.end.borrow().clone();
        let receiver = child.end.subscribe();
        Some((child.parent.clone(), child.view.clone(), ended, receiver))
    }

    pub(super) fn views(&self, parent: &str) -> Vec<ChildView> {
        let inner = self.lock();
        inner
            .order
            .iter()
            .filter_map(|id| inner.children.get(id))
            .filter(|c| c.parent == parent)
            .map(|c| c.view.clone())
            .collect()
    }

    /// The children still waiting for a slot, oldest first.
    pub(super) fn queued(&self) -> Vec<String> {
        self.lock().queue.iter().cloned().collect()
    }

    /// The next child in turn, when a slot is free: it takes the slot.
    pub(super) fn next_queued(&self) -> Option<String> {
        let mut inner = self.lock();
        if inner.running >= inner.max {
            return None;
        }
        loop {
            // An entry that is gone, or was cancelled while it waited, is skipped.
            let id = inner.queue.pop_front()?;
            let Some(child) = inner.children.get_mut(&id) else { continue };
            if child.view.status != ChildStatus::Queued {
                continue;
            }
            child.view.status = ChildStatus::Running;
            inner.running += 1;
            return Some(id);
        }
    }

    pub(super) fn set_max(&self, max: usize) {
        self.lock().max = max;
    }

    /// A tool call of `session` started: when it is a running child, its last action and files are updated. The child to
    /// announce (`None` for a session that is no child, and while the last announcement is younger than 500 ms).
    pub(crate) fn note_call(&self, session: &str, name: &str, input: &serde_json::Value, summary: &str) -> Option<(String, ChildView)> {
        let mut inner = self.lock();
        let child = inner.children.get_mut(session)?;
        let summary = plain(&child.view.title, summary);
        child.view.last_action = Some(live::LastAction { tool: name.into(), summary, status: live::ActionStatus::Running, at: Utc::now() });
        if let Some((path, changed)) = live::touched(name, input) {
            live::add_touch(&mut child.view.files, path, changed);
        }
        if child.last_emit.is_some_and(|at| at.elapsed() < PROGRESS_EVERY) {
            return None;
        }
        child.last_emit = Some(Instant::now());
        Some((child.parent.clone(), child.view.clone()))
    }

    /// The owner's message to child `id`: a receipt `queued` on its view. Its id, the parent and the view to announce.
    pub(crate) fn steer_add(&self, id: &str, text: &str, interrupt: bool) -> Option<(String, String, ChildView)> {
        let mut inner = self.lock();
        let child = inner.children.get_mut(id)?;
        let steer_id = format!("st_{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
        child.view.steering.push(live::Steering { id: steer_id.clone(), text: text.into(), interrupt, state: live::SteerState::Queued, at: Utc::now(), delivered_at: None });
        if child.view.steering.len() > live::MAX_STEERING {
            // The oldest that was read goes first.
            let at = child.view.steering.iter().position(|s| s.state == live::SteerState::Delivered).unwrap_or(0);
            child.view.steering.remove(at);
        }
        Some((steer_id, child.parent.clone(), child.view.clone()))
    }

    /// The worker read these messages: the receipts become `delivered`. The view to announce, when any changed.
    pub(crate) fn steer_delivered(&self, id: &str, steer_ids: &[String]) -> Option<(String, ChildView)> {
        let mut inner = self.lock();
        let child = inner.children.get_mut(id)?;
        let mut changed = false;
        for s in child.view.steering.iter_mut().filter(|s| s.state == live::SteerState::Queued && steer_ids.contains(&s.id)) {
            s.state = live::SteerState::Delivered;
            s.delivered_at = Some(Utc::now());
            changed = true;
        }
        changed.then(|| (child.parent.clone(), child.view.clone()))
    }

    /// The ids of the receipts of an interrupt that wait to be read.
    pub(crate) fn queued_interrupts(&self, id: &str) -> Vec<String> {
        let inner = self.lock();
        inner.children.get(id).map(|c| c.view.steering.iter().filter(|s| s.interrupt && s.state == live::SteerState::Queued).map(|s| s.id.clone()).collect()).unwrap_or_default()
    }

    /// Mark (or unmark) child `id` as interrupted.
    pub(crate) fn mark_interrupt(&self, id: &str, yes: bool) {
        if let Some(child) = self.lock().children.get_mut(id) {
            child.interrupted = yes;
        }
    }

    /// Was `id` interrupted? Reading it clears it.
    pub(crate) fn take_interrupt(&self, id: &str) -> bool {
        self.lock().children.get_mut(id).is_some_and(|c| std::mem::take(&mut c.interrupted))
    }

    /// The call of `session` ended with `status` (also a call refused before it ran): always announced.
    pub(crate) fn note_result(&self, session: &str, name: &str, summary: &str, status: crate::protocol::ToolCallStatus) -> Option<(String, ChildView)> {
        let mut inner = self.lock();
        let child = inner.children.get_mut(session)?;
        let summary = plain(&child.view.title, summary);
        let now = live::ActionStatus::of(status);
        match child.view.last_action.as_mut() {
            Some(a) if a.tool == name && a.status == live::ActionStatus::Running => a.status = now,
            _ => child.view.last_action = Some(live::LastAction { tool: name.into(), summary, status: now, at: Utc::now() }),
        }
        child.last_emit = Some(Instant::now());
        Some((child.parent.clone(), child.view.clone()))
    }
}
