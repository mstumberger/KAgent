//! Awaiting turns (S17): the handle of a started turn, and `Agent::wait_idle` for
//! callers that only hold a session id.

use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

use crate::protocol::{TurnStatus, Usage};

use super::Agent;

/// How a turn ended. `text` is the turn's text deltas joined, also when the turn was
/// cancelled or failed, so callers keep partial answers.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub status: TurnStatus,
    pub error: Option<String>,
    pub text: String,
    pub usage: Usage,
}

impl TurnResult {
    pub(super) fn failed(error: impl Into<String>) -> Self {
        Self { status: TurnStatus::Failed, error: Some(error.into()), text: String::new(), usage: Usage::default() }
    }
}

/// One turn's shared state: its text as it streams and its result once it has ended.
#[derive(Default)]
pub(super) struct TurnCell {
    pub text: Arc<Mutex<String>>,
    pub result: Mutex<Option<TurnResult>>,
}

/// Wait for one started turn.
pub struct TurnHandle {
    cell: Arc<TurnCell>,
    /// Sent (or dropped with the turn's task) once the turn is over.
    done: oneshot::Receiver<()>,
}

impl std::fmt::Debug for TurnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnHandle").finish()
    }
}

impl TurnHandle {
    /// A handle sharing a fresh cell: the handle, the sender the turn's task signals
    /// (or drops) when the turn is over, and the cell the task records into.
    pub(super) fn new() -> (Self, oneshot::Sender<()>, Arc<TurnCell>) {
        let (done, rx) = oneshot::channel();
        let cell = Arc::new(TurnCell::default());
        (Self { cell: cell.clone(), done: rx }, done, cell)
    }

    /// Resolves exactly once: when the turn ends, panics or is cancelled by
    /// `stop_all`, and when its task is dropped at shutdown (then `Cancelled`, with
    /// the text that streamed so far).
    pub async fn wait(self) -> TurnResult {
        let _ = self.done.await;
        let mut result = self.cell.result.lock().unwrap();
        result.take().unwrap_or_else(|| {
            let text = std::mem::take(&mut *self.cell.text.lock().unwrap());
            TurnResult {
                status: TurnStatus::Cancelled,
                error: Some("the turn was aborted before it finished".into()),
                text,
                usage: Usage::default(),
            }
        })
    }
}

impl Agent {
    /// Resolve once the session has no running turn and nothing queued, with the
    /// result of its last turn (`Failed` "no turn has run" when there was none).
    pub async fn wait_idle(&self, session_id: &str) -> TurnResult {
        let state = match self.state(session_id).await {
            Ok(s) => s,
            Err(e) => return TurnResult::failed(format!("{e:#}")),
        };
        let mut settled = state.settled.subscribe();
        loop {
            if state.is_idle() {
                let last: Option<TurnResult> = settled.borrow().clone();
                return last.unwrap_or_else(|| TurnResult::failed("no turn has run in this session"));
            }
            // Every settled turn publishes here; the sender lives as long as `state`.
            if settled.changed().await.is_err() {
                return TurnResult::failed("the session was dropped while waiting");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{json, Value};
    use tokio::sync::{broadcast, mpsc, Notify};

    use super::*;
    use crate::agent::{AgentConfig, AgentDeps, SendOutcome};
    use crate::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
    use crate::permission::{PermissionEngine, PermissionMode, PermissionBroker};
    use crate::protocol::{AgentEvent, ToolCall, TurnStatus, Via};
    use crate::session::SessionStore;
    use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry};
    use tokio_util::sync::CancellationToken;

    /// What the fake model does on one request.
    enum Step {
        /// Stream these deltas, then answer.
        Say { deltas: Vec<&'static str>, response: ModelResponse },
        /// Stream, wait for `release`, then answer.
        Gated { deltas: Vec<&'static str>, response: ModelResponse, release: Arc<Notify> },
        /// Stream, then block until the turn is cancelled.
        Hold { deltas: Vec<&'static str> },
        Fail(&'static str),
    }

    struct FakeModel(std::sync::Mutex<Vec<Step>>);

    #[async_trait]
    impl ModelClient for FakeModel {
        async fn complete(
            &self,
            _request: ModelRequest,
            deltas: mpsc::UnboundedSender<ModelDelta>,
            cancel: CancellationToken,
        ) -> anyhow::Result<ModelResponse> {
            let step = self.0.lock().unwrap().remove(0);
            let stream = |parts: &[&'static str]| {
                for p in parts {
                    let _ = deltas.send(ModelDelta::Text((*p).to_string()));
                }
            };
            match step {
                Step::Say { deltas: parts, response } => {
                    stream(&parts);
                    Ok(response)
                }
                Step::Gated { deltas: parts, response, release } => {
                    stream(&parts);
                    release.notified().await;
                    Ok(response)
                }
                Step::Hold { deltas: parts } => {
                    stream(&parts);
                    cancel.cancelled().await;
                    Err(anyhow::anyhow!("cancelled"))
                }
                Step::Fail(message) => Err(anyhow::anyhow!(message)),
            }
        }
    }

    /// A tool whose future panics.
    struct PanicTool(ToolContract);

    #[async_trait]
    impl Tool for PanicTool {
        fn contract(&self) -> &ToolContract {
            &self.0
        }
        async fn call(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
            panic!("tool exploded")
        }
    }

    struct Rig {
        agent: Arc<Agent>,
        session: String,
        events: broadcast::Receiver<AgentEvent>,
        _dir: tempfile::TempDir,
    }

    async fn rig(steps: Vec<Step>, tools: Vec<Arc<dyn Tool>>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let session = store.create("t").await.unwrap().id;
        let (etx, erx) = broadcast::channel(256);
        let mut registry = ToolRegistry::new();
        for t in tools {
            registry.register(t);
        }
        let cfg = AgentConfig {
            model_ref: "fake::test".into(),
            system_prompt: String::new(),
            max_iterations: 8,
            options: Default::default(),
            privilege: crate::tool::PrivilegeMethod::None,
            context_tokens: 0,
            fallback_model_ref: String::new(),
            fallback_context_tokens: 0,
            fallback_max_output: None,
        };
        let agent = Agent::new(AgentDeps {
            services: None,
            audit: None,
            model: Arc::new(FakeModel(std::sync::Mutex::new(steps))),
            tools: Arc::new(registry),
            permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
            broker: PermissionBroker::new(etx.clone(), Duration::from_secs(30)),
            store,
            events: etx,
            config: Arc::new(move |_| cfg.clone()),
            on_rules_changed: None,
        });
        Rig { agent, session, events: erx, _dir: dir }
    }

    async fn started(agent: &Arc<Agent>, session: &str, input: &str) -> TurnHandle {
        match agent.send(session, input.into(), Via::Text).await.unwrap() {
            SendOutcome::Started { handle, .. } => handle,
            SendOutcome::Queued => panic!("nothing runs, so the input must start a turn"),
        }
    }

    async fn wait_event(events: &mut broadcast::Receiver<AgentEvent>, pred: impl Fn(&AgentEvent) -> bool) {
        let ev = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let ev = events.recv().await.expect("event channel open");
                if pred(&ev) {
                    break;
                }
            }
        })
        .await;
        assert!(ev.is_ok(), "event deadline");
    }

    async fn wait_finished(events: &mut broadcast::Receiver<AgentEvent>, status: TurnStatus) {
        wait_event(events, move |e| matches!(e, AgentEvent::TurnFinished { status: s, .. } if *s == status)).await;
    }

    #[tokio::test]
    async fn wait_resolves_with_each_status() {
        let Rig { agent, session: sid, mut events, _dir } = rig(
            vec![
                Step::Say { deltas: vec!["Hel", "lo"], response: ModelResponse { text: "Hello".into(), ..Default::default() } },
                Step::Fail("provider down"),
                Step::Hold { deltas: vec!["part ", "ial"] },
            ],
            vec![],
        )
        .await;

        let r = started(&agent, &sid, "q1").await.wait().await;
        assert_eq!(r.status, TurnStatus::Completed);
        assert_eq!((r.text.as_str(), r.error.as_deref()), ("Hello", None));
        wait_finished(&mut events, TurnStatus::Completed).await;

        let r = started(&agent, &sid, "q2").await.wait().await;
        assert_eq!(r.status, TurnStatus::Failed);
        assert!(r.error.as_deref().unwrap().contains("provider down"));
        wait_finished(&mut events, TurnStatus::Failed).await;

        // Scenario S17 item 1: cancelled after two text deltas.
        let handle = started(&agent, &sid, "q3").await;
        wait_event(&mut events, |e| matches!(e, AgentEvent::TextDelta { text, .. } if text == "ial")).await;
        assert!(agent.cancel(&sid).await);
        let r = handle.wait().await;
        assert_eq!(r.status, TurnStatus::Cancelled);
        assert_eq!((r.text.as_str(), r.error.as_deref()), ("part ial", None));
        wait_finished(&mut events, TurnStatus::Cancelled).await;
        assert!(!agent.is_running(&sid).await);
    }

    #[tokio::test]
    async fn a_panicking_tool_fails_the_turn_and_the_session_recovers() {
        let tool = Arc::new(PanicTool(ToolContract::read_only("boom", "always panics", json!({"type": "object"}))));
        let Rig { agent, session: sid, mut events, _dir } = rig(
            vec![
                Step::Say {
                    deltas: vec!["looking"],
                    response: ModelResponse {
                        tool_calls: vec![ToolCall { id: "c1".into(), name: "boom".into(), input: json!({}) }],
                        ..Default::default()
                    },
                },
                Step::Say { deltas: vec!["fine"], response: ModelResponse { text: "fine".into(), ..Default::default() } },
            ],
            vec![tool],
        )
        .await;

        // Scenario S17 item 2: a tool future that panics.
        let r = started(&agent, &sid, "q1").await.wait().await;
        assert_eq!(r.status, TurnStatus::Failed);
        assert_eq!(r.error.as_deref(), Some("internal error (see log)"));
        assert_eq!(r.text, "looking", "text streamed before the panic is kept");
        wait_finished(&mut events, TurnStatus::Failed).await;
        assert!(!agent.is_running(&sid).await, "the panic must not leave the session stuck");

        // The same session runs a next turn, awaited by session id only.
        let second = started(&agent, &sid, "q2").await;
        let r = tokio::time::timeout(Duration::from_secs(10), agent.wait_idle(&sid)).await.unwrap();
        assert_eq!(r.status, TurnStatus::Completed);
        assert_eq!(r.text, "fine");
        // And a caller that asks only now, after both turns settled, gets that result.
        let _ = second.wait().await;
        assert_eq!(agent.wait_idle(&sid).await.text, "fine", "a result settled with nobody waiting is not lost");
    }

    #[tokio::test]
    async fn wait_idle_returns_after_a_queued_second_turn() {
        let release = Arc::new(Notify::new());
        let Rig { agent, session: sid, mut events, _dir } = rig(
            vec![
                Step::Gated { deltas: vec!["one "], response: ModelResponse { text: "one".into(), ..Default::default() }, release: release.clone() },
                Step::Say { deltas: vec!["two"], response: ModelResponse { text: "two".into(), ..Default::default() } },
            ],
            vec![],
        )
        .await;
        let r = agent.wait_idle(&sid).await;
        let error = r.error.clone().unwrap_or_default();
        assert_eq!((r.status, error.as_str()), (TurnStatus::Failed, "no turn has run in this session"));

        let first = started(&agent, &sid, "q1").await;
        // The first model request is in progress (the model streamed and now waits)...
        wait_event(&mut events, |e| matches!(e, AgentEvent::TextDelta { text, .. } if text == "one ")).await;
        // ...so the next input is queued behind it, not merged into it.
        assert!(matches!(agent.send(&sid, "q2".into(), Via::Text).await.unwrap(), SendOutcome::Queued));
        let (waiting_agent, waiting_session) = (agent.clone(), sid.clone());
        let idle = tokio::spawn(async move { waiting_agent.wait_idle(&waiting_session).await });
        tokio::task::yield_now().await;
        assert!(!idle.is_finished(), "wait_idle resolved while a turn ran with input queued");

        release.notify_one();
        first.wait().await;
        let r = idle.await.unwrap();
        assert_eq!(r.status, TurnStatus::Completed);
        assert_eq!(r.text, "one two", "the turn ends only after the queued input was answered");
        assert!(!agent.is_running(&sid).await);
    }

    #[tokio::test]
    async fn wait_resolves_when_the_turn_task_is_dropped() {
        let (handle, done, cell) = TurnHandle::new();
        cell.text.lock().unwrap().push_str("half a sen");
        let waiter = tokio::spawn(handle.wait());
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        // What a runtime shutdown does to the turn's task: its locals are dropped.
        drop(done);
        let r = waiter.await.unwrap();
        assert_eq!(r.status, TurnStatus::Cancelled);
        assert_eq!(r.text, "half a sen");
    }
}
