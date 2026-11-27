//! Decision of 2026-11-27 (plan-todo.md section 2): when an approved plan ends.

use super::*;

fn item(content: &str, status: crate::todo::TodoStatus) -> crate::todo::TodoItemInput {
    crate::todo::TodoItemInput { content: content.into(), status: Some(status), priority: None }
}

/// Decision of 2026-11-27 (plan-todo.md section 2): an approved plan ends when its todo list
/// is all completed.
#[tokio::test]
async fn an_approved_plan_ends_when_the_todo_list_is_all_completed() {
    use crate::todo::TodoStatus::{Completed, InProgress};
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    answered(&r, json!({ "plan": "1. a\n2. b" }), ApprovalDecision::AllowOnce).await;
    r.agent.set_todo(&r.session, vec![item("a", Completed), item("b", InProgress)]).await.unwrap();
    assert!(r.agent.approved_plan(&r.session).await.unwrap().is_some(), "one step is still open");
    r.agent.set_todo(&r.session, vec![]).await.unwrap();
    assert!(r.agent.approved_plan(&r.session).await.unwrap().is_some(), "an empty list is not a finished one");
    r.agent.set_todo(&r.session, vec![item("a", Completed), item("b", Completed)]).await.unwrap();
    assert!(r.agent.approved_plan(&r.session).await.unwrap().is_none(), "all done: the plan is over");
    assert!(r.agent.store().meta(&r.session).await.unwrap().approved_plan.is_none(), "and stored so");
}

#[tokio::test]
async fn an_approved_plan_expires_24_hours_after_the_approval() {
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    answered(&r, json!({ "plan": "1. a" }), ApprovalDecision::AllowOnce).await;
    let state = r.agent.state(&r.session).await.unwrap();
    let set_age = |hours: i64| {
        let state = state.clone();
        async move {
            let mut meta = state.meta.lock().await;
            meta.approved_plan.as_mut().unwrap().approved_at = Some(chrono::Utc::now() - chrono::TimeDelta::hours(hours));
        }
    };
    set_age(23).await;
    assert!(r.agent.approved_plan(&r.session).await.unwrap().is_some(), "23 h: still in force");
    set_age(24).await;
    assert!(r.agent.approved_plan(&r.session).await.unwrap().is_none(), "24 h: expired");
    assert!(r.agent.store().meta(&r.session).await.unwrap().approved_plan.is_none(), "dropped from the stored meta");
}

#[test]
fn a_plan_without_an_approval_time_counts_as_expired() {
    let now = chrono::Utc::now();
    let old = ApprovedPlan { path: "p".into(), text: "t".into(), approved_at: None };
    assert!(old.expired_at(now));
    let fresh = ApprovedPlan { approved_at: Some(now), ..old };
    assert!(!fresh.expired_at(now) && fresh.expired_at(now + PLAN_TTL));
}
