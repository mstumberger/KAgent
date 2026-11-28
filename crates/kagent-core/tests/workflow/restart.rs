//! A daemon restart in each state (workflows.md section 3.5): a second engine reads the folder the first one wrote, as a
//! new daemon would. A run that was planning or working comes back paused and never continues by itself; the rest stay.

use std::sync::Arc;

use kagent_core::workflow::{Decision, Engine, RunState, TaskState};
use tokio::sync::Notify;

use super::rig::*;

/// What a new daemon sees: another engine over the same folder, with what `recover` did.
fn restarted(w: &Wf) -> Arc<Engine> {
    let engine = Engine::new(w.agent.clone(), w.dir.path().join("runs"), Arc::new(lookup_profile), definitions(w.defs.clone()));
    engine.set_differ(Arc::new(|_, files| format!("diff of {}", files.join(","))));
    engine.recover();
    engine
}

async fn plan_ready(w: &Wf, tasks: &[(&str, &[&str], &[&str])]) -> String {
    w.model.script("Plan: build the thing", vec![plan(tasks)]);
    let id = w.start("plan-implement-review", "build the thing").await.id;
    w.until_state(&id, RunState::AwaitingApproval).await;
    id
}

#[tokio::test]
async fn a_run_that_was_planning_comes_back_paused_and_continues_on_request() {
    let w = wf().await;
    w.model.script("Plan: build the thing", vec![Step::Hold, plan(&[("T1", &[], &[])])]);
    let id = w.start("plan-implement-review", "build the thing").await.id;
    until(10, "the planner runs", || w.running_workers(&id) == 1).await;
    let second = restarted(&w);
    let run = second.get(&id).unwrap();
    assert_eq!((run.state, run.reason.as_str()), (RunState::Paused, "the daemon restarted"));
    w.engine.stop_run(&id).await.unwrap();
    until(10, "the old planner is cancelled", || w.running_workers(&id) == 0).await;
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    assert_eq!(w.model.of("Plan: build the thing").len(), 1, "it did not start by itself");
    second.continue_run(&id).await.unwrap();
    until(10, "the new planner's plan", || second.get(&id).is_some_and(|r| r.state == RunState::AwaitingApproval)).await;
    assert_eq!(second.get(&id).unwrap().tasks.len(), 1);
}

#[tokio::test]
async fn a_run_awaiting_approval_stays_and_is_approved_by_the_new_engine() {
    let w = wf().await;
    w.model.script("Implement T1: Title T1", vec![say("done")]);
    w.model.script("Review T1: Title T1", vec![pass()]);
    let id = plan_ready(&w, &[("T1", &[], &[])]).await;
    let second = restarted(&w);
    assert_eq!(second.get(&id).unwrap().state, RunState::AwaitingApproval);
    assert_eq!(second.get(&id).unwrap().tasks.len(), 1, "the plan was stored");
    second.approve_plan(&id, None).unwrap();
    until(10, "done on the new engine", || second.get(&id).is_some_and(|r| r.state == RunState::Done)).await;
}

#[tokio::test]
async fn a_task_that_was_implementing_or_in_review_starts_that_stage_again() {
    // implementing
    let w = wf().await;
    w.model.script("Implement T1: Title T1", vec![Step::Hold, say("report 2")]);
    w.model.script("Review T1: Title T1", vec![pass()]);
    let id = plan_ready(&w, &[("T1", &[], &[])]).await;
    w.engine.approve_plan(&id, None).unwrap();
    until(10, "the implementer runs", || w.running_workers(&id) == 1).await;
    let second = restarted(&w);
    assert_eq!((second.get(&id).unwrap().state, second.get(&id).unwrap().tasks[0].state), (RunState::Paused, TaskState::Implementing));
    w.engine.stop_run(&id).await.unwrap();
    second.continue_run(&id).await.unwrap();
    until(10, "done", || second.get(&id).is_some_and(|r| r.state == RunState::Done)).await;
    assert!(w.model.of("Implement T1: Title T1")[1].users[0].contains("An earlier attempt at this task was interrupted"));

    // in review
    let w = wf().await;
    let hold = Arc::new(Notify::new());
    w.model.script("Implement T1: Title T1", vec![say("the report")]);
    w.model.script("Review T1: Title T1", vec![gate(&hold, "never"), pass()]);
    let id = plan_ready(&w, &[("T1", &[], &[])]).await;
    w.engine.approve_plan(&id, None).unwrap();
    w.until_task(&id, "T1", TaskState::InReview).await;
    until(10, "the reviewer runs", || w.running_workers(&id) == 1).await;
    let second = restarted(&w);
    assert_eq!(second.get(&id).unwrap().tasks[0].state, TaskState::InReview);
    w.engine.stop_run(&id).await.unwrap();
    second.continue_run(&id).await.unwrap();
    until(10, "done", || second.get(&id).is_some_and(|r| r.state == RunState::Done)).await;
    let t = second.get(&id).unwrap().tasks[0].clone();
    assert_eq!((t.rounds.len(), t.rounds[0].report.as_str()), (1, "the report"), "the implementer did not run again");
}

#[tokio::test]
async fn a_task_that_was_sent_back_keeps_its_findings_across_a_restart() {
    let w = wf().await;
    w.model.script("Implement T1: Title T1", vec![say("r1"), Step::Hold, say("r2")]);
    w.model.script("Review T1: Title T1", vec![changes("fix the thing"), pass()]);
    let id = plan_ready(&w, &[("T1", &[], &[])]).await;
    w.engine.approve_plan(&id, None).unwrap();
    until(10, "round 2 is running", || w.model.of("Implement T1: Title T1").len() == 2 && w.running_workers(&id) == 1).await;
    let second = restarted(&w);
    let t = second.get(&id).unwrap().tasks[0].clone();
    assert_eq!((t.state, t.round, t.open_findings().len()), (TaskState::Changes, 2, 1));
    w.engine.stop_run(&id).await.unwrap();
    second.continue_run(&id).await.unwrap();
    until(10, "done", || second.get(&id).is_some_and(|r| r.state == RunState::Done)).await;
    let last = w.model.of("Implement T1: Title T1").pop().unwrap();
    assert!(last.users[0].contains("MUST (a.rs:3): fix the thing"), "the new implementer was told the findings: {:?}", last.users);
}

#[tokio::test]
async fn a_task_that_needs_the_owner_and_the_final_states_survive() {
    let w = wf().await;
    for t in ["T1"] {
        w.model.script(&format!("Implement {t}: Title {t}"), vec![say("r1"), say("r2"), say("r3")]);
        w.model.script(&format!("Review {t}: Title {t}"), vec![changes("a"), changes("b"), changes("c")]);
    }
    let id = plan_ready(&w, &[("T1", &[], &[])]).await;
    w.engine.approve_plan(&id, None).unwrap();
    w.until_task(&id, "T1", TaskState::NeedsOwner).await;
    let second = restarted(&w);
    let run = second.get(&id).unwrap();
    assert_eq!((run.state, run.tasks[0].state), (RunState::Paused, TaskState::NeedsOwner), "the decision is still the owner's");
    w.engine.stop_run(&id).await.unwrap();
    second.continue_run(&id).await.unwrap();
    second.decide(&id, "T1", Decision::Accept).unwrap();
    until(10, "done", || second.get(&id).is_some_and(|r| r.state == RunState::Done)).await;
    // Final states stay as they were.
    let third = restarted(&w);
    assert_eq!(third.get(&id).unwrap().state, RunState::Done);
    assert_eq!(third.list(None).len(), 1);
    assert_eq!(third.list(Some("/nowhere")).len(), 0);
}
