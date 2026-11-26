//! The tools over the subagents and the wait of `agent_run`: `agent_wait` and `agent_list` speak of the children of their session; `agent_stop` cancels a running child and a queued one; `background: false` waits for the answer, then lets the child go on. Part of `tests/subagents`.

use super::rig::*;
use std::sync::Arc;
use std::time::Duration;

use kagent_core::agent::children::ChildStatus;
use kagent_core::permission::PermissionMode;
use kagent_core::protocol::{ToolCallStatus, TurnStatus};
use serde_json::{json, Value};
use tokio::sync::Notify;

#[tokio::test]
async fn agent_wait_and_agent_list_speak_of_the_children_of_their_session() {
    let release = Arc::new(Notify::new());
    let r = rig(&[("parent", vec![run("job-w"), call("agent_wait", json!({})), call("agent_list", json!({})), say("done"), say("after")]), ("job-w", vec![gated(&release, "waited for")])], PermissionMode::Ask).await;
    let handle = started(&r, "one job").await;
    until(10, "the parent waits", || r.model.of("parent").len() == 2).await;
    let child = children(&r).remove(0);
    release.notify_one();
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let waited = results(&r, &r.session, "agent_wait").await.remove(0).content;
    assert!(waited.starts_with("[Agent \"job-w\" done in ") && waited.ends_with(&format!(" s]\nagent {}, profile general\nwaited for\n", child.id)), "{waited}");
    let listed = results(&r, &r.session, "agent_list").await.remove(0).content;
    assert!(listed.starts_with(&format!("[1] {} general done (", child.id)) && listed.ends_with(" s)"), "{listed}");
    // Scoped to the session, and to what there is.
    let (text, status) = run_tool(&r, &r.session, "agent_wait", json!({"ids": ["nope"]})).await;
    assert_eq!((status, text.as_str()), (ToolCallStatus::Error, "no subagent of this session has the id 'nope': agent_list names them"));
    assert_eq!(run_tool(&r, "elsewhere", "agent_list", json!({})).await.0, "this session started no subagents");
    assert_eq!(run_tool(&r, "elsewhere", "agent_wait", json!({})).await.0, "this session started no subagents: start one with agent_run");
    assert!(run_tool(&r, &r.session, "agent_wait", json!({"ids": (0..9).map(|i| i.to_string()).collect::<Vec<_>>()})).await.0.contains("at most 8"), "nine at a time is one too many");
}

#[tokio::test]
async fn agent_stop_cancels_a_running_child_and_a_queued_one() {
    // One slot: the first job runs, the other two wait.
    let rp = Arc::new(Notify::new());
    let r = rig(
        &[
            ("parent", vec![run("job-s1"), run("job-s2"), run("job-s3"), gated(&rp, "working"), say("after")]),
            ("job-s1", vec![hold()]),
            ("job-s2", vec![hold()]),
            ("job-s3", vec![hold()]),
        ],
        PermissionMode::Ask,
    )
    .await;
    r.agent.set_max_concurrent_children(1).await;
    let handle = started(&r, "three jobs").await;
    let kids = kids(&r, 3).await;
    let (first, waiting, last) = (kids[0].id.clone(), kids[1].id.clone(), kids[2].id.clone());
    until(10, "the parent's fourth request, held", || r.model.of("parent").len() == 4).await;
    assert_eq!(statuses(&r, &first), [ChildStatus::Running]);
    assert_eq!(statuses(&r, &waiting), [ChildStatus::Queued]);

    // A queued child: cancelled, and never started. The others are not touched.
    assert_eq!(run_tool(&r, &r.session, "agent_stop", json!({"id": waiting})).await, ("stopped".to_string(), ToolCallStatus::Ok));
    assert_eq!(statuses(&r, &waiting), [ChildStatus::Queued, ChildStatus::Cancelled]);
    assert!(r.model.of("job-s2").is_empty(), "it never made a request");
    assert_eq!(statuses(&r, &first), [ChildStatus::Running], "one is stopped, not the others");
    assert_eq!(statuses(&r, &last), [ChildStatus::Queued]);

    // A running child: its turn is cancelled; the slot it frees goes to the next in turn.
    assert_eq!(run_tool(&r, &r.session, "agent_stop", json!({"id": first})).await, ("stopped".to_string(), ToolCallStatus::Ok));
    wait_status(&r, &first, ChildStatus::Cancelled).await;
    wait_status(&r, &last, ChildStatus::Running).await;
    assert_eq!(statuses(&r, &waiting), [ChildStatus::Queued, ChildStatus::Cancelled], "the cancelled one stays where it is");
    assert_eq!(run_tool(&r, &r.session, "agent_stop", json!({"id": last})).await, ("stopped".to_string(), ToolCallStatus::Ok));
    wait_status(&r, &last, ChildStatus::Cancelled).await;
    until(10, "the cancelled turns ended", || finished(&r, &first) == 1 && finished(&r, &last) == 1).await;

    // Only children of the calling session, and only while they run or wait.
    let (text, status) = run_tool(&r, "elsewhere", "agent_stop", json!({"id": first})).await;
    assert_eq!((status, text.as_str()), (ToolCallStatus::Error, format!("no subagent '{first}' belongs to this session").as_str()));
    assert_eq!(run_tool(&r, &r.session, "agent_stop", json!({"id": "nope"})).await.1, ToolCallStatus::Error);
    assert!(run_tool(&r, &r.session, "agent_stop", json!({"id": waiting})).await.0.contains("the subagent already ended (cancelled)"));
    assert!(notes(&r, &r.session).await.is_empty(), "a stopped child reports nothing");
    rp.notify_one();
    assert_eq!(handle.wait().await.status, TurnStatus::Completed);
}

#[tokio::test]
async fn background_false_waits_for_the_answer_and_past_the_wait_the_child_goes_on() {
    let (slow, rp) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let r = rig(
        &[
            ("parent", vec![pair(("agent_run", json!({"description": "job-f", "prompt": "quick", "background": false})), ("agent_run", json!({"description": "job-slow", "prompt": "slow", "background": false}))), gated(&rp, "working"), say("after")]),
            ("job-f", vec![say("fast answer")]),
            ("job-slow", vec![gated(&slow, "slow answer")]),
        ],
        PermissionMode::Ask,
    )
    .await;
    // 120 s in the daemon (tools.md section 21).
    r.access.set_run_wait(Duration::from_millis(300));
    let handle = started(&r, "two jobs, one slow").await;
    until(10, "both calls answered", || r.model.of("parent").len() == 2).await;
    // The calls ran next to each other, so the children are looked up by their job.
    let by_title = |title: &str| children(&r).into_iter().find(|c| c.title == title).expect(title);
    let answers = results(&r, &r.session, "agent_run").await;
    assert!(answers[0].content.starts_with("[Agent \"job-f\" done in ") && answers[0].content.ends_with(&format!(" s]\nagent {}, profile general\nfast answer\n", by_title("job-f").id)), "{}", answers[0].content);
    let still: Value = serde_json::from_str(&answers[1].content).unwrap();
    assert_eq!(still["agentId"], json!(by_title("job-slow").id));
    assert_eq!(still["status"], json!("running"));
    assert!(still["note"].as_str().unwrap().contains("still running in the background"), "{still}");
    let slow_id = by_title("job-slow").id;
    assert!(r.agent.is_running(&slow_id).await, "the slow one goes on");

    slow.notify_one();
    wait_status(&r, &slow_id, ChildStatus::Done).await;
    rp.notify_one();
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = r.model.of("parent");
    assert_eq!(seen.len(), 3, "the held request, then one that carries the notifications");
    assert!(seen[2].last_user.starts_with("[Agent \"job-slow\" done in "), "its answer came as steering: {}", seen[2].last_user);
    assert_eq!(notes(&r, &r.session).await.len(), 2, "the first arrived in the call, and as a message too");
}
