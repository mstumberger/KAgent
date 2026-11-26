//! The loop's side of the hooks on a ready agent (the call sites of `hooks.rs`); the
//! tool calls themselves are in `tests/hooks.rs`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::agent::{AgentConfig, AgentDeps};
use crate::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use crate::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use crate::protocol::{Role, TurnStatus};
use crate::session::SessionStore;
use crate::tool::{PrivilegeMethod, ToolRegistry};

/// The daemon's hooks: what they were asked, and what each event adds.
struct Daemon(Mutex<Vec<String>>);

#[async_trait]
impl AgentHooks for Daemon {
    async fn event(&self, event: HookEvent<'_>) -> Option<String> {
        self.0.lock().unwrap().push(match event {
            HookEvent::SessionStart { session_id, .. } => format!("SessionStart {session_id}"),
            HookEvent::UserPromptSubmit { session_id, .. } => format!("UserPromptSubmit {session_id}"),
            HookEvent::Stop { session_id, turn_id, status, error, .. } => format!("Stop {session_id} {turn_id} {status:?} {}", error.unwrap_or("-")),
        });
        match event {
            HookEvent::SessionStart { .. } => Some("a new session".into()),
            HookEvent::UserPromptSubmit { prompt, .. } => Some(format!("about {prompt}")),
            HookEvent::Stop { turn_id, status, error, .. } => Some(format!("{turn_id} {status:?} {}", error.unwrap_or("-"))),
        }
    }
}

struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        anyhow::bail!("no request is made in these tests")
    }
}

/// An agent with a session, with or without the daemon's hooks.
async fn agent(hooks: Option<Arc<dyn AgentHooks>>) -> (Arc<Agent>, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let session = store.create("t").await.unwrap().id;
    let (etx, _keep) = broadcast::channel(16);
    let cfg = AgentConfig {
        model_ref: "fake::test".into(),
        system_prompt: String::new(),
        max_iterations: 8,
        options: Default::default(),
        privilege: PrivilegeMethod::None,
        context_tokens: 0,
        fallback_model_ref: String::new(),
        fallback_context_tokens: 0,
        fallback_max_output: None,
    };
    let agent = Agent::new(AgentDeps {
        services: None,
        audit: None,
        model: Arc::new(NoModel),
        tools: Arc::new(ToolRegistry::new()),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(5)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
        hooks,
    });
    (agent, session, dir)
}

fn texts(messages: &[Message]) -> Vec<&str> {
    messages.iter().map(|m| m.content.as_str()).collect()
}

#[tokio::test]
async fn session_start_once_then_the_prompt_and_stop_after_the_turn() {
    let daemon = Arc::new(Daemon(Mutex::new(Vec::new())));
    let (agent, session, _dir) = agent(Some(daemon.clone())).await;
    let state = agent.state(&session).await.unwrap();
    let mut budget = CONTEXT_CHARS_PER_TURN;

    // The first turn of the session.
    agent.turn_hooks(&session, &state, "hello", Via::Text).await;
    assert_eq!(*daemon.0.lock().unwrap(), vec![format!("SessionStart {session}"), format!("UserPromptSubmit {session}")]);
    let first = agent.hook_messages(&session, &mut budget);
    assert_eq!(texts(&first), ["[hook SessionStart] a new session", "[hook UserPromptSubmit] about hello"], "stored user messages, in the order the events came");
    assert!(first.iter().all(|m| m.role == Role::User));
    assert!(agent.hook_messages(&session, &mut budget).is_empty(), "taken once");

    // The next turn: the session has a history now.
    state.messages.lock().await.push(Message::user("hello"));
    agent.turn_hooks(&session, &state, "again", Via::Text).await;
    assert_eq!(*daemon.0.lock().unwrap(), vec![format!("SessionStart {session}"), format!("UserPromptSubmit {session}"), format!("UserPromptSubmit {session}")], "SessionStart comes before the first turn only");
    assert_eq!(texts(&agent.hook_messages(&session, &mut budget)), ["[hook UserPromptSubmit] about again"]);
    let left = budget;
    assert!(left < CONTEXT_CHARS_PER_TURN && left > 0, "the turn's budget is spent by its messages: {left} left");

    // Stop: after the turn, with how it ended. Its context waits for the next turn.
    agent.stop_hooks(&session, "turn-1", Via::Voice, TurnStatus::Failed, Some("boom")).await;
    assert_eq!(daemon.0.lock().unwrap().last().unwrap(), &format!("Stop {session} turn-1 Failed boom"));
    assert_eq!(texts(&agent.hook_messages(&session, &mut budget)), ["[hook Stop] turn-1 Failed boom"]);
}

#[test]
fn the_context_of_a_turn_is_capped_on_a_character_boundary() {
    let mut budget = CONTEXT_CHARS_PER_TURN;
    assert_eq!(take(&mut budget, &"a".repeat(3_000)), Some("a".repeat(3_000)));
    assert_eq!(take(&mut budget, &"é".repeat(2_000)), Some("é".repeat(1_000)), "what is left of the budget");
    assert_eq!(budget, 0);
    assert_eq!(take(&mut budget, "more"), None);
    let mut budget = 5;
    assert_eq!(take(&mut budget, "  éééééééé "), Some("ééééé".to_string()), "whole characters only");
}

#[tokio::test]
async fn contexts_are_kept_apart_by_session() {
    let (agent, session, _dir) = agent(None).await;
    assert!(agent.hooks.daemon.is_none(), "no daemon hooks");
    agent.state(&session).await.unwrap();
    let other = agent.store().create("other").await.unwrap().id;
    agent.state(&other).await.unwrap();
    agent.add_hook_context(&session, [("Stop", "one".to_string())]);
    agent.add_hook_context(&other, [("PostToolUse", "two".to_string()), ("PostToolUse", "  ".to_string())]);
    let mut budget = CONTEXT_CHARS_PER_TURN;
    assert_eq!(texts(&agent.hook_messages(&other, &mut budget)), ["[hook PostToolUse] two"], "blank context is not stored");
    assert!(agent.hook_messages(&other, &mut budget).is_empty(), "taken once");
    assert_eq!(texts(&agent.hook_messages(&session, &mut budget)), ["[hook Stop] one"]);
    // A session the agent forgot takes no context anymore.
    agent.forget(&session);
    agent.add_hook_context(&session, [("Stop", "late".to_string())]);
    assert!(agent.hook_messages(&session, &mut budget).is_empty());
}
