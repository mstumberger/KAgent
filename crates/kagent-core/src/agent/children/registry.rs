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
const ORCHESTRATION: [&str; 4] = ["agent_run", "agent_wait", "agent_stop", "agent_list"];

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
}

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
}
