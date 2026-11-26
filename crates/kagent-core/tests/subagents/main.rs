//! Scenario S31 items 1 and 2 and the scripted-model tests of agents.md section 7, in four
//! files: this one has a child's answer reaching an idle parent as a new turn and a busy one
//! as steering, the concurrency limit, stop-all, the depth limit, the tool groups of a
//! profile and the read-only `explore`, and a child of a planning session. `tools.rs` has
//! `agent_wait`, `agent_list`, `agent_stop` and `background: false`; `rules.rs` the delete
//! of a parent, a profile's model, whom a notification wakes and a background task's
//! message; `rig.rs` is the rig (the scripted model and the tools, on `../common`).

#[path = "../common/mod.rs"]
mod common;
mod rig;
mod rules;
mod tools;

use std::sync::Arc;

use kagent_core::agent::children::{ChildRequest, ChildStatus, Profile};
use kagent_core::permission::PermissionMode;
use kagent_core::protocol::{ToolCallStatus, TurnStatus};
use kagent_core::session::{SessionMeta, SessionOrigin, SessionStore};
use rig::*;
use serde_json::{json, Value};
use tokio::sync::Notify;

pub fn ids(metas: Vec<SessionMeta>) -> Vec<String> {
    metas.into_iter().map(|m| m.id).collect()
}

/// The last stored message of the session is the assistant's `text`.
pub async fn ends_with(r: &Rig, text: &str) {
    let stored = messages(r, &r.session).await;
    assert_eq!(stored.last().unwrap().content, text, "{stored:?}");
}

#[tokio::test]
async fn a_finished_child_gives_an_idle_parent_a_new_turn_with_the_notification() {
    let r = rig(
        &[
            ("parent", vec![call("agent_run", json!({"description": "job-a", "prompt": ["do", "it"]})), say("started it"), say("the final summary")]),
            ("job-a", vec![say("the answer from the child")]),
        ],
        PermissionMode::Ask,
    )
    .await;
    let result = turn(&r, "please research").await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);

    // The child: a session of its own under the parent (agents.md section 2).
    let children = kids(&r, 1).await;
    let child = &children[0];
    assert_eq!((child.title.as_str(), child.profile.as_str()), ("job-a", "general"));
    // The parent's turn is over; the child's goes on a moment longer.
    wait_status(&r, &child.id, ChildStatus::Done).await;
    assert_eq!(child.status, ChildStatus::Done);
    let meta = r.store.meta(&child.id).await.unwrap();
    assert_eq!((meta.origin, meta.parent.as_deref(), meta.title.as_str()), (SessionOrigin::Subagent, Some(r.session.as_str()), "job-a"));
    let all = r.store.list().await.unwrap();
    assert_eq!(ids(SessionStore::visible(all.clone(), None)), vec![r.session.clone()], "hidden from the main list");
    assert_eq!(ids(SessionStore::visible(all, Some(&r.session))), vec![child.id.clone()], "listed under its parent");
    assert_eq!(statuses(&r, &child.id), [ChildStatus::Running, ChildStatus::Done], "the UI is told of every state");

    // What `agent_run` answered at once: the id, and that the job runs.
    let answer: Value = serde_json::from_str(&results(&r, &r.session, "agent_run").await[0].content).unwrap();
    assert_eq!(answer, json!({"agentId": child.id, "status": "running", "title": "job-a", "profile": "general"}));

    // The parent was idle: the notification started a turn, and the model saw it.
    wait_turns(&r, &r.session, 2).await;
    assert_eq!(started_turns(&r, &r.session), 2, "a new turn of the parent, started by the notification");
    let seen = r.model.of("parent");
    assert_eq!(seen.len(), 3, "two requests of the first turn, one of the second");
    assert_eq!(seen[2].system, "BASE PROMPT.", "the parent's prompt is its own");
    let note = &seen[2].last_user;
    assert!(note.starts_with("[Agent \"job-a\" done in ") && note.ends_with(&format!(" s]\nagent {}, profile general\nthe answer from the child\n", child.id)), "{note}");
    assert_eq!(notes(&r, &r.session).await, vec![note.clone()], "stored once, as a user message with Via::Agent");
    ends_with(&r, "the final summary").await;

    // What the child was told and offered.
    let mine = r.model.of("job-a");
    assert_eq!(mine.len(), 1);
    assert!(mine[0].system.starts_with("BASE PROMPT.\nYou are a subagent. job-a. End with a complete final answer: it is returned to the agent that started you.\n"), "{}", mine[0].system);
    assert_eq!(mine[0].model_ref, "fake::test", "general: the chat model");
    assert_eq!(mine[0].last_user, "What the conversation you were started from is about (a digest):\nUser: please research\n\nYour job:\ndo\nit", "the parent's digest, then the job (its lines joined)");
    let mut offered = mine[0].tools.clone();
    offered.sort();
    assert_eq!(offered, ["probe_a", "probe_b", "probe_c", "shell", "task_output", "task_stop"], "every tool but those over subagents");
}

#[tokio::test]
async fn a_busy_parent_gets_the_notification_as_steering() {
    let (release, answer) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let r = rig(
        &[("parent", vec![run("job-b"), gated(&release, "working"), say("after the child")]), ("job-b", vec![gated(&answer, "answer b")])],
        PermissionMode::Ask,
    )
    .await;
    let handle = started(&r, "start a helper").await;
    until(10, "the parent's second request, held", || r.model.of("parent").len() == 2).await;
    let child = kids(&r, 1).await.remove(0);
    // The child ends while the parent's second request is held.
    answer.notify_one();
    wait_status(&r, &child.id, ChildStatus::Done).await;
    assert!(r.agent.is_running(&r.session).await, "the parent is busy");
    assert!(notes(&r, &r.session).await.is_empty(), "not stored yet: steering waits for the next model request");

    release.notify_one();
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = r.model.of("parent");
    assert_eq!(seen.len(), 3, "the held request, then one that carries the notification");
    assert!(seen[1].last_user == "start a helper", "the held request did not have it");
    assert!(seen[2].last_user.starts_with("[Agent \"job-b\" done in ") && seen[2].last_user.ends_with("answer b\n"), "{}", seen[2].last_user);
    assert_eq!(started_turns(&r, &r.session), 1, "one turn: the notification steered it and started none");
    let stored: Vec<String> = messages(&r, &r.session).await.iter().map(|m| format!("{:?}:{:?}:{}", m.role, m.via, m.content)).collect();
    assert_eq!(stored[3..], vec!["Assistant:Text:working".to_string(), format!("User:Agent:{}", seen[2].last_user), "Assistant:Text:after the child".to_string()], "stored in order: after the held answer, before the next");
}

#[tokio::test]
async fn the_concurrency_limit_queues_children_until_a_slot_frees() {
    let (r1, r2, r3, rp) = (Arc::new(Notify::new()), Arc::new(Notify::new()), Arc::new(Notify::new()), Arc::new(Notify::new()));
    let r = rig(
        &[
            ("parent", vec![run("job-1"), run("job-2"), run("job-3"), gated(&rp, "working"), say("after")]),
            ("job-1", vec![gated(&r1, "one")]),
            ("job-2", vec![gated(&r2, "two")]),
            ("job-3", vec![gated(&r3, "three")]),
        ],
        PermissionMode::Ask,
    )
    .await;
    r.agent.set_max_concurrent_children(2).await;
    let handle = started(&r, "three jobs").await;
    until(10, "the parent's fourth request, held", || r.model.of("parent").len() == 4).await;
    let kids = children(&r);
    assert_eq!(kids.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["job-1", "job-2", "job-3"], "in the order they were started");
    let states: Vec<Value> = results(&r, &r.session, "agent_run").await.iter().map(|m| serde_json::from_str::<Value>(&m.content).unwrap()["status"].clone()).collect();
    assert_eq!(states, [json!("running"), json!("running"), json!("queued")], "two run; the third is queued and said to be");
    assert_eq!(statuses(&r, &kids[2].id), [ChildStatus::Queued]);
    assert!(r.model.of("job-3").is_empty(), "a queued child makes no request");

    // A slot frees: the child in turn starts.
    r1.notify_one();
    wait_status(&r, &kids[0].id, ChildStatus::Done).await;
    wait_status(&r, &kids[2].id, ChildStatus::Running).await;
    until(10, "the queued child's first request", || r.model.of("job-3").len() == 1).await;
    assert_eq!(statuses(&r, &kids[2].id), [ChildStatus::Queued, ChildStatus::Running]);
    r2.notify_one();
    r3.notify_one();
    wait_status(&r, &kids[1].id, ChildStatus::Done).await;
    wait_status(&r, &kids[2].id, ChildStatus::Done).await;

    // The parent was busy all along: three notifications, all steering the one turn.
    rp.notify_one();
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = r.model.of("parent");
    assert_eq!(seen.len(), 5);
    assert!(seen[4].last_user.starts_with("[Agent \"job-"), "{}", seen[4].last_user);
    assert_eq!(notes(&r, &r.session).await.len(), 3);
    assert_eq!(started_turns(&r, &r.session), 1);
}

#[tokio::test]
async fn stop_all_cancels_a_running_child_and_a_queued_one_never_starts() {
    let r = rig(&[("parent", vec![run("job-k1"), run("job-k2"), hold()]), ("job-k1", vec![hold()])], PermissionMode::Ask).await;
    r.agent.set_max_concurrent_children(1).await;
    let handle = started(&r, "two jobs").await;
    let kids = kids(&r, 2).await;
    wait_status(&r, &kids[0].id, ChildStatus::Running).await;
    wait_status(&r, &kids[1].id, ChildStatus::Queued).await;
    until(10, "the parent's third request, held", || r.model.of("parent").len() == 3).await;

    let cancelled = r.agent.stop_all().await;
    assert_eq!(cancelled, 2, "the parent's turn and the running child's");
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Cancelled);
    wait_status(&r, &kids[0].id, ChildStatus::Cancelled).await;
    assert_eq!(statuses(&r, &kids[1].id), [ChildStatus::Queued, ChildStatus::Cancelled], "the queued one is cancelled too, and never started");
    assert!(r.model.of("job-k2").is_empty());
    until(10, "the running child's turn is cancelled", || finished(&r, &kids[0].id) == 1).await;
    assert!(!r.agent.is_running(&r.session).await && !r.agent.is_running(&kids[0].id).await);
    assert!(notes(&r, &r.session).await.is_empty(), "a cancelled child tells nobody: the kill switch starts no turn");
    assert_eq!(statuses(&r, &kids[1].id), [ChildStatus::Queued, ChildStatus::Cancelled], "and the slot the running one freed does not start it");
}

#[tokio::test]
async fn a_subagent_cannot_start_subagents() {
    // The depth limit (agents.md section 2): not among its tools, refused when it calls one, and refused by the agent itself.
    let r = rig(
        &[
            ("parent", vec![run("job-d"), say("started"), say("after")]),
            ("job-d", vec![call("agent_run", json!({"description": "grandchild", "prompt": "deeper"})), say("no subagent of mine")]),
        ],
        PermissionMode::Ask,
    )
    .await;
    turn(&r, "delegate").await;
    let child = children(&r).remove(0);
    wait_status(&r, &child.id, ChildStatus::Done).await;
    let answer = results(&r, &child.id, "agent_run").await.remove(0);
    assert!(answer.is_error && answer.content.starts_with("not executed: a subagent cannot start or manage subagents"), "{}", answer.content);
    assert!(r.agent.children_of(&child.id).is_empty() && children(&r).len() == 1, "no grandchild");
    assert!(!r.model.seen().iter().any(|s| s.tag == "grandchild"));
    let deeper = r.agent
        .start_child(ChildRequest { parent: child.id.clone(), description: "by hand".into(), prompt: "p".into(), profile: Profile::general(), model: None })
        .await
        .err()
        .expect("the agent refuses it too");
    assert!(format!("{deeper:#}").contains("a subagent cannot start subagents"), "{deeper:#}");
    // And a session that does not exist.
    let nowhere = r.agent.start_child(ChildRequest { parent: "no-such-session".into(), description: "d".into(), prompt: "p".into(), profile: Profile::general(), model: None }).await.err().unwrap();
    assert!(format!("{nowhere:#}").contains("there is no session 'no-such-session'"), "{nowhere:#}");
}

#[tokio::test]
async fn the_profile_decides_the_tools_and_explore_is_read_only() {
    // Scenario S31 item 2. Full control: without the restriction everything would run.
    let rp = Arc::new(Notify::new());
    let r = rig(
        &[
            ("parent", vec![run_as("job-g", "narrow"), run_as("job-e", "explore"), gated(&rp, "working"), say("after")]),
            ("job-g", vec![call("probe_a", json!({})), say("narrow done")]),
            ("job-e", vec![call("probe_c", json!({})), call("probe_a", json!({})), say("explore done")]),
        ],
        PermissionMode::Full,
    )
    .await;
    r.access.set_profiles(vec![Profile { name: "narrow".into(), tool_groups: vec!["system".into()], ..Profile::general() }]);
    let handle = started(&r, "two profiles").await;
    let kids = kids(&r, 2).await;
    wait_status(&r, &kids[0].id, ChildStatus::Done).await;
    wait_status(&r, &kids[1].id, ChildStatus::Done).await;

    // A profile with groups: a call outside them is refused when it runs.
    let narrow = results(&r, &kids[0].id, "probe_a").await.remove(0);
    assert!(narrow.is_error && narrow.content.starts_with("not executed: tool 'probe_a' is not in this subagent's tool groups (system)"), "{}", narrow.content);
    let offered = r.model.of("job-g")[0].tools.clone();
    assert!(!offered.iter().any(|t| t.starts_with("probe_")), "not offered either: {offered:?}");
    assert!(r.model.of("job-g")[0].system.contains("Your tool groups are only: system."), "and the prompt says so");

    // `explore`: read-only. The mutating call is refused, the read-only one runs.
    let mutation = results(&r, &kids[1].id, "probe_c").await.remove(0);
    assert!(mutation.is_error && mutation.content.starts_with("not executed: explore is read-only"), "{}", mutation.content);
    assert!(!results(&r, &kids[1].id, "probe_a").await[0].is_error);
    let ran: Vec<&str> = r.spans.lock().unwrap().entries.iter().map(|e| e.0).collect();
    assert_eq!(ran, ["probe_a"], "only the read-only probe ran, once");
    assert!(r.model.of("job-e")[0].system.contains("You are read-only: every call that would change something is refused."));

    // The same two setters on a session that is not a subagent.
    assert_eq!(run_tool(&r, &r.session, "probe_c", json!({})).await.1, ToolCallStatus::Ok);
    r.agent.set_session_read_only(&r.session, true);
    let (text, status) = run_tool(&r, &r.session, "probe_c", json!({})).await;
    assert_eq!((status, text.as_str()), (ToolCallStatus::Denied, "not executed: explore is read-only: this call would change something. Research with read-only tools, then answer"));
    assert_eq!(run_tool(&r, &r.session, "probe_a", json!({})).await.1, ToolCallStatus::Ok);
    r.agent.set_session_read_only(&r.session, false);
    r.agent.set_session_tool_groups(&r.session, Some(vec!["system".into()]));
    let (text, status) = run_tool(&r, &r.session, "probe_a", json!({})).await;
    assert_eq!(status, ToolCallStatus::Denied, "{text}");
    assert!(text.starts_with("not executed: tool 'probe_a' is not in this subagent's tool groups (system)"), "{text}");
    r.agent.set_session_tool_groups(&r.session, None);
    assert_eq!(run_tool(&r, &r.session, "probe_a", json!({})).await.1, ToolCallStatus::Ok);

    // What the tool refuses: a profile that does not exist (the others are named), no description.
    rp.notify_one();
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let (text, status) = run_tool(&r, &r.session, "agent_run", json!({"description": "x", "prompt": "y", "profile": "nope"})).await;
    assert_eq!((status, text.as_str()), (ToolCallStatus::Error, "unknown profile 'nope': general, explore, narrow"));
    let (text, status) = run_tool(&r, &r.session, "agent_run", json!({"description": "", "prompt": "y"})).await;
    assert_eq!((status, text.as_str()), (ToolCallStatus::Error, "the subagent was not started: the description of the job is needed: at most 80 characters"));
    assert_eq!(children(&r).len(), 2, "nothing was started");
}

#[tokio::test]
async fn a_child_of_a_planning_session_gets_the_plan_restriction() {
    // plan-todo.md section 2 and agents.md section 2.
    let r = rig(&[("parent", vec![run("job-never"), say("cannot start one")]), ("job-p", vec![call("probe_c", json!({})), call("probe_a", json!({})), say("found")])], PermissionMode::Full).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    let result = turn(&r, "plan something").await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    // `agent_run` is a mutating call that plan mode does not allow (as before this stage):
    // the model cannot start a subagent from a planning session.
    let refused = results(&r, &r.session, "agent_run").await.remove(0);
    assert!(refused.is_error && refused.content.contains(kagent_core::plan::PLAN_DENIED), "{}", refused.content);
    assert!(children(&r).is_empty());

    // Started all the same (by hand here): the child researches only.
    let child = r.agent.start_child(ChildRequest { parent: r.session.clone(), description: "job-p".into(), prompt: "look around".into(), profile: Profile::general(), model: None }).await.unwrap();
    wait_status(&r, &child.id, ChildStatus::Done).await;
    let mutation = results(&r, &child.id, "probe_c").await.remove(0);
    assert!(mutation.is_error && mutation.content.contains(kagent_core::plan::PLAN_DENIED), "{}", mutation.content);
    assert!(!results(&r, &child.id, "probe_a").await[0].is_error, "research goes");
    let meta = r.store.meta(&child.id).await.unwrap();
    let mine = r.model.of("job-p");
    assert!(!meta.planning && !mine[0].system.contains("PLAN MODE"), "the child's own flag stays off: no plan prompt, so no plan card from a subagent");
}
