//! Per-session state and the session map: the running turn, the input queued behind
//! it, the hand-over from one turn to the next, and the session history.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::title;
use crate::permission::{ApprovalDecision, ApprovalKind};
use crate::protocol::{AgentEvent, Message, Role, Via};
use crate::session::SessionMeta;
use crate::todo::{TodoItem, TodoItemInput};

use super::{Agent, TurnResult};

pub(crate) struct SessionState {
    pub meta: tokio::sync::Mutex<SessionMeta>,
    pub messages: tokio::sync::Mutex<Vec<Message>>,
    /// The running turn's token; `None` while the session is idle.
    pub running: Mutex<Option<CancellationToken>>,
    /// Input that arrived while a turn runs, drained at the next model-request boundary.
    pub pending: Mutex<VecDeque<TurnInput>>,
    /// The turn that settled last (`None` until one has).
    pub settled: watch::Sender<Option<TurnResult>>,
    /// The prompt tokens of the last model response, for the compaction trigger
    /// (compaction.md section 6); 0 after a compaction and until the next response.
    pub last_input_tokens: std::sync::atomic::AtomicU64,
}

/// What starts a turn, and what waits in the queue behind a running one.
#[derive(Clone)]
pub(crate) struct TurnInput {
    pub text: String,
    pub via: Via,
    /// `TurnOpts::system_suffix` of this input: with the queued input it goes into
    /// the prompt of the running turn, from its next model request on.
    pub system_suffix: Option<String>,
}

/// What `take` did with a sent input.
pub(crate) enum Taken {
    /// Started a turn under this cancellation token.
    Turn(CancellationToken),
    /// Queued behind the running turn.
    Queued,
}

impl SessionState {
    pub fn new(meta: SessionMeta, messages: Vec<Message>) -> Self {
        Self {
            meta: tokio::sync::Mutex::new(meta),
            messages: tokio::sync::Mutex::new(messages),
            running: Mutex::new(None),
            pending: Mutex::new(VecDeque::new()),
            settled: watch::channel(None).0,
            last_input_tokens: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Start a turn, or queue the input when one is running.
    pub fn take(&self, input: &TurnInput) -> Taken {
        let mut running = self.running.lock().unwrap();
        if running.is_some() {
            self.pending.lock().unwrap().push_back(input.clone());
            return Taken::Queued;
        }
        let token = CancellationToken::new();
        *running = Some(token.clone());
        Taken::Turn(token)
    }

    /// Record the settled turn and make the session idle, or hand the queue over to a
    /// new turn. Both happen under one lock, so `is_idle` is never true in between.
    pub fn settle(&self, result: TurnResult) -> Option<(TurnInput, CancellationToken)> {
        let mut running = self.running.lock().unwrap();
        *running = None;
        // `send_replace`, not `send`: `send` drops the value when nobody is waiting
        // yet, and a later `wait_idle` must still get this turn's result.
        self.settled.send_replace(Some(result));
        let next = self.pending.lock().unwrap().pop_front();
        next.map(|input| {
            let token = CancellationToken::new();
            *running = Some(token.clone());
            (input, token)
        })
    }

    /// No running turn and nothing queued.
    pub fn is_idle(&self) -> bool {
        let running = self.running.lock().unwrap();
        running.is_none() && self.pending.lock().unwrap().is_empty()
    }
}

impl Agent {
    pub(crate) async fn state(&self, session_id: &str) -> anyhow::Result<Arc<SessionState>> {
        if let Some(s) = self.sessions.lock().unwrap().get(session_id) {
            return Ok(s.clone());
        }
        let meta = self.store.meta(session_id).await?;
        let messages = self.store.load_messages(session_id).await?;
        let planning = meta.planning;
        let state = Arc::new(SessionState::new(meta, messages));
        let state = self
            .sessions
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_insert(state)
            .clone();
        // Plan mode is in the meta, so it outlives a daemon restart: the engine follows it.
        self.permissions.set_planning(session_id, planning);
        Ok(state)
    }

    pub async fn messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        Ok(self.state(session_id).await?.messages.lock().await.clone())
    }

    pub async fn is_running(&self, session_id: &str) -> bool {
        match self.state(session_id).await {
            Ok(s) => s.running.lock().unwrap().is_some(),
            Err(_) => false,
        }
    }

    pub fn forget(&self, session_id: &str) {
        if let Some(s) = self.sessions.lock().unwrap().remove(session_id) {
            if let Some(c) = s.running.lock().unwrap().take() {
                c.cancel();
            }
        }
        self.permissions.set_planning(session_id, false);
        self.children.forget(session_id);
    }

    pub async fn cancel(&self, session_id: &str) -> bool {
        let Ok(s) = self.state(session_id).await else { return false };
        let token = s.running.lock().unwrap().clone();
        match token {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// Kill switch: cancel every running turn (a subagent's too); returns how many. The
    /// subagents waiting for a slot never start, and every background task is killed.
    pub async fn stop_all(&self) -> usize {
        // First the waiting ones, so none of them starts when a running subagent ends.
        self.cancel_queued_children();
        let _ = crate::tools::background::Background::shared().stop_all();
        let sessions: Vec<Arc<SessionState>> = self.sessions.lock().unwrap().values().cloned().collect();
        let mut n = 0;
        for s in sessions {
            // Cancel but leave the token in place: the turn task clears it once it has
            // actually unwound, so no new turn can start on top of a dying one.
            if let Some(t) = s.running.lock().unwrap().as_ref() {
                if !t.is_cancelled() {
                    t.cancel();
                    n += 1;
                }
            }
        }
        n
    }

    /// Append messages from outside the loop (voice transcripts) to the history.
    pub async fn append_external(&self, session_id: &str, messages: Vec<Message>) -> anyhow::Result<()> {
        let state = self.state(session_id).await?;
        self.push_messages(session_id, &state, messages).await
    }

    /// The session's todo list (plan-todo.md section 1); empty while it has none.
    pub async fn todo(&self, session_id: &str) -> anyhow::Result<Vec<TodoItem>> {
        Ok(self.state(session_id).await?.meta.lock().await.todo.clone())
    }

    /// Replace the session's todo list with `items`, normalised as `todo_write` does:
    /// in the session's meta, in its file, and as a `todoUpdated` event. A list that is
    /// refused (`todo::normalise`) changes nothing.
    pub async fn set_todo(&self, session_id: &str, items: Vec<TodoItemInput>) -> anyhow::Result<Vec<TodoItem>> {
        let items = crate::todo::normalise(items)?;
        let state = self.state(session_id).await?;
        let mut meta = state.meta.lock().await;
        let before = std::mem::replace(&mut meta.todo, items.clone());
        // A list that is all done ends the approved plan (plan-todo.md section 2).
        let plan = if crate::plan::todo_done(&items) { meta.approved_plan.take() } else { None };
        if let Err(e) = self.store.save_meta(&meta).await {
            meta.todo = before;
            if plan.is_some() {
                meta.approved_plan = plan;
            }
            return Err(e);
        }
        // Sent under the lock that stored the list, so the events reach the UI in the
        // order the lists were stored.
        let _ = self.events.send(AgentEvent::TodoUpdated { session_id: session_id.into(), items: items.clone() });
        Ok(items)
    }

    /// Whether the session is in plan mode (plan-todo.md section 2).
    pub async fn planning(&self, session_id: &str) -> anyhow::Result<bool> {
        Ok(self.state(session_id).await?.meta.lock().await.planning)
    }

    /// Put the session in or out of plan mode and say whether the flag changed. Leaving it
    /// also denies the plan cards of the session that still wait for an answer, so a running
    /// turn goes on in the normal mode instead of waiting for the card of a cancelled plan.
    pub async fn set_planning(&self, session_id: &str, active: bool) -> anyhow::Result<bool> {
        self.change_planning(session_id, active, crate::plan::PLAN_CANCELLED).await
    }

    async fn change_planning(&self, session_id: &str, active: bool, why: &str) -> anyhow::Result<bool> {
        let state = self.state(session_id).await?;
        let mut meta = state.meta.lock().await;
        if meta.planning == active {
            return Ok(false);
        }
        meta.planning = active;
        if let Err(e) = self.store.save_meta(&meta).await {
            meta.planning = !active;
            return Err(e);
        }
        drop(meta);
        // After the file: the engine and the UI never see a flag the file does not have.
        self.permissions.set_planning(session_id, active);
        if !active {
            for request in self.broker.pending() {
                if request.kind == ApprovalKind::Plan && request.session_id == session_id {
                    self.broker.resolve(&request.request_id, ApprovalDecision::Deny { reason: Some(why.to_string()) }, crate::permission::ApprovalVia::Click);
                }
            }
        }
        let _ = self.events.send(AgentEvent::PlanUpdated { session_id: session_id.into(), active });
        Ok(true)
    }

    /// The owner approved `text` as the plan of `session_id` (`plan_exit`): plan mode off,
    /// the plan saved under `plans_dir` and kept in the meta as `approvedPlan`, which the
    /// prompt of every later request of the session carries. Plan mode ends even when the
    /// plan cannot be stored: the owner approved it.
    pub async fn approve_plan(&self, session_id: &str, plans_dir: &Path, text: &str) -> anyhow::Result<crate::plan::ApprovedPlan> {
        self.change_planning(session_id, false, crate::plan::PLAN_SUPERSEDED).await?;
        let path = crate::plan::save(plans_dir, session_id, text).await?;
        let plan = crate::plan::ApprovedPlan { path: path.display().to_string(), text: text.to_string(), approved_at: Some(chrono::Utc::now()) };
        let state = self.state(session_id).await?;
        let mut meta = state.meta.lock().await;
        let before = meta.approved_plan.replace(plan.clone());
        if let Err(e) = self.store.save_meta(&meta).await {
            meta.approved_plan = before;
            return Err(e);
        }
        Ok(plan)
    }

    /// The approved plan the next request carries, read under the meta's lock: one that is
    /// 24 h old is dropped from the meta here (plan-todo.md section 2).
    pub(super) async fn current_plan(&self, state: &SessionState) -> (bool, Option<crate::plan::ApprovedPlan>) {
        let mut meta = state.meta.lock().await;
        if meta.approved_plan.as_ref().is_some_and(|p| p.expired_at(chrono::Utc::now())) {
            let before = meta.approved_plan.take();
            if let Err(e) = self.store.save_meta(&meta).await {
                tracing::warn!(session_id = %meta.id, "the expired plan could not be dropped: {e:#}");
                meta.approved_plan = before;
                return (meta.planning, None);
            }
        }
        (meta.planning, meta.approved_plan.clone())
    }

    /// The approved plan of `session_id` as the next request would carry it.
    pub async fn approved_plan(&self, session_id: &str) -> anyhow::Result<Option<crate::plan::ApprovedPlan>> {
        let state = self.state(session_id).await?;
        Ok(self.current_plan(&state).await.1)
    }

    /// Add messages to the session: the file first, then memory, the meta (count,
    /// title) and the UI events.
    pub(super) async fn push_messages(&self, session_id: &str, state: &SessionState, messages: Vec<Message>) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        self.store.append(session_id, &messages).await?;
        state.messages.lock().await.extend(messages.iter().cloned());
        let mut meta = state.meta.lock().await;
        meta.message_count += messages.len();
        meta.updated_at = chrono::Utc::now();
        if let Some(t) = self.refined.lock().unwrap().remove(session_id) {
            meta.title = t;
        }
        if title::is_placeholder(&meta.title) {
            if let Some(first) = messages.iter().find(|m| m.role == Role::User && title::meaningful(&m.content)) {
                if let Some(q) = title::quick_title(&first.content) {
                    meta.title = q;
                    let _ = self.events.send(AgentEvent::SessionUpdated { session_id: session_id.into(), title: meta.title.clone() });
                    self.refine_title(session_id, first.content.clone());
                }
            }
        }
        self.store.save_meta(&meta).await?;
        drop(meta);
        for m in messages {
            let _ = self.events.send(AgentEvent::MessageAppended { session_id: session_id.into(), message: m });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SessionState {
        let now = chrono::Utc::now();
        SessionState::new(
            SessionMeta {
                id: "s".into(),
                title: String::new(),
                created_at: now,
                updated_at: now,
                message_count: 0,
                origin: Default::default(),
                parent: None,
                summary: None,
                summary_upto: None,
                todo: Vec::new(),
                planning: false,
                approved_plan: None,
                model: None,
                reasoning_level: None,
            },
            vec![],
        )
    }

    #[test]
    fn settle_hands_the_queue_to_a_new_turn_without_an_idle_gap() {
        use crate::protocol::TurnStatus;

        let input = |text: &str, via: Via, suffix: Option<&str>| TurnInput {
            text: text.into(),
            via,
            system_suffix: suffix.map(str::to_string),
        };
        let s = state();
        assert!(s.is_idle());
        let Taken::Turn(first) = s.take(&input("one", Via::Text, None)) else { panic!("nothing runs, so the input starts a turn") };
        assert!(!s.is_idle());
        assert!(matches!(s.take(&input("two", Via::Task, Some("skill block"))), Taken::Queued), "an input sent during a turn is queued");

        let (next, token) = s.settle(TurnResult::failed("first")).expect("the queued input starts the next turn");
        assert_eq!((next.text.as_str(), next.via), ("two", Via::Task));
        assert_eq!(next.system_suffix.as_deref(), Some("skill block"), "the queued input keeps the block of its turn");
        assert!(!s.is_idle(), "the queue goes straight into a new turn");
        first.cancel();
        assert!(!token.is_cancelled(), "the new turn has a token of its own");

        // Both turns settled while nobody listened: a waiter that comes later still
        // gets the last result (found by the real-model run, missed by the first test).
        assert!(s.settle(TurnResult::failed("second")).is_none());
        assert!(s.is_idle());
        let mut settled = s.settled.subscribe();
        let last = settled.borrow_and_update().clone();
        assert_eq!(last.map(|r| (r.status, r.error)), Some((TurnStatus::Failed, Some("second".into()))), "the last turn's result, not the first's");
        assert!(!settled.has_changed().unwrap(), "nothing settled since, so a waiter keeps waiting");
    }

    #[test]
    fn an_idle_session_starts_a_new_turn() {
        let s = state();
        let one = TurnInput { text: "one".into(), via: Via::Text, system_suffix: None };
        let Taken::Turn(_) = s.take(&one) else { panic!() };
        assert!(s.settle(TurnResult::failed("done")).is_none());
        assert!(s.is_idle());
        assert!(matches!(s.take(&one), Taken::Turn(_)));
        assert!(!s.is_idle());
    }
}
