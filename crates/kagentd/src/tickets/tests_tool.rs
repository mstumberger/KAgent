use kagent_core::permission::{Decision, Origin, PermissionEngine, PermissionMode};
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::tool::{list_text, TicketsCell, TicketsTool};
use super::*;

fn rig() -> (TicketsTool, Arc<Tickets>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let (tx, _rx) = broadcast::channel(16);
    let hub = Tickets::new(dir.path().join("tickets"), tx);
    let cell: TicketsCell = Default::default();
    let _ = cell.set(hub.clone());
    (TicketsTool::new(cell), hub, dir)
}

async fn run(t: &TicketsTool, input: Value) -> (bool, String) {
    let o = t.call(input, ToolContext::standalone(std::time::Duration::from_secs(5), 10_000)).await.unwrap();
    (o.is_error, o.content)
}

#[tokio::test]
async fn add_update_list_close_in_the_inbox() {
    let (t, hub, _d) = rig();
    let (err, text) = run(&t, json!({"action": "add", "title": "Renew the certificate", "priority": "high", "labels": ["ops"]})).await;
    assert!(!err);
    assert_eq!(text, "Added IN-1 \"Renew the certificate\" (todo, high) to Inbox.");
    assert_eq!(hub.get("IN-1").unwrap().history[0].by, By::Agent);
    let (_, text) = run(&t, json!({"action": "update", "key": "IN-1", "state": "in_progress"})).await;
    assert_eq!(text, "IN-1 updated: state: todo -> in_progress.");
    let (err, text) = run(&t, json!({"action": "update", "key": "IN-1", "state": "in_progress"})).await;
    assert!(err && text.starts_with("not done: nothing to change on IN-1. Next: pass a field to change."), "{text}");
    let (_, text) = run(&t, json!({"action": "list"})).await;
    assert_eq!(text, "1 ticket in Inbox:\nIN-1 [in_progress, high] Renew the certificate (ops)");
    let (_, text) = run(&t, json!({"action": "close", "key": "IN-1", "note": "done by hand"})).await;
    assert_eq!(text, "IN-1 is done. Note added.");
    let (err, text) = run(&t, json!({"action": "close", "key": "IN-1"})).await;
    assert!(err && text.contains("already done"), "{text}");
    let (_, text) = run(&t, json!({"action": "list", "state": "todo"})).await;
    assert_eq!(text, "No tickets in Inbox.");
}

#[tokio::test]
async fn refusals_say_what_to_do() {
    let (t, _hub, _d) = rig();
    let (_, text) = run(&t, json!({"action": "add"})).await;
    assert_eq!(text, "not done: add needs a title. Next: pass title.");
    let (_, text) = run(&t, json!({"action": "update", "key": "KA-99", "title": "x"})).await;
    assert_eq!(text, "not done: no ticket KA-99. Next: call tickets list to see the keys.");
    let (_, text) = run(&t, json!({"action": "update"})).await;
    assert!(text.contains("needs a key"), "{text}");
    let (_, text) = run(&t, json!({"action": "add", "title": "x".repeat(300)})).await;
    assert!(text.starts_with("not done: the ticket was not added: title: 1 to 200"), "{text}");
    let (_, text) = run(&t, json!({"action": "explode"})).await;
    assert!(text.contains("use add, update, list or close"), "{text}");
}

#[tokio::test]
async fn the_tool_cannot_reach_another_projects_ticket() {
    let (t, hub, d) = rig();
    let p = d.path().join("proj");
    std::fs::create_dir_all(&p).unwrap();
    hub.create(Create { project: Some(p.to_string_lossy().to_string()), title: "elsewhere".into(), ..Default::default() }, By::Owner).unwrap();
    // The session has no project (no agent is attached): the Inbox. The project's key is not found from here.
    let (_, text) = run(&t, json!({"action": "update", "key": "PR-1", "title": "x"})).await;
    assert!(text.contains("no ticket PR-1"), "{text}");
    let (_, all) = run(&t, json!({"action": "list", "all_projects": true})).await;
    assert!(all.starts_with("1 ticket in all projects:") && all.contains("elsewhere"), "{all}");
    let (_, here) = run(&t, json!({"action": "list"})).await;
    assert_eq!(here, "No tickets in Inbox.");
}

#[test]
fn list_is_read_only_and_the_rest_ask_and_always_from_voice() {
    let (t, _h, _d) = rig();
    let list = t.assess(&json!({"action": "list"}));
    assert!(list.read_only && !list.needs_approval && !list.always_ask);
    assert_eq!(kagent_core::tools::group_of("tickets"), "agent");
    for action in ["add", "update", "close"] {
        let a = t.assess(&json!({"action": action, "title": "t", "key": "KA-1"}));
        assert!(!a.read_only && a.needs_approval && a.always_ask, "{action}");
        for mode in [PermissionMode::Ask, PermissionMode::Trusted, PermissionMode::Full] {
            let engine = PermissionEngine::new(mode, Vec::new());
            assert!(matches!(engine.evaluate_from("s", "tickets", &a, Origin::Voice), Decision::Ask { .. }), "{action} from voice in {mode:?}");
        }
        let ask = PermissionEngine::new(PermissionMode::Ask, Vec::new());
        assert!(matches!(ask.evaluate("s", "tickets", &a), Decision::Ask { .. }), "{action} asks in chat");
    }
    let engine = PermissionEngine::new(PermissionMode::ReadOnly, Vec::new());
    assert!(matches!(engine.evaluate_from("s", "tickets", &list, Origin::Voice), Decision::Allow { .. }));
}

#[test]
fn a_long_list_is_cut_and_says_so() {
    let ticket = |i: usize| Ticket {
        key: format!("IN-{i}"),
        project: None,
        title: format!("t{i}"),
        description: String::new(),
        state: State::Todo,
        priority: Priority::None,
        labels: vec![],
        parent: None,
        links: vec![],
        assignee: None,
        done_when: vec![],
        notes: vec![],
        history: vec![],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let all: Vec<Ticket> = (1..=45).map(ticket).collect();
    let text = list_text(&all, "Inbox");
    assert_eq!(text.lines().count(), 1 + 30 + 1);
    assert!(text.ends_with("(15 more: narrow it with state, label or text)"));
}
