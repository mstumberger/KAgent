//! Scenarios S24 items 2 and 3 and the scripted-model test of plan-todo.md section 3: a
//! planning turn where the model tries a mutating call, gets the denial, then calls
//! `plan_exit`; the approved plan is in the system prompt of the next request, also after a
//! compaction. The rig is the one of `common/`.

mod common;
use common::{ProbeTool, Spans};

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;

use kagent_core::agent::{Agent, AgentDeps, SendOutcome, TurnOpts, TurnResult};
use kagent_core::model::ModelResponse;
use kagent_core::permission::{AllowRule, ApprovalDecision, ApprovalKind, ApprovalRequest, PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::plan::{PlanAccess, PlanEnter, PlanExit, PLAN_PROMPT};
use kagent_core::protocol::{AgentEvent, ToolCall, TurnStatus, Via};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;

struct Rig {
    agent: Arc<Agent>,
    model: common::ScriptedModel,
    store: SessionStore,
    broker: Arc<PermissionBroker>,
    events: broadcast::Receiver<AgentEvent>,
    session: String,
    /// The probes that ran, in order.
    spans: Arc<Mutex<Spans>>,
    plans: PathBuf,
    _dir: tempfile::TempDir,
}

/// A session with the two probes, the two plan tools and a scripted model, in `mode`.
async fn rig(mode: PermissionMode, rules: Vec<AllowRule>, responses: Vec<ModelResponse>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let session = store.create("t").await.unwrap().id;
    let (etx, erx) = broadcast::channel(256);
    let spans = Arc::new(Mutex::new(Spans::default()));
    let access = PlanAccess::default();
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    tools.register(Arc::new(PlanEnter::new(access.clone())));
    tools.register(Arc::new(PlanExit::new(access.clone(), dir.path().join("plans"))));
    let model = common::ScriptedModel::new(responses);
    let cfg = common::config();
    let permissions = Arc::new(PermissionEngine::new(mode, rules));
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions,
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(30)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    access.attach(&agent);
    Rig { broker: agent.broker().clone(), agent, model, store, events: erx, session, spans, plans: dir.path().join("plans"), _dir: dir }
}

/// A model answer that calls one tool.
fn calls(name: &str, input: Value) -> ModelResponse {
    ModelResponse { tool_calls: vec![ToolCall { id: format!("call_{name}"), name: name.into(), input }], ..Default::default() }
}

fn says(text: &str) -> ModelResponse {
    ModelResponse { text: text.into(), ..Default::default() }
}

/// Run one turn and wait for it; also what it emitted.
async fn turn(r: &Rig, text: &str, opts: TurnOpts) -> (TurnResult, Vec<AgentEvent>) {
    let mut events = r.events.resubscribe();
    let outcome = r.agent.send_with(&r.session, text.into(), Via::Text, opts).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!("nothing runs, so the input starts a turn") };
    let result = handle.wait().await;
    let mut emitted = Vec::new();
    while let Ok(ev) = events.try_recv() {
        emitted.push(ev);
    }
    (result, emitted)
}

fn completed(result: &TurnResult) {
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
}

/// The last stored message of the session is the assistant's `text`. (The rig streams the
/// system prompt as the turn's text, so `TurnResult::text` is not looked at here.)
async fn ends_with(r: &Rig, text: &str) {
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored.last().unwrap().content, text);
}

/// The next approval card, with a deadline.
async fn next_card(events: &mut broadcast::Receiver<AgentEvent>) -> ApprovalRequest {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let AgentEvent::ApprovalRequested(request) = events.recv().await.expect("event channel open") {
                break request;
            }
        }
    })
    .await
    .expect("a card was raised")
}

/// A side task that answers the next `n` cards of the session with `decision`: the cards.
fn answering(r: &Rig, decision: ApprovalDecision, n: usize) -> tokio::task::JoinHandle<Vec<ApprovalRequest>> {
    let mut events = r.events.resubscribe();
    let broker = r.broker.clone();
    tokio::spawn(async move {
        let mut cards = Vec::new();
        for _ in 0..n {
            let request = next_card(&mut events).await;
            assert!(broker.resolve(&request.request_id, decision.clone()));
            cards.push(request);
        }
        cards
    })
}

/// The system prompts of the requests made so far.
fn systems(r: &Rig) -> Vec<String> {
    r.model.systems.lock().unwrap().clone()
}

/// The events of a type, as JSON.
fn of_type(emitted: &[AgentEvent], ty: &str) -> Vec<Value> {
    emitted.iter().filter_map(|e| serde_json::to_value(e).ok().filter(|v| v["type"] == ty)).collect()
}

#[tokio::test]
async fn a_planning_turn_refuses_what_is_not_research_and_the_approved_plan_is_in_the_next_prompt() {
    // Scenario S24 item 2 and the scripted-model test of plan-todo.md section 3. Full control
    // and an allow rule: without plan mode the mutation would run.
    let plan = ["## Free disk space", "1. `disk_info`: find the full disks.", "2. Ask before each cleanup."];
    let r = rig(
        PermissionMode::Full,
        vec![AllowRule { tool: "probe_c".into(), prefix: String::new() }],
        vec![calls("probe_c", json!({})), calls("plan_exit", json!({ "plan": plan })), calls("probe_c", json!({})), says("done")],
    )
    .await;
    let mut events = r.events.resubscribe();
    let broker = r.broker.clone();
    let side = tokio::spawn(async move {
        let request = next_card(&mut events).await;
        assert!(broker.resolve(&request.request_id, ApprovalDecision::AllowOnce));
        request
    });
    let (result, emitted) = turn(&r, "plan how to free disk space", TurnOpts { plan: true, ..Default::default() }).await;
    let card = side.await.unwrap();
    completed(&result);
    ends_with(&r, "done").await;

    // The flag was set before the turn started, and cleared by the approval.
    let types: Vec<String> = emitted.iter().map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_string()).collect();
    assert!(types[0] == "planUpdated" && types[1] == "turnStarted", "planUpdated comes first: {types:?}");
    assert_eq!(of_type(&emitted, "planUpdated"), vec![json!({ "type": "planUpdated", "sessionId": r.session, "active": true }), json!({ "type": "planUpdated", "sessionId": r.session, "active": false })]);

    // The card (tools.md section 19): the plan as lines arrives joined.
    let text = plan.join("\n");
    assert_eq!((card.kind, card.tool.as_str(), card.summary.as_str(), card.can_remember), (ApprovalKind::Plan, "plan", "Approve plan", false));
    assert_eq!((card.input.clone(), card.preview.as_deref()), (json!({ "plan": text }), Some(text.as_str())));

    // The first call was refused with the plan message and never ran; the one after the
    // approval ran, under the normal mode.
    let ran: Vec<&str> = r.spans.lock().unwrap().entries.iter().map(|e| e.0).collect();
    assert_eq!(ran, ["probe_c"], "only the call after the approval");
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored[0].content, "plan how to free disk space", "the user's message is the text");
    let refused = stored.iter().find(|m| m.tool_call_id.as_deref() == Some("call_probe_c") && m.is_error).expect("the refused call has its result");
    assert_eq!(refused.content, format!("not executed: {}", kagent_core::plan::PLAN_DENIED));

    // The prompt: the plan block while planning, then the approved plan.
    let systems = systems(&r);
    assert_eq!(systems.len(), 4, "three requests with calls, then the answer");
    for system in &systems[..2] {
        assert!(system.ends_with(PLAN_PROMPT) && !system.contains("Approved plan"), "planning: no approved plan yet");
    }
    let block = format!("\nApproved plan, follow it:\n{text}\n");
    for system in &systems[2..] {
        assert!(system.ends_with(&block) && !system.contains(PLAN_PROMPT), "the approved plan, and no longer the plan block");
    }

    // The file, the meta.
    assert!(!r.agent.planning(&r.session).await.unwrap());
    let path = r.plans.join(format!("{}-1.md", r.session));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    let meta = r.store.meta(&r.session).await.unwrap();
    assert_eq!(meta.approved_plan, Some(kagent_core::plan::ApprovedPlan { path: path.display().to_string(), text }));
}

#[tokio::test]
async fn plan_enter_by_the_model_plans_the_rest_of_the_turn_and_the_turns_after_it() {
    let r = rig(
        PermissionMode::Ask,
        vec![],
        vec![calls("plan_enter", json!({ "reason": "free disk space" })), calls("probe_c", json!({})), says("planning"), calls("probe_c", json!({})), says("still planning")],
    )
    .await;
    let (result, emitted) = turn(&r, "plan how to free disk space", TurnOpts::default()).await;
    completed(&result);
    ends_with(&r, "planning").await;
    assert_eq!(of_type(&emitted, "planUpdated"), vec![json!({ "type": "planUpdated", "sessionId": r.session, "active": true })], "one event: the turn did not start in plan mode");
    assert!(r.agent.planning(&r.session).await.unwrap(), "and it stays so after the turn");
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert!(stored.iter().any(|m| m.content.starts_with("Plan mode is on") && !m.is_error), "plan_enter says what is refused from now on");
    assert!(stored.iter().any(|m| m.content == format!("not executed: {}", kagent_core::plan::PLAN_DENIED)), "the mutation in the same turn is refused");

    // The next turn plans too: the flag is the session's, not the turn's.
    let (result, _) = turn(&r, "go on", TurnOpts::default()).await;
    completed(&result);
    ends_with(&r, "still planning").await;
    let systems = systems(&r);
    assert_eq!(systems.len(), 5);
    assert!(!systems[0].contains(PLAN_PROMPT), "the first request of the first turn did not plan");
    assert!(systems[1..].iter().all(|s| s.ends_with(PLAN_PROMPT)), "every request since does");
    assert!(r.spans.lock().unwrap().entries.iter().all(|e| e.0 != "probe_c"), "the mutation never ran");

    // Cancel: said once; asking again changes nothing and says nothing.
    let mut events = r.events.resubscribe();
    assert!(r.agent.set_planning(&r.session, false).await.unwrap());
    assert!(!r.agent.set_planning(&r.session, false).await.unwrap());
    let mut said = Vec::new();
    while let Ok(ev) = events.try_recv() {
        said.push(serde_json::to_value(ev).unwrap());
    }
    assert_eq!(said, vec![json!({ "type": "planUpdated", "sessionId": r.session, "active": false })]);
}

#[tokio::test]
async fn a_rejected_plan_keeps_the_session_planning_and_the_result_carries_the_reason() {
    // Scenario S24 item 3: two plans in one turn, each rejected as the owner says.
    let r = rig(
        PermissionMode::Ask,
        vec![],
        vec![calls("plan_exit", json!({ "plan": "1. Look." })), calls("plan_exit", json!({ "plan": ["1. Look.", "2. Ask."] })), says("planning")],
    )
    .await;
    let cards = answering(&r, ApprovalDecision::Deny { reason: Some("step 1 is too risky".into()) }, 2);
    let (result, emitted) = turn(&r, "plan it", TurnOpts { plan: true, ..Default::default() }).await;
    completed(&result);
    ends_with(&r, "planning").await;
    let cards = cards.await.unwrap();
    assert_eq!(cards.len(), 2, "one card per plan");
    assert!(cards.iter().all(|c| c.kind == ApprovalKind::Plan));
    let stored = r.store.load_messages(&r.session).await.unwrap();
    let answers: Vec<(String, bool)> = stored.iter().filter(|m| m.tool_name.as_deref() == Some("plan_exit")).map(|m| (m.content.clone(), m.is_error)).collect();
    let expected = "Plan rejected: step 1 is too risky. Revise the plan and call plan_exit again.";
    assert_eq!(answers, [(expected.to_string(), false), (expected.to_string(), false)], "a rejected plan is an answer, not a failure");
    assert!(r.agent.planning(&r.session).await.unwrap(), "still planning");
    assert!(!r.plans.exists(), "nothing was saved");
    assert!(r.store.meta(&r.session).await.unwrap().approved_plan.is_none());
    assert_eq!(of_type(&emitted, "planUpdated"), vec![json!({ "type": "planUpdated", "sessionId": r.session, "active": true })], "the flag was set once and did not change again");
    let systems = systems(&r);
    assert_eq!(systems.len(), 3, "two plans, then the answer");
    assert!(systems.iter().all(|s| s.ends_with(PLAN_PROMPT) && !s.contains("Approved plan")), "every request plans");
}

#[tokio::test]
async fn a_plan_denied_without_a_reason_says_so() {
    // A denial with no reason (another client's Deny button) still names one.
    let r = rig(PermissionMode::Ask, vec![], vec![calls("plan_exit", json!({ "plan": "1. Look." })), says("planning")]).await;
    let mut events = r.events.resubscribe();
    let broker = r.broker.clone();
    let side = tokio::spawn(async move {
        let request = next_card(&mut events).await;
        assert!(broker.resolve(&request.request_id, ApprovalDecision::Deny { reason: None }));
    });
    let (result, _) = turn(&r, "plan it", TurnOpts { plan: true, ..Default::default() }).await;
    side.await.unwrap();
    completed(&result);
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert!(stored.iter().any(|m| m.content == "Plan rejected: the user denied this plan. Revise the plan and call plan_exit again."), "the result carries a reason");
    assert!(r.agent.planning(&r.session).await.unwrap(), "still planning");
}

#[tokio::test]
async fn cancelling_the_plan_answers_the_card_that_waits_and_the_turn_goes_on() {
    // "Cancel plan" while the card is shown: the rest of the turn follows the normal mode.
    let r = rig(PermissionMode::Ask, vec![], vec![calls("plan_exit", json!({ "plan": "1. step" })), says("went on")]).await;
    let mut events = r.events.resubscribe();
    let (agent, session) = (r.agent.clone(), r.session.clone());
    let side = tokio::spawn(async move {
        let request = next_card(&mut events).await;
        assert!(agent.set_planning(&session, false).await.unwrap(), "the cancel changed the flag");
        request
    });
    let (result, _) = turn(&r, "plan something", TurnOpts { plan: true, ..Default::default() }).await;
    let card = side.await.unwrap();
    completed(&result);
    ends_with(&r, "went on").await;
    assert_eq!(card.kind, ApprovalKind::Plan);
    let stored = r.store.load_messages(&r.session).await.unwrap();
    let answer = stored.iter().find(|m| m.tool_name.as_deref() == Some("plan_exit")).expect("the result of plan_exit");
    assert_eq!(answer.content, format!("Plan not approved: {}. Plan mode is off: go on as the user asks, under the normal permissions.", kagent_core::plan::PLAN_CANCELLED));
    assert!(r.agent.broker().pending().is_empty(), "the card is gone");
    assert!(!r.agent.planning(&r.session).await.unwrap());
    let systems = systems(&r);
    assert!(systems[0].ends_with(PLAN_PROMPT) && !systems[1].contains(PLAN_PROMPT) && !systems[1].contains("Approved plan"), "the request after the cancel plans no more");
}

#[tokio::test]
async fn the_approved_plan_is_in_the_prompt_after_a_compaction() {
    let r = rig(PermissionMode::Ask, vec![], vec![calls("plan_exit", json!({ "plan": "1. Look." })), says("done"), says("answer 2"), says("SUMMARY"), says("after")]).await;
    let mut events = r.events.resubscribe();
    let broker = r.broker.clone();
    let side = tokio::spawn(async move {
        let request = next_card(&mut events).await;
        assert!(broker.resolve(&request.request_id, ApprovalDecision::AllowOnce));
    });
    let _ = turn(&r, "plan it", TurnOpts { plan: true, ..Default::default() }).await;
    side.await.unwrap();
    let (result, _) = turn(&r, "a question", TurnOpts::default()).await;
    completed(&result);

    // Compacted by hand: the summary request comes, then a request that starts with the
    // summary and still carries the plan.
    r.agent.compact(&r.session).await.unwrap();
    let (result, _) = turn(&r, "another question", TurnOpts::default()).await;
    completed(&result);
    ends_with(&r, "after").await;
    let systems = systems(&r);
    assert!(systems.last().unwrap().ends_with("\nApproved plan, follow it:\n1. Look.\n"), "the plan is still there: {:?}", systems.last().unwrap());
    let requests = r.model.requests.lock().unwrap().clone();
    assert!(requests.last().unwrap()[0].content.starts_with("Conversation so far (summary of the first"), "the history was compacted");
}
