//! A background `shell` call in the loop (work-windows.md section 7, scenario 3): the events of the task reach
//! the stream, the list of the session holds its command, the end is said before the notification of S31, and the
//! kill switch says `killed`. A scripted model, a registry of its own.

mod common;
use common::*;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::broadcast;

use kagent_core::agent::{Agent, AgentDeps, SendOutcome};
use kagent_core::model::ModelResponse;
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{AgentEvent, Role, ToolCall, TurnStatus};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;
use kagent_core::tools::background::Background;
use kagent_core::tools::shell::Shell;

struct World {
    agent: Arc<Agent>,
    bg: Background,
    store: SessionStore,
    events: broadcast::Receiver<AgentEvent>,
    session: String,
    _dir: tempfile::TempDir,
}

async fn world(script: Vec<ModelResponse>) -> World {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let meta = store.create("t").await.unwrap();
    let (etx, erx) = broadcast::channel(256);
    let bg = Background::new(dir.path().join("out"));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Shell::with_tasks(bg.clone())));
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(ScriptedModel::new(script)),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Full, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx.clone(),
        config: Arc::new(|_| config()),
        on_rules_changed: None,
    });
    // What the daemon does (subagents/runner.rs): the events of the tasks go to the stream, a task that ended tells its session.
    let tell = agent.clone();
    bg.configure(dir.path().join("out"), Arc::new(move |session: String, text: String| {
        let tell = tell.clone();
        tokio::spawn(async move {
            let _ = tell.notify(&session, &text).await;
        });
    }));
    bg.set_event_sink(Arc::new(move |id, session, status| {
        let _ = etx.send(AgentEvent::BackgroundTask { task_id: id.into(), session_id: session.into(), status: status.label().into() });
    }));
    World { agent, bg, store, events: erx, session: meta.id, _dir: dir }
}

fn shell_call(command: &str) -> ModelResponse {
    ModelResponse { tool_calls: vec![ToolCall { id: "call_bg".into(), name: "shell".into(), input: json!({"command": command, "background": true}) }], ..Default::default() }
}

fn say(text: &str) -> ModelResponse {
    ModelResponse { text: text.into(), ..Default::default() }
}

fn task_event(ev: &AgentEvent, status: &str) -> bool {
    matches!(ev, AgentEvent::BackgroundTask { status: s, .. } if s == status)
}

#[tokio::test]
async fn a_background_shell_call_emits_running_and_the_end_before_the_notification() {
    let mut w = world(vec![shell_call("echo from-the-task"), say("started"), say("noted")]).await;
    let SendOutcome::Started { handle, .. } = w.agent.send(&w.session, "run it".into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    assert_eq!(handle.wait().await.status, TurnStatus::Completed);
    let running = wait_event(&mut w.events, 5, |e| task_event(e, "running")).await;
    let AgentEvent::BackgroundTask { task_id, session_id, .. } = running else { unreachable!() };
    assert_eq!(session_id, w.session);
    let ended = wait_event(&mut w.events, 10, |e| task_event(e, "done")).await;
    assert!(matches!(&ended, AgentEvent::BackgroundTask { task_id: t, .. } if *t == task_id));
    // The list of the session holds the command and its final state.
    let list = w.bg.list(&w.session);
    assert_eq!((list.len(), list[0].command.as_str(), list[0].id.as_str()), (1, "echo from-the-task", task_id.as_str()));
    assert_eq!(serde_json::to_value(&list[0]).unwrap()["status"], "done");
    // The notification of S31 reaches the session after the event of the end.
    let end = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let stored = w.store.load_messages(&w.session).await.unwrap();
        if stored.iter().any(|m| m.role == Role::User && m.content.contains(&format!("[Task {task_id} finished, exit 0]"))) {
            break;
        }
        assert!(std::time::Instant::now() < end, "the session is told how the task ended");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_kill_switch_says_killed_and_tells_no_session() {
    let mut w = world(vec![shell_call("sleep 40"), say("started")]).await;
    let SendOutcome::Started { handle, .. } = w.agent.send(&w.session, "run it".into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    assert_eq!(handle.wait().await.status, TurnStatus::Completed);
    wait_event(&mut w.events, 5, |e| task_event(e, "running")).await;
    assert_eq!(w.bg.list(&w.session)[0].command, "sleep 40");
    assert_eq!(w.bg.stop_all(), 1);
    wait_event(&mut w.events, 5, |e| task_event(e, "killed")).await;
    assert_eq!(serde_json::to_value(&w.bg.list(&w.session)[0]).unwrap()["status"], "killed");
    // Nothing more is said about it: no second event, no message.
    tokio::time::sleep(Duration::from_millis(600)).await;
    while let Ok(ev) = w.events.try_recv() {
        assert!(!matches!(ev, AgentEvent::BackgroundTask { .. }), "{ev:?}");
    }
    let stored = w.store.load_messages(&w.session).await.unwrap();
    assert!(!stored.iter().any(|m| m.role == Role::User && m.content.contains("finished")), "a killed task tells nobody");
}
