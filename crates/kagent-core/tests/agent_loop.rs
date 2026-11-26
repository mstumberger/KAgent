//! Scenarios S0-S1 items 5-7: the loop's parallel scheduling, denial handling,
//! cancellation and history repair, driven by a scripted fake model (the rig is in
//! `common/`).

mod common;
use common::*;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kagent_core::agent::{SendOutcome, TurnOpts};
use kagent_core::model::ModelResponse;
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{AgentEvent, Message, Role, ToolCallStatus};

// ------------------------------------------------------------------ scenarios

#[tokio::test]
async fn parallel_reads_then_denied_mutation() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let second = ModelResponse { text: "all done".into(), ..Default::default() };
    let first = ModelResponse {
        tool_calls: vec![call("1", "probe_a"), call("2", "probe_b"), call("3", "probe_c")],
        ..Default::default()
    };
    let mut r = rig(vec![first, second], spans.clone()).await;

    // Deny the approval as soon as it is asked.
    let broker = r.broker.clone();
    let mut erx = r.events.resubscribe();
    tokio::spawn(async move {
        let ev = wait_event(&mut erx, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await;
        if let AgentEvent::ApprovalRequested(req) = ev {
            broker.resolve(&req.request_id, ApprovalDecision::Deny { reason: None });
        }
    });

    let outcome = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    assert!(matches!(outcome, SendOutcome::Started { .. }));
    let fin = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    let AgentEvent::TurnFinished { status, .. } = fin else { panic!() };
    assert_eq!(status, kagent_core::protocol::TurnStatus::Completed);

    // Two read-only probes overlapped (ran in parallel).
    let s = spans.lock().unwrap();
    let a = s.entries.iter().find(|e| e.0 == "probe_a").expect("probe_a ran");
    let b = s.entries.iter().find(|e| e.0 == "probe_b").expect("probe_b ran");
    assert!(b.1 < a.2 && a.1 < b.2, "read-only calls must overlap: a {:?}-{:?} b {:?}-{:?}", a.1, a.2, b.1, b.2);
    drop(s);

    // Second model request: assistant message followed by 3 tool results in call order.
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let tail = &requests[1][requests[1].len() - 4..];
    assert_eq!(tail[0].role, Role::Assistant);
    assert_eq!(tail[0].tool_calls.len(), 3);
    for (i, m) in tail[1..].iter().enumerate() {
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some(["1", "2", "3"][i]));
    }
    assert!(tail[3].is_error);
    assert_eq!(tail[3].content, "not executed: the user denied this action");
}

#[tokio::test]
async fn cancel_during_approval_leaves_no_dangling_call() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let second = ModelResponse { text: "unreached".into(), ..Default::default() };
    let first = ModelResponse { tool_calls: vec![call("1", "probe_c")], ..Default::default() };
    let mut r = rig(vec![first, second], spans).await;

    let _ = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    let ev = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await;
    let AgentEvent::ApprovalRequested(_req) = ev else { panic!() };
    // Cancel while the approval is pending; never resolve it.
    assert!(r.agent.cancel(&r.session).await);
    let fin = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    let AgentEvent::TurnFinished { status, .. } = fin else { panic!() };
    assert_eq!(status, kagent_core::protocol::TurnStatus::Cancelled);

    let messages = r.store.load_messages(&r.session).await.unwrap();
    let assistant = messages.iter().find(|m| m.role == Role::Assistant && !m.tool_calls.is_empty()).expect("assistant with calls");
    for c in &assistant.tool_calls {
        let has_result = messages.iter().any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(&c.id));
        assert!(has_result, "call {} must have a result", c.id);
    }
    assert_eq!(r.model.requests.lock().unwrap().len(), 1, "no second model request after cancel");
}

#[tokio::test]
async fn sanitize_history_repairs_dangling_call() {
    let mut messages = vec![Message::user("hi")];
    let mut assistant = Message::assistant("working", vec![call("1", "probe_a")]);
    assistant.provider_state = None;
    messages.push(assistant);
    messages.push(Message::user("hello again"));

    let fixed = kagent_core::agent::sanitize_history(&messages);
    // user, assistant, synthetic tool result, user
    assert_eq!(fixed.len(), 4);
    assert_eq!(fixed[2].role, Role::Tool);
    assert_eq!(fixed[2].tool_call_id.as_deref(), Some("1"));
    assert!(fixed[2].is_error);

    // A clean history passes through unchanged.
    let with_result = kagent_core::agent::sanitize_history(&fixed);
    assert_eq!(with_result.len(), 4);
}

#[tokio::test]
async fn a_turn_block_reaches_every_request_of_its_turn_only_and_is_not_stored() {
    // Scenario S20 item 5, the loop's part: `/skill request` gives the turn the skill body.
    let block = "\nSkill 'alpha' (named by the user):\nRun the checks.\n";
    let spans = Arc::new(Mutex::new(Spans::default()));
    let first = ModelResponse { tool_calls: vec![call("1", "probe_a")], ..Default::default() };
    let second = ModelResponse { text: "done".into(), ..Default::default() };
    let r = rig(vec![first, second, ModelResponse { text: "plain".into(), ..Default::default() }], spans).await;

    let outcome = r.agent
        .send_with(&r.session, "/alpha check it".into(), Default::default(), TurnOpts { system_suffix: Some(block.to_string()), ..Default::default() })
        .await
        .unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!("nothing runs, so the turn starts") };
    let result = handle.wait().await;
    assert_eq!(result.status, kagent_core::protocol::TurnStatus::Completed, "{:?}", result.error);

    // Every model request of the turn carried the block, at the end of the prompt.
    let systems = r.model.systems.lock().unwrap().clone();
    assert_eq!(systems.len(), 2, "a tool call, then the answer");
    assert!(systems.iter().all(|s| s.ends_with(block)), "{systems:?}");
    // The stored user message is only the text; the body is nowhere in the session.
    let messages = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(messages[0].content, "/alpha check it");
    assert!(!messages.iter().any(|m| m.content.contains("Run the checks.")), "the body is not stored");

    // The block does not outlive its turn.
    let outcome = r.agent.send(&r.session, "and now?".into(), Default::default()).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!() };
    handle.wait().await;
    let systems = r.model.systems.lock().unwrap().clone();
    assert_eq!(systems.len(), 3, "two requests, then one for the plain turn");
    assert!(!systems[2].contains("Run the checks."), "{systems:?}");
}

#[tokio::test]
async fn a_block_of_input_queued_behind_a_running_turn_joins_its_next_request() {
    let block = "\nSkill 'beta' (named by the user):\nBeta steps.\n";
    let spans = Arc::new(Mutex::new(Spans::default()));
    // The first call runs 150 ms (`probe_a`): long enough to queue a second message behind it.
    let first = ModelResponse { tool_calls: vec![call("1", "probe_a")], ..Default::default() };
    let second = ModelResponse { text: "both done".into(), ..Default::default() };
    let r = rig(vec![first, second], spans).await;

    let outcome = r.agent.send(&r.session, "first".into(), Default::default()).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!() };
    // Let the turn make its first request (the tool call then runs 150 ms).
    let deadline = Instant::now() + Duration::from_secs(5);
    while r.model.systems.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the first request never came");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let queued = r.agent
        .send_with(&r.session, "/beta second".into(), Default::default(), TurnOpts { system_suffix: Some(block.to_string()), ..Default::default() })
        .await
        .unwrap();
    assert!(matches!(queued, SendOutcome::Queued), "a turn is running");
    let result = handle.wait().await;
    assert_eq!(result.status, kagent_core::protocol::TurnStatus::Completed, "{:?}", result.error);

    let systems = r.model.systems.lock().unwrap().clone();
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(systems.len(), 2, "the queued message steers the running turn; no third turn");
    assert!(!systems[0].contains("Beta steps."), "not before the message arrived");
    assert!(systems[1].ends_with(block), "{}", systems[1]);
    // Every request that sees the message has the block, and no other does.
    for (messages, system) in requests.iter().zip(&systems) {
        let seen = messages.iter().any(|m| m.role == Role::User && m.content == "/beta second");
        assert_eq!(system.contains("Beta steps."), seen);
    }
}

// Silence the unused warning for ToolCallStatus import used via full path.
#[allow(dead_code)]
fn _touch() -> Option<ToolCallStatus> {
    None
}

#[tokio::test]
async fn disabled_tool_group_is_refused_not_just_hidden() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let r = rig(vec![], spans.clone()).await;
    // probe_* tools map to group "other"; enabling only "system" disables them.
    r.agent.set_tool_groups(Some(vec!["system".into()]));
    let cancel = tokio_util::sync::CancellationToken::new();
    let (msg, status) = r
        .agent
        .execute_call(&r.session, "t", &call("x", "probe_a"), &cancel, kagent_core::tool::PrivilegeMethod::None)
        .await;
    assert_eq!(status, kagent_core::protocol::ToolCallStatus::Denied);
    assert!(msg.content.contains("disabled"), "{}", msg.content);
    assert!(spans.lock().unwrap().entries.is_empty(), "disabled tool must not run");
    // Re-enabling all groups lets it run.
    r.agent.set_tool_groups(None);
    let (_, status) = r
        .agent
        .execute_call(&r.session, "t", &call("y", "probe_a"), &cancel, kagent_core::tool::PrivilegeMethod::None)
        .await;
    assert_eq!(status, kagent_core::protocol::ToolCallStatus::Ok);
}
