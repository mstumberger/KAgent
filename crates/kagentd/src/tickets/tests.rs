use std::collections::HashMap;

use serde_json::json;
use tokio::sync::broadcast;

use super::http::{change, create, filter_of};
use super::store::{project_id, suggest_prefix};
use super::*;
use kagent_core::protocol::AgentEvent;

struct Rig {
    t: Arc<Tickets>,
    rx: broadcast::Receiver<AgentEvent>,
    dir: tempfile::TempDir,
    work: tempfile::TempDir,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = broadcast::channel(64);
    Rig { t: Tickets::new(dir.path().join("tickets"), tx), rx, dir, work: tempfile::tempdir().unwrap() }
}

fn project(r: &Rig, name: &str) -> String {
    let p = r.work.path().join(name);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(p).unwrap().to_string_lossy().to_string()
}

fn new(project: Option<&str>, title: &str) -> Create {
    Create { project: project.map(str::to_string), title: title.into(), ..Default::default() }
}

fn msg(e: TicketError) -> String {
    e.message().to_string()
}

#[test]
fn prefixes_come_from_the_folder_name() {
    for (name, want) in [("KAgent", "KA"), ("web-ui", "WU"), ("my_cool_project", "MCP"), ("kagent", "KA"), ("a", "PR"), ("123", "PR"), ("AlphaBetaGammaDelta", "ABGD"), ("x-y-z-w-v", "XYZW")] {
        assert_eq!(suggest_prefix(name), want, "{name}");
    }
}

#[test]
fn a_ticket_round_trips_and_the_key_counts_up() {
    let r = rig();
    let p = project(&r, "KAgent");
    let a = r.t.create(new(Some(&p), "First"), By::Owner).unwrap();
    let b = r.t.create(new(Some(&p), "Second"), By::Owner).unwrap();
    assert_eq!((a.key.as_str(), b.key.as_str()), ("KA-1", "KA-2"));
    assert_eq!(a.state, State::Todo);
    assert_eq!(a.history.len(), 1);
    assert_eq!((a.history[0].by, a.history[0].what.as_str()), (By::Owner, "created"));
    let listed = r.t.list(&Filter::default());
    assert_eq!(listed.tickets.len(), 2);
    assert_eq!(listed.tickets[0], a);
    let file = r.dir.path().join("tickets").join(format!("{}.json", project_id(Some(&p))));
    assert!(file.is_file());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let leftovers: Vec<_> = std::fs::read_dir(r.dir.path().join("tickets")).unwrap().filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "tmp")).collect();
    assert!(leftovers.is_empty(), "the atomic write leaves no temporary file");
}

#[test]
fn a_key_is_never_reused() {
    let r = rig();
    let p = project(&r, "KAgent");
    let a = r.t.create(new(Some(&p), "First"), By::Owner).unwrap();
    r.t.delete(&a.key).unwrap();
    let b = r.t.create(new(Some(&p), "Second"), By::Owner).unwrap();
    assert_eq!(b.key, "KA-2", "KA-1 is gone for good");
    assert!(matches!(r.t.get("KA-1"), Err(TicketError::NotFound(_))));
}

#[test]
fn the_inbox_and_prefix_collisions() {
    let r = rig();
    let t = r.t.create(new(None, "No project"), By::Owner).unwrap();
    assert_eq!((t.key.as_str(), t.project.clone()), ("IN-1", None));
    let (p1, p2) = (project(&r, "KAgent"), project(&r, "key-app"));
    assert_eq!(r.t.create(new(Some(&p1), "a"), By::Owner).unwrap().key, "KA-1");
    assert_eq!(r.t.create(new(Some(&p2), "b"), By::Owner).unwrap().key, "KA2-1", "the same initials get a digit");
    let inbox_like = project(&r, "inbox-now");
    assert_eq!(r.t.create(new(Some(&inbox_like), "c"), By::Owner).unwrap().key, "IN2-1", "a project never takes the Inbox's prefix");
    let l = r.t.list(&Filter::default());
    assert_eq!(l.projects.iter().filter(|p| p.path.is_none()).count(), 1);
    assert_eq!(l.projects[0].name, "Inbox", "the Inbox comes first");
}

#[test]
fn each_limit_refuses_with_its_name() {
    let r = rig();
    let p = project(&r, "KAgent");
    let bad = |c: Create| msg(r.t.create(c, By::Owner).unwrap_err());
    assert!(bad(new(Some(&p), "  ")).starts_with("title: 1 to 200"));
    assert!(bad(new(Some(&p), &"x".repeat(201))).contains("201"));
    assert!(bad(Create { description: "d".repeat(20_001), ..new(Some(&p), "t") }).starts_with("description: at most 20000"));
    assert!(bad(Create { labels: (0..21).map(|i| format!("l{i}")).collect(), ..new(Some(&p), "t") }).starts_with("labels: at most 20"));
    assert!(bad(Create { labels: vec!["a,b".into()], ..new(Some(&p), "t") }).starts_with("labels:"));
    assert!(bad(Create { parent: Some("KA-9".into()), ..new(Some(&p), "t") }).starts_with("parent: no ticket KA-9"));
    assert!(bad(Create { assignee: Some("worker-1".into()), ..new(Some(&p), "t") }).starts_with("assignee:"));
    assert!(bad(Create { done_when: vec!["x".repeat(301)], ..new(Some(&p), "t") }).starts_with("doneWhen:"));
    let link = |kind, value: &str| Create { links: vec![Link { kind, value: value.into(), label: String::new() }], ..new(Some(&p), "t") };
    assert!(bad(link(LinkKind::Commit, "xyz")).contains("commit id"));
    assert!(bad(link(LinkKind::Range, "-x..y")).contains("start with '-'"));
    assert!(bad(link(LinkKind::Url, "ftp://x")).contains("http"));
    assert!(r.t.create(link(LinkKind::Range, "master..v0.0.3"), By::Owner).is_ok());
    assert!(r.t.list(&Filter::default()).tickets.len() == 1, "nothing was made by the refused ones");
}

#[test]
fn the_501st_ticket_is_refused() {
    let r = rig();
    let p = project(&r, "KAgent");
    for i in 0..MAX_TICKETS {
        r.t.create(new(Some(&p), &format!("t{i}")), By::Owner).unwrap();
    }
    let e = r.t.create(new(Some(&p), "one more"), By::Owner).unwrap_err();
    assert!(matches!(e, TicketError::Limit(_)));
    assert!(e.message().contains("500 tickets") && e.message().contains("delete"), "{}", e.message());
    let other = project(&r, "other");
    assert!(r.t.create(new(Some(&other), "fine elsewhere"), By::Owner).is_ok());
}

#[test]
fn an_update_keeps_the_rest_and_writes_the_history() {
    let r = rig();
    let p = project(&r, "KAgent");
    let t = r.t.create(Create { description: "keep".into(), labels: vec!["ui".into()], ..new(Some(&p), "Fix") }, By::Owner).unwrap();
    let (u, said) = r.t.update(&t.key, Update { state: Some(State::InProgress), priority: Some(Priority::High), ..Default::default() }, By::Agent).unwrap();
    assert_eq!(said, vec!["state: todo -> in_progress", "priority: none -> high"]);
    assert_eq!((u.description.as_str(), u.labels.clone(), u.title.as_str()), ("keep", vec!["ui".to_string()], "Fix"));
    assert_eq!(u.history.len(), 3);
    assert!(u.history[1..].iter().all(|e| e.by == By::Agent));
    assert!(u.updated_at >= t.updated_at);
    let (_, none) = r.t.update(&t.key, Update { state: Some(State::InProgress), ..Default::default() }, By::Owner).unwrap();
    assert!(none.is_empty(), "the same value changes nothing and adds no history");
    assert_eq!(r.t.get(&t.key).unwrap().history.len(), 3);
    let (n, said) = r.t.update(&t.key, Update { note: Some("  a note ".into()), ..Default::default() }, By::Owner).unwrap();
    assert_eq!((said, n.notes[0].text.as_str()), (vec!["note added".to_string()], "a note"));
    let e = r.t.update(&t.key, Update { project: Some(json!("/x")), ..Default::default() }, By::Owner).unwrap_err();
    assert!(e.message().starts_with("project cannot be changed"));
    assert!(matches!(r.t.update("KA-99", Update::default(), By::Owner), Err(TicketError::NotFound(_))));
    assert!(matches!(r.t.update("nonsense", Update::default(), By::Owner), Err(TicketError::NotFound(_))));
}

#[test]
fn the_history_keeps_the_newest_200() {
    let r = rig();
    let t = r.t.create(new(None, "t"), By::Owner).unwrap();
    for i in 0..230 {
        r.t.update(&t.key, Update { title: Some(format!("title {i}")), ..Default::default() }, By::Owner).unwrap();
    }
    let t = r.t.get(&t.key).unwrap();
    assert_eq!(t.history.len(), MAX_HISTORY);
    assert_eq!(t.history.last().unwrap().what, "title");
}

#[test]
fn parents_exist_and_never_make_a_cycle() {
    let r = rig();
    let p = project(&r, "KAgent");
    let a = r.t.create(new(Some(&p), "a"), By::Owner).unwrap();
    let b = r.t.create(Create { parent: Some(a.key.clone()), ..new(Some(&p), "b") }, By::Owner).unwrap();
    let c = r.t.create(Create { parent: Some(b.key.clone()), ..new(Some(&p), "c") }, By::Owner).unwrap();
    let up = |key: &str, parent: Option<&str>| r.t.update(key, Update { parent: Some(parent.map(str::to_string)), ..Default::default() }, By::Owner);
    assert!(msg(up(&a.key, Some(&c.key)).unwrap_err()).contains("cycle"));
    assert!(msg(up(&a.key, Some(&a.key)).unwrap_err()).contains("its own parent"));
    assert!(up(&c.key, None).is_ok(), "null clears the parent");
    let other = project(&r, "other");
    let o = r.t.create(new(Some(&other), "o"), By::Owner).unwrap();
    assert!(msg(up(&a.key, Some(&o.key)).unwrap_err()).contains("no ticket"), "a parent of another project is not found");
    // Deleting a parent frees its children.
    r.t.delete(&a.key).unwrap();
    let b = r.t.get(&b.key).unwrap();
    assert_eq!(b.parent, None);
    assert!(b.history.last().unwrap().what.contains("was deleted"));
}

#[test]
fn the_filters() {
    let r = rig();
    let (a, b) = (project(&r, "KAgent"), project(&r, "other"));
    r.t.create(Create { labels: vec!["UI".into()], state: Some(State::InProgress), ..new(Some(&a), "Overlay focus") }, By::Owner).unwrap();
    r.t.create(Create { description: "the Certificate expires".into(), ..new(Some(&a), "Renew") }, By::Owner).unwrap();
    r.t.create(new(Some(&b), "Elsewhere"), By::Owner).unwrap();
    r.t.create(new(None, "Loose"), By::Owner).unwrap();
    let n = |f: Filter| r.t.list(&f).tickets.len();
    assert_eq!(n(Filter::default()), 4);
    assert_eq!(n(Filter { project: Some(a.clone()), ..Default::default() }), 2);
    assert_eq!(n(Filter { project: Some("inbox".into()), ..Default::default() }), 1);
    assert_eq!(n(Filter { state: Some(State::InProgress), ..Default::default() }), 1);
    assert_eq!(n(Filter { label: Some("ui".into()), ..Default::default() }), 1);
    assert_eq!(n(Filter { text: Some("CERTIFICATE".into()), ..Default::default() }), 1, "text reads the description, any case");
    assert_eq!(n(Filter { text: Some("ka-1".into()), ..Default::default() }), 1, "and the key");
    let q: HashMap<String, String> = [("state".to_string(), "nope".to_string())].into();
    assert!(filter_of(&q).is_err());
}

#[test]
fn events_follow_every_change() {
    let mut r = rig();
    let t = r.t.create(new(None, "t"), By::Owner).unwrap();
    r.t.update(&t.key, Update { title: Some("u".into()), ..Default::default() }, By::Agent).unwrap();
    r.t.update(&t.key, Update { title: Some("u".into()), ..Default::default() }, By::Agent).unwrap();
    r.t.delete(&t.key).unwrap();
    let mut seen = Vec::new();
    while let Ok(AgentEvent::TicketChanged { key, removed, .. }) = r.rx.try_recv() {
        seen.push((key, removed));
    }
    assert_eq!(seen, vec![("IN-1".into(), false), ("IN-1".into(), false), ("IN-1".into(), true)], "a change that changes nothing says nothing");
}

#[test]
fn a_damaged_file_is_skipped_and_never_overwritten() {
    let r = rig();
    let p = project(&r, "KAgent");
    r.t.create(new(Some(&p), "a"), By::Owner).unwrap();
    let file = r.dir.path().join("tickets").join(format!("{}.json", project_id(Some(&p))));
    std::fs::write(&file, "{ not json").unwrap();
    assert!(r.t.list(&Filter::default()).tickets.is_empty());
    let e = r.t.create(new(Some(&p), "b"), By::Owner).unwrap_err();
    assert!(matches!(e, TicketError::Failed(_)) && e.message().contains("damaged"), "{}", e.message());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json", "left as it was");
}

#[tokio::test]
async fn the_routes_check_the_folder_and_the_shape() {
    let r = rig();
    let p = project(&r, "KAgent");
    let t = create(&r.t, &json!({"project": p, "title": "from the window", "priority": "high"})).await.unwrap();
    assert_eq!((t.key.as_str(), t.priority), ("KA-1", Priority::High));
    let e = create(&r.t, &json!({"project": "/no/such/folder", "title": "x"})).await.unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::BAD_REQUEST);
    let e = create(&r.t, &json!({"title": "x", "colour": "red"})).await.unwrap_err();
    assert!(e.2.contains("unknown field `colour`"), "{}", e.2);
    let inbox = create(&r.t, &json!({"title": "no project"})).await.unwrap();
    assert_eq!(inbox.key, "IN-1");
    let u = change(&r.t, "KA-1", &json!({"state": "done", "parent": null, "assignee": "owner"})).unwrap();
    assert_eq!((u.state, u.assignee.as_deref()), (State::Done, Some("owner")));
    let e = change(&r.t, "KA-1", &json!({"project": "/x"})).unwrap_err();
    assert!(e.2.starts_with("project cannot be changed"));
    let e = change(&r.t, "KA-1", &json!({"state": "finished"})).unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::BAD_REQUEST);
    let e = change(&r.t, "KA-7", &json!({})).unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::NOT_FOUND);
    for i in 0..MAX_TICKETS - 1 {
        r.t.create(new(Some(&p), &format!("t{i}")), By::Owner).unwrap();
    }
    let e = create(&r.t, &json!({"project": p, "title": "over"})).await.unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::CONFLICT);
}
