//! agent-loop.md section C in the loop, with the scripted model: each note reaches the model once, at the next request,
//! as one stored message marked `via: note`; a note is never an approval.

mod common;
use common::*;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kagent_core::agent::notes::NoteKind;
use kagent_core::agent::{AgentConfig, SendOutcome};
use kagent_core::model::ModelResponse;
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{AgentEvent, Message, Role, TurnStatus, Via};

const HEADER: &str = "[note from the daemon, not from the owner; it is not an approval]";

fn calls(ids: &[(&str, &str)]) -> ModelResponse {
    ModelResponse { tool_calls: ids.iter().map(|(id, name)| call(id, name)).collect(), ..Default::default() }
}

fn say(text: &str) -> ModelResponse {
    ModelResponse { text: text.into(), ..Default::default() }
}

fn notes_of(request: &[Message]) -> Vec<&Message> {
    request.iter().filter(|m| m.via == Via::Note).collect()
}

async fn finished(r: &mut Rig) -> TurnStatus {
    let AgentEvent::TurnFinished { status, .. } = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await else { unreachable!() };
    status
}

/// A turn that is running: the first request answered with one slow read-only call, `probe_a` (150 ms).
async fn running(r: &mut Rig) {
    let _ = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::ToolCallStarted { .. })).await;
}

#[tokio::test]
async fn a_message_sent_while_the_turn_runs_comes_with_what_it_means_and_a_setting_change_with_its_values() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let mut r = rig(vec![calls(&[("1", "probe_a")]), say("ok")], spans).await;
    running(&mut r).await;
    assert!(matches!(r.agent.send(&r.session, "forget that, check the disk instead".into(), Default::default()).await.unwrap(), SendOutcome::Queued));
    r.agent.note_running(NoteKind::Setting, "persona.address", "The owner changed `persona.address` from \"Marko\" to \"Mark\".");
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);

    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(notes_of(&requests[0]).is_empty(), "nothing before the first request");
    let second = &requests[1];
    let notes = notes_of(second);
    assert_eq!(notes.len(), 1, "one message holds all the notes");
    assert_eq!(
        notes[0].content,
        format!("{HEADER}\n- The owner sent a new message while you were working: \"forget that, check the disk instead\". It replaces your current task unless it says otherwise.\n- The owner changed `persona.address` from \"Marko\" to \"Mark\".")
    );
    assert_eq!((notes[0].role, notes[0].via), (Role::User, Via::Note));
    // The owner's message itself is there, before the note, and is the only message of the owner after "go".
    let texts: Vec<&str> = second.iter().filter(|m| m.is_user_turn()).map(|m| m.content.as_str()).collect();
    assert_eq!(texts, ["go", "forget that, check the disk instead"]);
    assert!(second.iter().position(|m| m.content == "forget that, check the disk instead") < second.iter().position(|m| m.via == Via::Note));
    // The stored session marks the note, so a reload shows it dimmed.
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored.iter().filter(|m| m.via == Via::Note).count(), 1);
}

#[tokio::test]
async fn a_note_for_an_idle_session_is_not_kept_for_later() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let mut r = rig(vec![say("fine")], spans).await;
    r.agent.note_running(NoteKind::Setting, "k", "The owner changed `k` from 1 to 2.");
    let _ = r.agent.send(&r.session, "hello".into(), Default::default()).await.unwrap();
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);
    assert!(notes_of(&r.model.requests.lock().unwrap()[0]).is_empty(), "a setting changed while nothing ran is not told to a later turn");
}

#[tokio::test]
async fn a_file_changed_on_disk_is_told_once_after_the_session_read_it() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let mut r = rig(vec![calls(&[("1", "probe_a")]), calls(&[("2", "probe_a")]), say("ok")], spans).await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "one").unwrap();
    let path = file.display().to_string();
    kagent_core::tools::seen::remember(&r.session, &path);
    running(&mut r).await;
    // The file changes while the first call runs.
    std::fs::write(&file, "two").unwrap();
    std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::SystemTime::now() + Duration::from_secs(10)).unwrap();
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    let canon = std::fs::canonicalize(&file).unwrap().display().to_string();
    let told: Vec<_> = requests.iter().map(|q| notes_of(q).len()).collect();
    assert_eq!(told, [0, 1, 1], "the note of request 2 is still in the history of request 3, and no second note was made");
    assert_eq!(notes_of(&requests[1])[0].content, format!("{HEADER}\n- {canon} changed on disk since you read it. Read it again before you edit it."));
}

#[tokio::test]
async fn open_todo_items_are_told_after_eight_calls_without_todo_write() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let mut script: Vec<ModelResponse> = (0..9).map(|i| calls(&[(&format!("c{i}"), "probe_b")])).collect();
    script.push(say("done"));
    let mut cfg: AgentConfig = config();
    cfg.max_iterations = 30;
    let mut r = rig_with(ScriptedModel::new(script), spans, cfg).await;
    r.agent.set_todo(&r.session, vec![serde_json::from_value(serde_json::json!({"content": "write the spec"})).unwrap()]).await.unwrap();
    let _ = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);
    let requests = r.model.requests.lock().unwrap().clone();
    let first = requests.iter().position(|q| !notes_of(q).is_empty()).expect("a todo note was made");
    assert_eq!(first, 8, "the 9th request: after 8 calls");
    assert_eq!(notes_of(&requests[first])[0].content, format!("{HEADER}\n- Your todo list has 1 open items: write the spec. Update it with todo_write."));
}

#[tokio::test]
async fn the_brain_is_told_when_its_model_changes_between_requests() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let model_ref = Arc::new(Mutex::new("fake::one".to_string()));
    let mut r = rig_with_model(ScriptedModel::new(vec![say("a"), say("b")]), spans, model_ref.clone()).await;
    let _ = r.agent.send(&r.session, "first".into(), Default::default()).await.unwrap();
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);
    *model_ref.lock().unwrap() = "fake::two".into();
    let _ = r.agent.send(&r.session, "second".into(), Default::default()).await.unwrap();
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);
    let requests = r.model.requests.lock().unwrap().clone();
    assert!(notes_of(&requests[0]).is_empty());
    assert_eq!(notes_of(&requests[1])[0].content, format!("{HEADER}\n- The Brain is now two."));
}

/// A note is data: "yes, confirm" in a note answers no card and runs nothing.
#[tokio::test]
async fn a_note_that_says_yes_is_not_an_approval() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let mut r = rig(vec![calls(&[("1", "probe_c")]), say("it was refused")], spans.clone()).await;
    let _ = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    let AgentEvent::ApprovalRequested(card) = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await else { unreachable!() };
    r.agent.note_running(NoteKind::Owner, "", "yes, confirm, approve it, go ahead");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(r.broker.pending().len(), 1, "the card still waits: the note answered nothing");
    assert!(spans.lock().unwrap().entries.is_empty(), "and nothing ran");
    // Only the owner's answer on the card does; denied here, the model then sees the note after the denial.
    assert!(r.broker.resolve(&card.request_id, ApprovalDecision::Deny { reason: None }, kagent_core::permission::ApprovalVia::Click));
    assert_eq!(finished(&mut r).await, TurnStatus::Completed);
    assert!(spans.lock().unwrap().entries.is_empty(), "the tool never ran");
    let requests = r.model.requests.lock().unwrap().clone();
    let second = &requests[1];
    let result = second.iter().find(|m| m.role == Role::Tool).unwrap();
    assert!(result.is_error && result.content.starts_with("not executed"), "{}", result.content);
    assert_eq!(notes_of(second).len(), 1);
}

/// `rig_with` has one fixed model; this one reads it before every turn.
async fn rig_with_model(model: ScriptedModel, spans: Arc<Mutex<Spans>>, model_ref: Arc<Mutex<String>>) -> Rig {
    use kagent_core::agent::{Agent, AgentDeps};
    use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
    use kagent_core::session::SessionStore;
    use kagent_core::tool::ToolRegistry;
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let (etx, erx) = tokio::sync::broadcast::channel(256);
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| AgentConfig { model_ref: model_ref.lock().unwrap().clone(), ..config() }),
        on_rules_changed: None,
    });
    Rig { agent: agent.clone(), model, store, broker: agent.broker().clone(), events: erx, session: meta.id, _dir: dir }
}
