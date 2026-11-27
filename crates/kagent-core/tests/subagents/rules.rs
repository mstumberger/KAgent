//! The rules around subagents: deleting a parent cancels and deletes its children; a profile's model, iteration limit and window are applied; a notification wakes a chat, not a task, and not for ever; a background task's end reaches its session through the agent. Part of `tests/subagents`.

use super::rig::*;
use super::{ends_with};
use std::sync::Arc;

use kagent_core::agent::children::{ChildStatus, ModelInfo, Profile};
use kagent_core::permission::PermissionMode;
use kagent_core::protocol::{Role, ToolCallStatus, TurnStatus, Via};
use kagent_core::session::SessionOrigin;
use serde_json::{json, Value};
use tokio::sync::Notify;

#[tokio::test]
async fn deleting_a_parent_cancels_and_deletes_its_children() {
    let rp = Arc::new(Notify::new());
    let r = rig(&[("parent", vec![run("job-x"), gated(&rp, "working")]), ("job-x", vec![hold()])], PermissionMode::Ask).await;
    let handle = started(&r, "one job").await;
    let child = kids(&r, 1).await.remove(0);
    wait_status(&r, &child.id, ChildStatus::Running).await;
    until(10, "the parent's second request, held", || r.model.of("parent").len() == 2).await;

    r.agent.delete_session(&r.session).await.unwrap();
    assert_eq!(handle.wait().await.status, TurnStatus::Cancelled);
    assert!(r.store.list().await.unwrap().is_empty(), "the parent and its children are gone");
    assert!(r.store.meta(&child.id).await.is_err() && r.store.meta(&r.session).await.is_err());
    // The child's turn ends in a task of its own, after the parent's: wait for the event.
    wait_status(&r, &child.id, ChildStatus::Cancelled).await;
    assert_eq!(statuses(&r, &child.id), [ChildStatus::Running, ChildStatus::Cancelled]);
    assert!(children(&r).is_empty());
}

#[tokio::test]
async fn a_profiles_model_and_iteration_limit_are_applied_and_its_window_is_read() {
    let r = rig(&[("parent", vec![run_as("job-m", "small"), say("started"), say("after")]), ("job-m", vec![call("probe_a", json!({})), call("probe_a", json!({})), call("probe_a", json!({})), call("probe_a", json!({})), say("never")])], PermissionMode::Ask).await;
    r.access.set_profiles(vec![Profile { name: "small".into(), model: "small::model".into(), max_iterations: 3, prompt: "  Be brief.  ".into(), ..Profile::general() }]);
    // What the daemon reads from its providers: the window of that model.
    r.agent.set_model_window(Arc::new(|model_ref: &str| (model_ref == "small::model").then_some(ModelInfo { context_tokens: 256, max_output_tokens: Some(256) })));
    turn(&r, "a small model").await;
    wait_turns(&r, &r.session, 2).await;

    let mine = r.model.of("job-m");
    assert_eq!(mine.len(), 3, "stopped after 3 model requests (the profile's maxIterations), not 8");
    assert!(mine.iter().all(|s| s.model_ref == "small::model"), "the profile's model");
    assert_eq!(mine[0].tools, ["probe_a", "probe_b", "probe_c"], "the window of that model decides which groups fit: a 256-token window has no room for the agent group (with the chat model's, every tool is offered)");
    assert!(mine[0].system.ends_with("\nBe brief.\n"), "the profile prompt is the last part: {}", mine[0].system);
    let note = notes(&r, &r.session).await;
    let child = children(&r).remove(0);
    assert_eq!(note.len(), 1);
    assert!(note[0].starts_with("[Agent \"job-m\" failed in ") && note[0].contains(&format!("agent {}, profile small\nerror: stopped after 3 model requests (max iterations)", child.id)), "{}", note[0]);
    assert_eq!(statuses(&r, &child.id), [ChildStatus::Running, ChildStatus::Failed]);
    ends_with(&r, "after").await;

    // A model that is not a ref, or that the providers do not know, is refused before a
    // session is made (the local model passes a profile name as `model` in about half of its calls).
    let (text, status) = run_tool(&r, &r.session, "agent_run", json!({"description": "d", "prompt": "p", "model": "general"})).await;
    assert_eq!((status, text.as_str()), (ToolCallStatus::Error, "the subagent was not started: model 'general' is not a model the providers know: pass a model ref (<providerId>::<modelId>), or leave `model` out (the profile's model, else the chat model)"));
    assert_eq!(run_tool(&r, &r.session, "agent_run", json!({"description": "d", "prompt": "p", "model": "nope::x"})).await.1, ToolCallStatus::Error, "a ref the providers do not know");
    assert_eq!(children(&r).len(), 1, "nothing was started for either");
}

#[tokio::test]
async fn a_notification_wakes_a_chat_but_not_a_task_and_not_for_ever() {
    let r = rig(&[("parent", vec![say("auto"); 5])], PermissionMode::Ask).await;
    // The session of a scheduled task: nobody is at it, so the message is stored and no turn runs.
    let task = r.store.create_with("Task: report", SessionOrigin::Task, None).await.unwrap().id;
    let told = r.agent.notify(&task, "[Task bg_1a2b3c4d finished, exit 0]\nout\n").await.unwrap();
    assert!(!told, "no turn started");
    let stored = messages(&r, &task).await;
    assert_eq!(stored.len(), 1, "stored, and only stored: {stored:?}");
    assert_eq!((stored[0].role, stored[0].via, stored[0].content.as_str()), (Role::User, Via::Agent, "[Task bg_1a2b3c4d finished, exit 0]\nout\n"));
    assert!(r.model.seen().is_empty(), "no request was made");

    // A chat is woken, five times in a row at most.
    for i in 0..5 {
        assert!(r.agent.notify(&r.session, &format!("[Agent \"job-{i}\" done in 1 s]\nagent x, profile general\nanswer {i}")).await.unwrap(), "turn {i} starts");
        wait_turns(&r, &r.session, i + 1).await;
    }
    let sixth = r.agent.notify(&r.session, "[Agent \"job-5\" done in 1 s]\nagent x, profile general\nanswer 5").await.unwrap();
    assert!(!sixth, "the sixth in a row is stored, and does not start a turn");
    assert_eq!(notes(&r, &r.session).await.len(), 6);
    assert_eq!(r.model.of("parent").len(), 5);
    assert_eq!(started_turns(&r, &r.session), 5);
}

#[tokio::test]
async fn a_background_task_tells_its_session_through_the_agent() {
    // `shell {background: true}` answers at once; `task_output` reads it; the end is a message like a subagent's.
    let rp = Arc::new(Notify::new());
    let r = rig(&[("parent", vec![call("shell", json!({"command": "echo line-one; echo line-two >&2; exit 3", "background": true})), gated(&rp, "seen"), say("after")])], PermissionMode::Full).await;
    let handle = started(&r, "a background job").await;
    until(10, "the shell call answered", || r.model.of("parent").len() == 2).await;
    let started: Value = serde_json::from_str(&results(&r, &r.session, "shell").await[0].content).unwrap();
    let id = started["taskId"].as_str().unwrap().to_string();
    assert!(id.starts_with("bg_") && started["outputFile"].as_str().unwrap().ends_with(&format!("{id}.log")), "{started}");
    let (text, status) = run_tool(&r, &r.session, "task_output", json!({"task_id": id, "wait_sec": 5})).await;
    assert_eq!(status, ToolCallStatus::Ok, "{text}");
    assert!(text.starts_with(&format!("[Task {id} failed, exit 3, after")) && text.contains("line-one") && text.contains("line-two"), "{text}");

    // A task that is stopped tells nobody.
    let started: Value = serde_json::from_str(&run_tool(&r, &r.session, "shell", json!({"command": "sleep 45", "background": true})).await.0).unwrap();
    let killed = started["taskId"].as_str().unwrap().to_string();
    until(5, "the command runs", || running("sleep 45") == 1).await;
    assert_eq!(run_tool(&r, &r.session, "task_stop", json!({"task_id": killed})).await.0, format!("[Task {killed} killed]"));
    until(4, "the killed command is gone", || running("sleep 45") == 0).await;

    rp.notify_one();
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = r.model.of("parent");
    assert!(seen[2].last_user.starts_with(&format!("[Task {id} finished, exit 3]\n")) && seen[2].last_user.contains("line-one"), "{}", seen[2].last_user);
    assert_eq!(notes(&r, &r.session).await.len(), 1, "the killed task sent no message");
}

#[tokio::test]
async fn an_approval_card_of_a_subagent_carries_the_name_of_its_job() {
    // Scenario S31 item 4, agents.md section 2: the child uses the permission engine with
    // its own session id, and its card says whose call it is.
    let r = rig(&[("parent", vec![run("job-card"), say("after")]), ("job-card", vec![call("probe_c", json!({})), say("done")])], PermissionMode::Ask).await;
    let handle = started(&r, "one job").await;
    until(10, "the child's card", || !r.agent.broker().pending().is_empty()).await;
    let child = children(&r).remove(0);
    let card = r.agent.broker().pending().remove(0);
    assert_eq!((card.session_id.as_str(), card.tool.as_str()), (child.id.as_str(), "probe_c"), "the card is the child's own");
    let plain = r.agent.tools().get("probe_c").unwrap().assess(&json!({})).summary;
    assert_eq!(card.summary, format!("subagent \"job-card\": {plain}"), "named after the subagent, then the call");
    // The same call by a session that is not a subagent keeps its summary.
    assert!(!plain.starts_with("subagent"), "{plain}");
    r.agent.broker().resolve(&card.request_id, kagent_core::permission::ApprovalDecision::Deny { reason: None }, kagent_core::permission::ApprovalVia::Click);
    assert_eq!(handle.wait().await.status, TurnStatus::Completed);
    wait_status(&r, &child.id, ChildStatus::Done).await;
    assert!(results(&r, &child.id, "probe_c").await[0].is_error, "the denied call did not run");
}

/// Processes running exactly `command`.
fn running(command: &str) -> usize {
    std::process::Command::new("pgrep")
        .args(["-c", "-f", &format!("^{command}$")])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0))
        .unwrap_or(0)
}
