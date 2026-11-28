//! Steering (steering.md): what a running worker is doing, and (further down) correcting and interrupting it. On the rig
//! of this folder, with the file tools as probes.

use std::sync::Arc;

use kagent_core::agent::children::live::ActionStatus;
use kagent_core::agent::children::ChildStatus;
use kagent_core::permission::PermissionMode;
use kagent_core::protocol::TurnStatus;
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
