//! Steering (steering.md): what a running worker is doing, and (further down) correcting and interrupting it. On the rig
//! of this folder, with the file tools as probes.

use std::sync::Arc;

use kagent_core::agent::children::live::ActionStatus;
use kagent_core::agent::children::ChildStatus;
use kagent_core::permission::PermissionMode;
use kagent_core::protocol::{ToolCallStatus, TurnStatus};
use serde_json::json;
use tokio::sync::Notify;

use super::common::ProbeTool;
use super::rig::*;

/// Probes named as the file tools, so a worker's calls name files.
pub fn file_probes(r: &Arc<std::sync::Mutex<super::common::Spans>>) -> Vec<Arc<dyn kagent_core::tool::Tool>> {
    vec![Arc::new(ProbeTool::ro("read_file", r)), Arc::new(ProbeTool::ro("edit_file", r))]
}

#[tokio::test]
async fn a_running_worker_tells_its_last_action_and_the_files_it_touched() {
    let spans = Arc::new(std::sync::Mutex::new(Default::default()));
    let hold_third = Arc::new(Notify::new());
    let r = rig_with(
        &[
            ("parent", vec![run("job-live"), say("started")]),
            ("job-live", vec![call("read_file", json!({"path": "/work/a.rs"})), call("edit_file", json!({"path": "/work/a.rs"})), gated(&hold_third, "all done")]),
        ],
        PermissionMode::Full,
        file_probes(&spans),
    )
    .await;
    let result = turn(&r, "go").await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let child = kids(&r, 1).await.remove(0);
    until(10, "the worker's third request", || r.model.of("job-live").len() == 3).await;
    until(10, "the edit to be recorded", || children(&r)[0].last_action.as_ref().is_some_and(|a| a.tool == "edit_file" && a.status == ActionStatus::Done)).await;
    let view = children(&r).remove(0);
    assert_eq!(view.status, ChildStatus::Running);
    assert_eq!(view.files.iter().map(|f| (f.path.as_str(), f.changed)).collect::<Vec<_>>(), [("/work/a.rs", true)], "read, then changed: one file, changed");
    let last = view.last_action.unwrap();
    assert_eq!(last.summary, "edit_file {\"path\":\"/work/a.rs\"}", "the summary the card would show, without the name of the job");

    // `subagentUpdated` carried it: a window that only listens to events knows too.
    assert!(r.events_of_child(&child.id).iter().any(|c| c.last_action.as_ref().is_some_and(|a| a.tool == "read_file" && a.status == ActionStatus::Running)
        || c.last_action.as_ref().is_some_and(|a| a.tool == "read_file" && a.status == ActionStatus::Done)), "the events carry the tool the worker is in");
    assert!(r.events_of_child(&child.id).iter().any(|c| c.files.iter().any(|f| f.path == "/work/a.rs" && f.changed)), "and the files");
    hold_third.notify_one();
    wait_status(&r, &child.id, ChildStatus::Done).await;
}

// ---------------------------------------------------------------- correcting and interrupting (steering.md sections 3 and 4)

use kagent_core::agent::children::live::SteerState;
use kagent_core::agent::children::{TellError, STEER_RULE};
use kagent_core::model::ModelResponse;
use kagent_core::protocol::{Role, ToolCall, Via};

/// A held request that answers with one tool call when released.
fn gated_call(release: &Arc<Notify>, name: &str, input: serde_json::Value) -> Step {
    Step::Gate { release: release.clone(), response: ModelResponse { tool_calls: vec![ToolCall { id: format!("call_{name}"), name: name.into(), input }], ..Default::default() } }
}

fn states(r: &Rig, id: &str) -> Vec<(SteerState, bool)> {
    children(r).iter().find(|c| c.id == id).map(|c| c.steering.iter().map(|s| (s.state, s.delivered_at.is_some())).collect()).unwrap_or_default()
}

#[tokio::test]
async fn a_message_is_read_at_the_next_request_once_in_order_with_receipts() {
    let (first, last) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let r = rig(&[("parent", vec![run("job-t"), say("started")]), ("job-t", vec![gated_call(&first, "probe_a", json!({})), gated(&last, "final answer")])], PermissionMode::Ask).await;
    assert_eq!(turn(&r, "go").await.status, TurnStatus::Completed);
    let child = kids(&r, 1).await.remove(0);
    until(10, "the first request, held", || r.model.of("job-t").len() == 1).await;
    assert!(r.model.of("job-t")[0].system.contains(STEER_RULE), "every subagent's prompt tells it to acknowledge the owner's corrections");

    // Two messages while the request is held: queued, nothing read yet.
    r.agent.tell_child(None, &child.id, "leave the tests alone", false).await.unwrap();
    r.agent.tell_child(None, &child.id, "and keep the diff small", false).await.unwrap();
    assert_eq!(states(&r, &child.id), [(SteerState::Queued, false), (SteerState::Queued, false)]);
    assert_eq!(r.model.of("job-t").len(), 1, "no request was made for them");
    first.notify_one();
    until(10, "the second request, held", || r.model.of("job-t").len() == 2).await;
    // Delivered at the next request, as ONE message, in order, not a turn.
    assert_eq!(r.model.of("job-t")[1].last_user, "[from the owner] leave the tests alone\n[from the owner] and keep the diff small");
    assert_eq!(states(&r, &child.id), [(SteerState::Delivered, true), (SteerState::Delivered, true)]);
    let told: Vec<Vec<SteerState>> = r.events_of_child(&child.id).iter().map(|c| c.steering.iter().map(|s| s.state).collect()).filter(|v: &Vec<SteerState>| !v.is_empty()).collect();
    assert!(told.contains(&vec![SteerState::Queued, SteerState::Queued]) && told.last() == Some(&vec![SteerState::Delivered, SteerState::Delivered]), "the receipts were announced: {told:?}");
    let stored = messages(&r, &child.id).await;
    let note = stored.iter().find(|m| m.content.starts_with("[from the owner]")).unwrap();
    assert_eq!((note.role, note.via, note.is_user_turn()), (Role::User, Via::Note, false));
    assert_eq!(stored.iter().filter(|m| m.content.starts_with("[from the owner]")).count(), 1, "once");
    last.notify_one();
    wait_status(&r, &child.id, ChildStatus::Done).await;
    assert_eq!(r.model.of("job-t").len(), 2, "no third request for a message that was read");
}

#[tokio::test]
async fn an_owner_message_answers_no_approval_card_of_the_worker() {
    let release = Arc::new(Notify::new());
    let r = rig(&[("parent", vec![run("job-ask"), say("started")]), ("job-ask", vec![call("probe_c", json!({})), gated(&release, "ended without it")])], PermissionMode::Ask).await;
    assert_eq!(turn(&r, "go").await.status, TurnStatus::Completed);
    let child = kids(&r, 1).await.remove(0);
    until(10, "the worker's card", || r.agent.broker().pending().len() == 1).await;
    let card = r.agent.broker().pending().remove(0);
    for text in ["yes", "approve it", "confirm"] {
        r.agent.tell_child(None, &child.id, text, false).await.unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let still = r.agent.broker().pending();
    assert_eq!(still.len(), 1, "the card of the worker is still waiting: {still:?}");
    assert_eq!(still[0].request_id, card.request_id);
    assert!(r.spans.lock().unwrap().entries.iter().all(|(name, _, _)| *name != "probe_c"), "and the call did not run");
    // Only the owner's answer to the card (the approvals route) resolves it.
    assert!(r.agent.broker().resolve(&card.request_id, kagent_core::permission::ApprovalDecision::Deny { reason: None }, kagent_core::permission::ApprovalVia::Click));
    until(10, "the second request", || r.model.of("job-ask").len() == 2).await;
    let result = results(&r, &child.id, "probe_c").await.remove(0);
    assert!(result.content.starts_with("not executed:"), "{}", result.content);
    assert!(r.model.of("job-ask")[1].last_user.starts_with("[from the owner] yes\n"), "the words were delivered as words: {}", r.model.of("job-ask")[1].last_user);
    release.notify_one();
    wait_status(&r, &child.id, ChildStatus::Done).await;
}

#[tokio::test]
async fn a_worker_cannot_message_another_worker_or_the_owners_conversation() {
    let release = Arc::new(Notify::new());
    let r = rig_with(&[("parent", vec![run("job-a"), say("started")]), ("job-a", vec![gated(&release, "done")])], PermissionMode::Full, Vec::new()).await;
    assert_eq!(turn(&r, "go").await.status, TurnStatus::Completed);
    let child = kids(&r, 1).await.remove(0);
    until(10, "the worker's request", || r.model.of("job-a").len() == 1).await;
    assert!(!r.model.of("job-a")[0].tools.iter().any(|t| t == "agent_tell"), "a worker is not offered the tool");
    assert!(r.model.parent_tools().iter().any(|t| t == "agent_tell"), "the main agent is");
    let (text, status) = run_tool(&r, &child.id, "agent_tell", json!({"id": child.id, "text": "hello myself"})).await;
    assert_eq!(status, ToolCallStatus::Denied, "{text}");
    assert!(text.contains("a subagent cannot start or manage subagents"), "{text}");
    // Another session's tool finds no such child.
    let other = r.store.create("another conversation").await.unwrap().id;
    let (text, _) = run_tool(&r, &other, "agent_tell", json!({"id": child.id, "text": "not yours"})).await;
    assert!(text.contains("no subagent") && text.contains("belongs to this session"), "{text}");
    assert_eq!(r.agent.tell_child(Some(&other), &child.id, "x", false).await.unwrap_err(), TellError::Unknown(child.id.clone()));
    assert!(states(&r, &child.id).is_empty(), "nothing was delivered by any of them");
    // The owner's conversation is not a worker: the route's function finds no child there.
    assert_eq!(r.agent.tell_child(None, &r.session, "hello", false).await.unwrap_err(), TellError::Unknown(r.session.clone()));
    release.notify_one();
    wait_status(&r, &child.id, ChildStatus::Done).await;
}

#[tokio::test]
async fn an_interrupt_during_a_tool_call_leaves_the_session_consistent_and_the_worker_running() {
    let r = rig(&[("parent", vec![run("job-i"), say("started"), say("the parent heard it end")]), ("job-i", vec![call("shell", json!({"command": "sleep 30"})), say("acknowledged: dropping it")])], PermissionMode::Full).await;
    assert_eq!(turn(&r, "go").await.status, TurnStatus::Completed);
    let child = kids(&r, 1).await.remove(0);
    until(10, "the shell call to run", || children(&r)[0].last_action.as_ref().is_some_and(|a| a.tool == "shell" && a.status == ActionStatus::Running)).await;
    let view = r.agent.tell_child(None, &child.id, "stop that and do X", true).await.unwrap();
    assert!(view.steering[0].interrupt);
    wait_status(&r, &child.id, ChildStatus::Done).await;
    let ever: Vec<ChildStatus> = statuses(&r, &child.id);
    assert_eq!(ever, [ChildStatus::Running, ChildStatus::Done], "the worker was never cancelled or ended: it went on");
    assert_eq!(states(&r, &child.id), [(SteerState::Delivered, true)]);
    // The session: every call has its result, then the message first, then the answer.
    let stored = messages(&r, &child.id).await;
    let shape: Vec<String> = stored.iter().map(|m| format!("{:?}:{:?}", m.role, m.via)).collect();
    assert_eq!(shape, ["User:Agent", "Assistant:Agent", "Tool:Text", "User:Agent", "Assistant:Agent"], "{stored:?}");
    for m in stored.iter().filter(|m| m.role == Role::Assistant) {
        for c in &m.tool_calls {
            assert!(stored.iter().any(|t| t.role == Role::Tool && t.tool_call_id.as_deref() == Some(c.id.as_str())), "the call {} has a result", c.id);
        }
    }
    assert!(stored[2].content.contains("cancelled"), "{}", stored[2].content);
    assert_eq!(stored[3].content, "[from the owner] stop that and do X");
    assert_eq!(r.model.of("job-i")[1].last_user, "[from the owner] stop that and do X", "the resumed request holds it first");
    // The parent is told once, when the worker ends.
    wait_turns(&r, &r.session, 2).await;
    let told = notes(&r, &r.session).await;
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("done in") && told[0].contains("acknowledged: dropping it"), "{}", told[0]);
}

#[tokio::test]
async fn stop_still_ends_a_worker_and_a_message_to_an_ended_or_waiting_one_is_refused() {
    let release = Arc::new(Notify::new());
    let r = rig(&[("parent", vec![run("job-1"), say("a"), run("job-2"), say("b")]), ("job-1", vec![gated(&release, "one done")]), ("job-2", vec![say("two")])], PermissionMode::Ask).await;
    r.agent.set_max_concurrent_children(1).await;
    assert_eq!(turn(&r, "go").await.status, TurnStatus::Completed);
    let first = kids(&r, 1).await.remove(0);
    assert_eq!(turn(&r, "another").await.status, TurnStatus::Completed);
    let both = kids(&r, 2).await;
    assert_eq!(both[1].status, ChildStatus::Queued);
    assert_eq!(r.agent.tell_child(None, &both[1].id, "hi", false).await.unwrap_err(), TellError::NotStarted);
    assert!(TellError::NotStarted.to_string().contains("still queued"));
    for bad in ["", "   ", &"x".repeat(2001)] {
        assert!(matches!(r.agent.tell_child(None, &first.id, bad, false).await, Err(TellError::Invalid(_))), "{} chars", bad.len());
    }
    assert!(matches!(r.agent.tell_child(None, "nope", "hi", false).await, Err(TellError::Unknown(_))));
    // Stop ends it: not an interrupt.
    r.agent.cancel_child(&r.session, &first.id).await.unwrap();
    wait_status(&r, &first.id, ChildStatus::Cancelled).await;
    assert_eq!(r.agent.tell_child(None, &first.id, "too late", false).await.unwrap_err(), TellError::Ended(ChildStatus::Cancelled));
    assert_eq!(r.agent.tell_child(None, &first.id, "too late", true).await.unwrap_err(), TellError::Ended(ChildStatus::Cancelled));
    assert_eq!(TellError::Ended(ChildStatus::Done).to_string(), "the worker already ended (done)");
}

#[tokio::test]
async fn the_tool_tells_in_the_owners_words_and_names_what_it_did() {
    let release = Arc::new(Notify::new());
    let r = rig(&[("parent", vec![run("Implementer for KA-12"), say("started")]), ("Implementer for KA-12", vec![gated(&release, "ok")])], PermissionMode::Full).await;
    assert_eq!(turn(&r, "go").await.status, TurnStatus::Completed);
    let child = kids(&r, 1).await.remove(0);
    until(10, "the worker's request", || r.model.of("Implementer for KA-12").len() == 1).await;
    let (text, status) = run_tool(&r, &r.session, "agent_tell", json!({"name": "implementer", "text": "leave the tests alone"})).await;
    assert_eq!(status, ToolCallStatus::Ok, "{text}");
    assert_eq!(text, "Told \"Implementer for KA-12\": it will see it at its next step.");
    let (text, _) = run_tool(&r, &r.session, "agent_tell", json!({"id": child.id, "text": "do X now", "interrupt": true})).await;
    assert!(text.starts_with("Interrupted \"Implementer for KA-12\" and told it: do X now."), "{text}");
    let (text, status) = run_tool(&r, &r.session, "agent_tell", json!({"name": "nobody", "text": "hi"})).await;
    assert_eq!(status, ToolCallStatus::NotDone);
    assert!(text.starts_with("not done: no subagent of this session has 'nobody' in its name. Next: call agent_list"), "{text}");
    // It asks as a mutating tool does, and always out loud from voice.
    let a = r.agent.tools().get("agent_tell").unwrap().assess(&json!({"id": "x", "text": "hi"}));
    assert!(!a.read_only && a.needs_approval && a.always_ask, "{a:?}");
    release.notify_one();
    wait_status(&r, &child.id, ChildStatus::Done).await;
}
