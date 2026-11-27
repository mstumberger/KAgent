//! The tests of plan-todo.md section 3 (the todo items) and scenarios.md S23 items 1 and
//! 2. No model is asked: the tools run through `execute_call`, as they do in a turn.

use std::path::Path;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::agent::{AgentConfig, AgentDeps};
use crate::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use crate::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use crate::protocol::{AgentEvent, Message, ToolCall, ToolCallStatus};
use crate::session::SessionStore;
use crate::tool::{PrivilegeMethod, ToolRegistry};

/// Never asked: no test here runs a turn.
struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        anyhow::bail!("no model in this test")
    }
}

/// An agent with only the two todo tools (the daemon registers them the same way), over
/// the sessions of `dir`. A second one over the same directory is the daemon after a
/// restart.
fn agent_over(dir: &Path, mode: PermissionMode) -> (Arc<Agent>, broadcast::Sender<AgentEvent>) {
    let store = SessionStore::new(dir.join("sessions")).unwrap();
    let access = TodoAccess::default();
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(TodoWrite::new(access.clone())));
    registry.register(Arc::new(TodoRead::new(access.clone())));
    registry.register(Arc::new(crate::tools::terminal::Terminal::new()));
    let (events, _) = broadcast::channel(64);
    let cfg = AgentConfig {
        model_ref: String::new(),
        system_prompt: String::new(),
        max_iterations: 4,
        options: Default::default(),
        privilege: PrivilegeMethod::None,
        context_tokens: 0,
        fallback_model_ref: String::new(),
        fallback_context_tokens: 0,
        fallback_max_output: None,
    };
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(NoModel),
        tools: Arc::new(registry),
        permissions: Arc::new(PermissionEngine::new(mode, vec![])),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
        store,
        events: events.clone(),
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    access.attach(&agent);
    (agent, events)
}

/// One call as a turn makes it (`execute_call`): the text of its result, whether that is
/// an error, and its status.
async fn call(agent: &Arc<Agent>, session: &str, name: &str, input: Value) -> (String, bool, ToolCallStatus) {
    let call = ToolCall { id: "c1".into(), name: name.into(), input };
    let (msg, status) = agent.execute_call(session, "turn", &call, &CancellationToken::new(), PrivilegeMethod::None).await;
    (msg.content, msg.is_error, status)
}

/// The events until the channel is empty: their types, and the `todoUpdated` ones whole.
async fn drained(rx: &mut broadcast::Receiver<AgentEvent>) -> (Vec<String>, Vec<Value>) {
    let (mut types, mut todos) = (Vec::new(), Vec::new());
    while let Ok(ev) = rx.try_recv() {
        let v = serde_json::to_value(&ev).unwrap();
        if v["type"] == "todoUpdated" {
            todos.push(v.clone());
        }
        types.push(v["type"].as_str().unwrap().to_string());
    }
    (types, todos)
}

/// An item as the model or the panel sends it; `None` leaves the field out.
fn input(content: &str, status: Option<&str>, priority: Option<&str>) -> Value {
    json!({"content": content, "status": status, "priority": priority})
}

fn stored(content: &str, status: TodoStatus, priority: TodoPriority) -> TodoItem {
    TodoItem { content: content.into(), status, priority }
}

/// Scenario S23 item 1 and the first test of plan-todo.md section 3.
#[tokio::test]
async fn todo_write_normalises_the_list_and_emits_todo_updated() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, events) = agent_over(dir.path(), PermissionMode::Ask);
    let id = agent.store().create("t").await.unwrap().id;
    let mut rx = events.subscribe();
    let list = json!({"items": [
        input("Check disk", Some("in_progress"), Some("high")),
        input("Check memory", Some("in_progress"), None),
        input("  Summarize  ", None, Some("low"))
    ]});
    let (text, is_error, status) = call(&agent, &id, "todo_write", list).await;
    let want = vec![
        stored("Check disk", TodoStatus::InProgress, TodoPriority::High),
        stored("Check memory", TodoStatus::Pending, TodoPriority::Medium),
        stored("Summarize", TodoStatus::Pending, TodoPriority::Low),
    ];
    assert_eq!((status, is_error), (ToolCallStatus::Ok, false), "{text}");
    assert_eq!(text, "[1] [>] Check disk (high)\n[2] [ ] Check memory\n[3] [ ] Summarize", "the result lists every item");
    assert_eq!(agent.store().meta(&id).await.unwrap().todo, want, "the second in_progress item is pending, a missing priority is medium and a missing status pending");
    assert_eq!(agent.todo(&id).await.unwrap(), want, "the accessor the daemon reads");
    let (types, todos) = drained(&mut rx).await;
    assert_eq!(
        todos,
        vec![json!({"type": "todoUpdated", "sessionId": id, "items": [
            {"content": "Check disk", "status": "in_progress", "priority": "high"},
            {"content": "Check memory", "status": "pending", "priority": "medium"},
            {"content": "Summarize", "status": "pending", "priority": "low"}
        ]})],
        "one event after the change, with the whole list"
    );
    assert_eq!(types, ["toolCallStarted", "todoUpdated", "toolCallFinished"], "no approval was asked in ask mode");
}

/// The round trip through the meta, and scenario S23 item 2 (the list is still there
/// after a restart).
#[tokio::test]
async fn the_list_round_trips_through_the_meta_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, _events) = agent_over(dir.path(), PermissionMode::Ask);
    let store = agent.store();
    let id = store.create("t").await.unwrap().id;
    let list = vec![stored("Check disk", TodoStatus::InProgress, TodoPriority::High), stored("Summarize", TodoStatus::Pending, TodoPriority::Medium)];
    let sent = json!([{"content": "Check disk", "status": "in_progress", "priority": "high"}, {"content": "Summarize"}]);
    assert_eq!(agent.set_todo(&id, serde_json::from_value(sent).unwrap()).await.unwrap(), list);
    assert_eq!(store.meta(&id).await.unwrap().todo, list, "saved with the meta");
    agent.append_external(&id, vec![Message::user("go on")]).await.unwrap();
    assert_eq!(store.meta(&id).await.unwrap().todo, list, "a later save of the meta (a message of the turn) keeps the list");

    // A meta file from before the field existed.
    let old = r#"{"id":"old","title":"Old","createdAt":"2026-11-01T10:00:00Z","updatedAt":"2026-11-01T10:00:00Z"}"#;
    std::fs::write(dir.path().join("sessions").join("old.meta.json"), old).unwrap();
    assert!(store.meta("old").await.unwrap().todo.is_empty(), "loads with no list");

    drop(agent);
    let (restarted, _events) = agent_over(dir.path(), PermissionMode::Ask);
    assert_eq!(restarted.todo(&id).await.unwrap(), list, "the daemon after a restart finds the list in the meta");
    let (text, is_error, _) = call(&restarted, &id, "todo_read", json!({})).await;
    assert_eq!((is_error, text.as_str()), (false, "[1] [>] Check disk (high)\n[2] [ ] Summarize"), "and the model reads it back");
}

#[tokio::test]
async fn more_than_50_items_or_a_long_item_is_an_error_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, events) = agent_over(dir.path(), PermissionMode::Ask);
    let id = agent.store().create("t").await.unwrap().id;
    let mut rx = events.subscribe();
    let steps = |n: usize| json!({"items": (0..n).map(|i| json!({"content": format!("step {i}")})).collect::<Vec<_>>()});
    let (text, is_error, status) = call(&agent, &id, "todo_write", steps(51)).await;
    assert_eq!((status, is_error), (ToolCallStatus::Error, true));
    assert!(text.contains("51 items") && text.contains("at most 50"), "{text}");
    let (text, is_error, _) = call(&agent, &id, "todo_write", json!({"items": [{"content": "č".repeat(201)}]})).await;
    assert!(is_error && text.contains("item 1 of the todo list has 201 characters") && text.contains("at most 200"), "characters, not bytes: {text}");
    let (text, is_error, _) = call(&agent, &id, "todo_write", json!({"items": [{"content": "   "}]})).await;
    assert!(is_error && text.contains("item 1 of the todo list has no content"), "{text}");
    assert!(agent.todo(&id).await.unwrap().is_empty(), "a refused list changes nothing");
    assert!(drained(&mut rx).await.1.is_empty(), "and sends no event");

    // The limits themselves are kept: 50 items, and 200 characters (400 bytes here).
    let (text, is_error, _) = call(&agent, &id, "todo_write", json!({"items": [{"content": "č".repeat(200)}]})).await;
    assert!(!is_error, "{text}");
    let (text, is_error, _) = call(&agent, &id, "todo_write", steps(50)).await;
    let listed = text.lines().take_while(|l| !l.is_empty()).count();
    assert!(!is_error && listed == 50, "{listed} lines");
    assert!(text.contains("Removed 1 open item(s)"), "the long item it replaced is named: {text}");
}

#[tokio::test]
async fn read_only_mode_runs_both_tools_without_an_approval() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, events) = agent_over(dir.path(), PermissionMode::ReadOnly);
    let id = agent.store().create("t").await.unwrap().id;
    let mut rx = events.subscribe();
    let (text, is_error, status) = call(&agent, &id, "todo_write", json!({"items": [{"content": "Check disk", "status": "in_progress"}]})).await;
    assert_eq!((status, is_error), (ToolCallStatus::Ok, false), "{text}");
    let (text, is_error, status) = call(&agent, &id, "todo_read", json!({})).await;
    assert_eq!((status, is_error), (ToolCallStatus::Ok, false));
    assert_eq!(text, "[1] [>] Check disk");
    let (types, todos) = drained(&mut rx).await;
    assert!(!types.iter().any(|t| t == "approvalRequested"), "no card: {types:?}");
    assert_eq!(todos.len(), 1, "the list was written");
}

/// Enforced, not only hidden: with the `agent` group off in settings, the model does not
/// see the tools, and a call it remembered is refused.
#[tokio::test]
async fn a_tool_of_the_agent_group_is_refused_when_the_group_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, events) = agent_over(dir.path(), PermissionMode::Ask);
    let id = agent.store().create("t").await.unwrap().id;
    let mut rx = events.subscribe();
    agent.set_tool_groups(Some(vec!["system".to_string(), "agent".to_string()]));
    let names: Vec<String> = agent.advertised_specs().into_iter().map(|t| t.name).collect();
    assert_eq!(names, ["todo_read", "todo_write"], "the group switches both tools on");
    agent.set_tool_groups(Some(vec!["system".to_string()]));
    assert!(agent.advertised_specs().is_empty(), "and off: neither is offered");
    let (text, _, status) = call(&agent, &id, "todo_write", json!({"items": [{"content": "sneaked in"}]})).await;
    assert_eq!(status, ToolCallStatus::Denied);
    assert!(text.contains("tool 'todo_write' is disabled (group 'agent' is off in settings)"), "{text}");
    assert!(agent.todo(&id).await.unwrap().is_empty(), "refused, so the list is unchanged");
    assert!(drained(&mut rx).await.1.is_empty(), "and no event went out");
}

#[tokio::test]
async fn todo_read_answers_the_list_or_no_todo_list() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, events) = agent_over(dir.path(), PermissionMode::Ask);
    let id = agent.store().create("t").await.unwrap().id;
    let mut rx = events.subscribe();
    let (text, is_error, _) = call(&agent, &id, "todo_read", json!({})).await;
    assert_eq!((is_error, text.as_str()), (false, NO_LIST), "before the first list");
    call(&agent, &id, "todo_write", json!({"items": [{"content": "Check disk", "status": "completed", "priority": "low"}, {"content": "Summarize"}]})).await;
    let (text, is_error, _) = call(&agent, &id, "todo_read", json!({})).await;
    assert_eq!((is_error, text.as_str()), (false, "[1] [x] Check disk\n[2] [ ] Summarize"), "only a high item shows its priority");

    // An empty list clears it.
    let (text, is_error, _) = call(&agent, &id, "todo_write", json!({"items": []})).await;
    assert_eq!((is_error, text.as_str()), (false, NO_LIST));
    assert!(agent.todo(&id).await.unwrap().is_empty());
    let (_, todos) = drained(&mut rx).await;
    assert_eq!(todos.len(), 2, "one event after each of the two changes");
    assert_eq!(todos[1]["items"], json!([]), "the last one carries the empty list");

    // A session that does not exist.
    let (text, is_error, _) = call(&agent, "no-such-session", "todo_read", json!({})).await;
    assert!(is_error && text.contains("there is no session 'no-such-session'"), "{text}");
    // Without `items` nothing is written: the loop's validation refuses the call first.
    let (text, is_error, status) = call(&agent, &id, "todo_write", json!({})).await;
    assert_eq!((status, is_error), (ToolCallStatus::Error, true));
    assert!(text.contains("items"), "{text}");
}

#[test]
fn an_item_takes_its_three_fields_and_a_misspelt_one_is_refused() {
    let items: Vec<TodoItemInput> = serde_json::from_value(json!([{"content": "a", "status": null, "priority": null}, {"content": "b"}])).unwrap();
    assert_eq!(
        normalise(items).unwrap(),
        vec![stored("a", TodoStatus::Pending, TodoPriority::Medium), stored("b", TodoStatus::Pending, TodoPriority::Medium)],
        "left out or null: pending and medium"
    );
    assert!(serde_json::from_value::<Vec<TodoItemInput>>(json!([{"content": "a", "stat": "done"}])).is_err(), "a misspelt field is refused, not ignored");
    assert!(serde_json::from_value::<Vec<TodoItemInput>>(json!([{"content": "a", "status": "done"}])).is_err(), "and a status that does not exist");
    assert!(serde_json::from_value::<Vec<TodoItemInput>>(json!([{"status": "pending"}])).is_err(), "content is required");
    assert!(normalise(vec![]).unwrap().is_empty(), "an empty list clears this one");

    // A stored list is the JSON of api.md `TodoItem`, and the event carries it as `items`.
    let one = vec![stored("Check disk", TodoStatus::InProgress, TodoPriority::High)];
    assert_eq!(serde_json::to_value(&one).unwrap(), json!([{"content": "Check disk", "status": "in_progress", "priority": "high"}]));
    let event = AgentEvent::TodoUpdated { session_id: "s".into(), items: one };
    assert_eq!(serde_json::to_value(&event).unwrap(), json!({"type": "todoUpdated", "sessionId": "s", "items": [{"content": "Check disk", "status": "in_progress", "priority": "high"}]}));
}

/// The voice session of 2026-11-27: the owner added three items one at a time, the model
/// sent only the new one each time and the list kept only the last. Mode `add` appends,
/// and a replace that drops open items names them.
#[tokio::test]
async fn mode_add_appends_and_a_replace_that_drops_open_items_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, _events) = agent_over(dir.path(), PermissionMode::Ask);
    let id = agent.store().create("t").await.unwrap().id;
    let (text, ..) = call(&agent, &id, "todo_write", json!({"items": [input("Voice control", None, None)]})).await;
    assert_eq!(text, "[1] [ ] Voice control", "a first replace of an empty list drops nothing");
    let (text, is_error, _) = call(&agent, &id, "todo_write", json!({"mode": "add", "items": [input("Settings in the sidebar", None, Some("high"))]})).await;
    assert!(!is_error, "{text}");
    assert_eq!(text, "[1] [ ] Voice control\n[2] [ ] Settings in the sidebar (high)", "added after the item already there");
    let (text, ..) = call(&agent, &id, "todo_write", json!({"items": [input("Provider errors as a toast", None, None)]})).await;
    assert!(text.starts_with("[1] [ ] Provider errors as a toast\n\nRemoved 2 open item(s)"), "{text}");
    assert!(text.contains("\"Voice control\"") && text.contains("\"Settings in the sidebar\"") && text.contains("mode \"add\""), "{text}");
    // A completed item that goes is not named.
    call(&agent, &id, "todo_write", json!({"items": [input("Done thing", Some("completed"), None), input("Open thing", None, None)]})).await;
    let (text, ..) = call(&agent, &id, "todo_write", json!({"items": [input("Open thing", None, None)]})).await;
    assert_eq!(text, "[1] [ ] Open thing");
    let (_, is_error, _) = call(&agent, &id, "todo_write", json!({"mode": "append", "items": []})).await;
    assert!(is_error, "an unknown mode is refused");
}

/// 2026-11-27, 22:03: in `full` mode the live voice model typed into the owner's only
/// terminal 25 times without a question. From voice an `always_ask` call is confirmed
/// first in every mode; a read-only call is not.
#[tokio::test]
async fn from_voice_an_always_ask_call_is_confirmed_in_full_mode_too() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, _events) = agent_over(dir.path(), PermissionMode::Full);
    let id = agent.store().create("t").await.unwrap().id;
    let call = |input: Value| ToolCall { id: "c".into(), name: "terminal".into(), input };
    let (a, ask) = agent.voice_gate(&id, &call(json!({"action": "send", "target": "konsole", "text": "systemctl --failed"}))).unwrap();
    assert!(a.always_ask && ask, "typing into a terminal is asked out loud, full mode or not");
    assert!(a.summary.contains("systemctl --failed"), "the question names what would be typed: {}", a.summary);
    let (_, ask) = agent.voice_gate(&id, &call(json!({"action": "list"}))).unwrap();
    assert!(!ask, "listing the terminals changes nothing");
    let (_, ask) = agent.voice_gate(&id, &ToolCall { id: "c".into(), name: "todo_read".into(), input: json!({}) }).unwrap();
    assert!(!ask);
    assert!(agent.voice_gate(&id, &ToolCall { id: "c".into(), name: "nope".into(), input: json!({}) }).is_none());
}
