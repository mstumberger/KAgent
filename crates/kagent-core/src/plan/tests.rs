//! The tests of plan-todo.md section 3 (the plan items) and scenarios.md S24 items 1 to 3.
//! No model is asked: the tools run through `execute_calls`, as they do in a turn.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::agent::{AgentConfig, AgentDeps};
use crate::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use crate::permission::{AllowRule, ApprovalDecision, ApprovalRequest, Decision, PermissionBroker, PermissionEngine, PermissionMode, ToolOverride};
use crate::protocol::{AgentEvent, ToolCall, ToolCallStatus};
use crate::session::SessionStore;
use crate::todo::TodoAccess;
use crate::tool::{PrivilegeMethod, Tool};
use crate::tools::builtin_registry;

/// Never asked: no test here runs a turn.
struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        anyhow::bail!("no model in this test")
    }
}

struct Rig {
    agent: Arc<Agent>,
    events: broadcast::Sender<AgentEvent>,
    session: String,
    plans: PathBuf,
    dir: tempfile::TempDir,
}

/// An agent over a temporary home, with every builtin tool and the tools of the `agent`
/// group (registered as the daemon does), in `mode`.
async fn rig(mode: PermissionMode, rules: Vec<AllowRule>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let session = store.create("t").await.unwrap().id;
    let (events, _) = broadcast::channel(64);
    let plan = PlanAccess::default();
    let todo = TodoAccess::default();
    let mut registry = builtin_registry();
    registry.register(Arc::new(crate::todo::TodoWrite::new(todo.clone())));
    registry.register(Arc::new(crate::todo::TodoRead::new(todo.clone())));
    registry.register(Arc::new(PlanEnter::new(plan.clone())));
    registry.register(Arc::new(PlanExit::new(plan.clone(), dir.path().join("data/plans"))));
    let cfg = AgentConfig {
        model_ref: String::new(),
        system_prompt: String::new(),
        max_iterations: 4,
        options: Default::default(),
        privilege: PrivilegeMethod::None,
        context_tokens: 0,
        fallback_model_ref: String::new(),
        fallback_context_tokens: 0,
        fallback_max_output: None,
    };
    let agent = Agent::new(AgentDeps {
        services: None,
        audit: None,
        model: Arc::new(NoModel),
        tools: Arc::new(registry),
        permissions: Arc::new(PermissionEngine::new(mode, rules)),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
        store,
        events: events.clone(),
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    todo.attach(&agent);
    plan.attach(&agent);
    Rig { agent, events, session, plans: dir.path().join("data/plans"), dir }
}

/// One call as a turn makes it (`execute_call`): the text of its result, whether that is
/// an error, and its status.
async fn call(agent: &Arc<Agent>, session: &str, id: &str, name: &str, input: Value) -> (String, bool, ToolCallStatus) {
    let call = ToolCall { id: id.into(), name: name.into(), input };
    let (msg, status) = agent.execute_call(session, "turn", &call, &CancellationToken::new(), PrivilegeMethod::None).await;
    (msg.content, msg.is_error, status)
}

/// The events until the channel is empty, as JSON.
async fn drained(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(serde_json::to_value(&ev).unwrap());
    }
    out
}

/// The `planUpdated` events in `events`: whether each says the session plans.
fn planning_events(events: &[Value]) -> Vec<bool> {
    events.iter().filter(|e| e["type"] == "planUpdated").map(|e| e["active"].as_bool().unwrap()).collect()
}

/// The next approval card, with a deadline.
async fn next_card(rx: &mut broadcast::Receiver<AgentEvent>) -> ApprovalRequest {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let AgentEvent::ApprovalRequested(request) = rx.recv().await.expect("event channel open") {
                break request;
            }
        }
    })
    .await
    .expect("a card was raised")
}

/// Run one `plan_exit` while a side task answers its card with `decision`: the card and the
/// result of the call.
async fn answered(r: &Rig, input: Value, decision: ApprovalDecision) -> (ApprovalRequest, (String, bool, ToolCallStatus)) {
    let mut rx = r.events.subscribe();
    let broker = r.agent.broker().clone();
    let side = tokio::spawn(async move {
        let request = next_card(&mut rx).await;
        assert!(broker.resolve(&request.request_id, decision));
        request
    });
    let out = call(&r.agent, &r.session, "c1", "plan_exit", input).await;
    (side.await.unwrap(), out)
}

/// Scenario S24 item 1 and the evaluate table of plan-todo.md section 3.
#[tokio::test]
async fn in_plan_mode_only_research_runs_whatever_the_mode_the_rules_and_the_overrides_say() {
    let r = rig(PermissionMode::Full, vec![AllowRule { tool: "write_file".into(), prefix: String::new() }]).await;
    let engine = r.agent.permissions().clone();
    let assess = |name: &str, input: Value| r.agent.tools().get(name).expect("a builtin tool").assess(&input);
    let write = assess("write_file", json!({ "path": "~/plan-test.txt", "content": "x" }));
    let read = assess("read_file", json!({ "path": "~/plan-test.txt" }));
    let todo = assess("todo_write", json!({ "items": [] }));
    let mut overrides = std::collections::HashMap::new();
    overrides.insert("write_file".to_string(), ToolOverride::Allow);
    engine.set_overrides(overrides);
    engine.add_session_rule(&r.session, AllowRule { tool: "write_file".into(), prefix: String::new() });
    // Not planning: full control, the rule and the override let the write through.
    assert!(matches!(engine.evaluate(&r.session, "write_file", &write), Decision::Allow { .. }));

    assert!(r.agent.set_planning(&r.session, true).await.unwrap(), "the flag changed");
    let denied = Decision::Deny { reason: PLAN_DENIED.into() };
    let research = Decision::Allow { reason: "read-only call".into() };
    for mode in [PermissionMode::Ask, PermissionMode::Trusted, PermissionMode::Full] {
        engine.set_mode(mode);
        assert_eq!(engine.evaluate(&r.session, "write_file", &write), denied, "{mode:?}: nothing allows the write");
        assert_eq!(engine.evaluate(&r.session, "read_file", &read), research, "{mode:?}: research runs");
        assert_eq!(engine.evaluate(&r.session, "todo_write", &todo), research, "{mode:?}: the todo list is the agent's own");
    }
    assert!(matches!(engine.evaluate("another-session", "write_file", &write), Decision::Allow { .. }), "only the planning session is restricted");
    engine.set_mode(PermissionMode::ReadOnly);
    assert!(matches!(engine.evaluate(&r.session, "write_file", &write), Decision::Deny { reason } if reason != PLAN_DENIED), "read-only mode says so first");
    engine.set_mode(PermissionMode::Full);

    // As a turn runs the calls: the write is refused with the plan message and never
    // happens; the read and the todo list run.
    let notes = r.dir.path().join("notes.txt");
    std::fs::write(&notes, "hello").unwrap();
    let never = r.dir.path().join("never-written.txt");
    let calls = vec![
        ToolCall { id: "w".into(), name: "write_file".into(), input: json!({ "path": never, "content": "x" }) },
        ToolCall { id: "r".into(), name: "read_file".into(), input: json!({ "path": notes }) },
        ToolCall { id: "t".into(), name: "todo_write".into(), input: json!({ "items": [{ "content": "Find the full disks" }] }) },
    ];
    let results = r.agent.execute_calls(&r.session, "turn", &calls, &CancellationToken::new(), PrivilegeMethod::None).await;
    assert_eq!((results[0].content.as_str(), results[0].is_error), (format!("not executed: {PLAN_DENIED}").as_str(), true));
    assert!(!never.exists(), "the write did not happen");
    assert!(!results[1].is_error && results[1].content.contains("hello"), "{}", results[1].content);
    assert!(!results[2].is_error && results[2].content.contains("Find the full disks"), "{}", results[2].content);

    assert!(r.agent.set_planning(&r.session, false).await.unwrap());
    assert!(matches!(engine.evaluate(&r.session, "write_file", &write), Decision::Allow { .. }), "over: the write is allowed again");
}

/// `plan_exit` outside plan mode, or with no plan it can show (tools.md section 19).
#[tokio::test]
async fn plan_exit_outside_plan_mode_or_with_no_plan_to_show_is_an_error_and_raises_no_card() {
    let r = rig(PermissionMode::Ask, vec![]).await;
    let mut rx = r.events.subscribe();
    let (text, is_error, _) = call(&r.agent, &r.session, "c1", "plan_exit", json!({ "plan": "1. do it" })).await;
    assert!(is_error && text.contains("not in plan mode"), "{text}");
    assert!(call(&r.agent, "no-such-session", "c2", "plan_enter", json!({})).await.0.contains("there is no session 'no-such-session'"));
    assert!(r.agent.broker().pending().is_empty(), "no card");

    r.agent.set_planning(&r.session, true).await.unwrap();
    let _ = drained(&mut rx).await;
    for (what, input) in [
        ("no plan", json!({})),
        ("a blank plan", json!({ "plan": " \n " })),
        ("a plan over 12 KB", json!({ "plan": "x".repeat(MAX_PLAN_BYTES + 1) })),
        ("lines that are not text", json!({ "plan": [1, 2] })),
        ("a number", json!({ "plan": 7 })),
    ] {
        let (text, is_error, status) = call(&r.agent, &r.session, "c3", "plan_exit", input).await;
        assert_eq!((is_error, status), (true, ToolCallStatus::Error), "{what}: {text}");
        assert!(r.agent.broker().pending().is_empty(), "{what}: no card");
        assert!(planning_events(&drained(&mut rx).await).is_empty(), "{what}: the flag did not change");
        assert!(r.agent.planning(&r.session).await.unwrap(), "{what}: still planning");
    }
    // 12 KB exactly is taken.
    let (_, (text, is_error, _)) = answered(&r, json!({ "plan": "x".repeat(MAX_PLAN_BYTES) }), ApprovalDecision::AllowOnce).await;
    assert!(!is_error, "{text}");
}

/// Scenario S24 item 2.
#[tokio::test]
async fn an_approved_plan_ends_plan_mode_and_is_saved_and_kept_in_the_meta() {
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    let mut rx = r.events.subscribe();
    let lines = ["## Plan", "1. Look.", "2. Ask."];
    let (card, (text, is_error, status)) = answered(&r, json!({ "plan": lines }), ApprovalDecision::AllowOnce).await;
    assert_eq!((status, is_error), (ToolCallStatus::Ok, false));
    assert!(text.starts_with(PLAN_APPROVED) && text.contains(&format!("{}-1.md", r.session)), "the result names the file: {text}");

    // The card (tools.md section 19).
    let plan = lines.join("\n");
    assert_eq!((card.kind, card.tool.as_str(), card.summary.as_str(), card.can_remember), (ApprovalKind::Plan, "plan", "Approve plan", false));
    assert_eq!((card.input.clone(), card.preview.as_deref()), (json!({ "plan": plan }), Some(plan.as_str())));

    // The flag, the file and the meta.
    assert!(!r.agent.planning(&r.session).await.unwrap());
    assert!(!r.agent.permissions().is_planning(&r.session), "the engine follows");
    let path = r.plans.join(format!("{}-1.md", r.session));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), plan);
    assert_eq!(r.agent.store().meta(&r.session).await.unwrap().approved_plan, Some(ApprovedPlan { path: path.display().to_string(), text: plan.clone() }));
    assert_eq!(planning_events(&drained(&mut rx).await), [false], "one event for the change");

    // A second plan, one text with space around it: the next number, and it replaces the
    // first in the meta.
    r.agent.set_planning(&r.session, true).await.unwrap();
    let (_, (text, is_error, _)) = answered(&r, json!({ "plan": "  One line. \n" }), ApprovalDecision::AllowOnce).await;
    assert!(!is_error && text.contains(&format!("{}-2.md", r.session)), "{text}");
    assert_eq!(r.agent.store().meta(&r.session).await.unwrap().approved_plan.map(|p| p.text), Some("One line.".into()), "trimmed");
    assert_eq!(std::fs::read_dir(&r.plans).unwrap().count(), 2);
}

/// Scenario S24 item 3.
#[tokio::test]
async fn a_rejected_plan_keeps_the_session_planning_and_the_result_carries_the_reason() {
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    let mut rx = r.events.subscribe();
    for (decision, expected) in [
        (ApprovalDecision::Deny { reason: Some("step 1 is too risky".into()) }, "Plan rejected: step 1 is too risky. Revise the plan and call plan_exit again."),
        (ApprovalDecision::Deny { reason: None }, "Plan rejected: the user denied this plan. Revise the plan and call plan_exit again."),
    ] {
        let (card, (text, is_error, status)) = answered(&r, json!({ "plan": "1. step" }), decision).await;
        assert_eq!((status, is_error), (ToolCallStatus::Ok, false), "a rejected plan is an answer, not a failure");
        assert_eq!(text, expected);
        assert_eq!(card.kind, ApprovalKind::Plan);
        assert!(r.agent.planning(&r.session).await.unwrap(), "still planning");
        assert!(!r.plans.exists(), "nothing was saved");
        assert!(planning_events(&drained(&mut rx).await).is_empty(), "the flag did not change");
    }
    assert!(r.agent.store().meta(&r.session).await.unwrap().approved_plan.is_none());
}

#[tokio::test]
async fn cancelling_the_plan_answers_the_card_that_waits() {
    // "Cancel plan" while the card is shown: the rest of the turn follows the normal mode.
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    let mut rx = r.events.subscribe();
    let (agent, session) = (r.agent.clone(), r.session.clone());
    let side = tokio::spawn(async move {
        let request = next_card(&mut rx).await;
        assert!(agent.set_planning(&session, false).await.unwrap(), "Cancel plan changed the flag");
        request
    });
    let (text, is_error, status) = call(&r.agent, &r.session, "c1", "plan_exit", json!({ "plan": "1. step" })).await;
    let card = side.await.unwrap();
    assert_eq!((status, is_error), (ToolCallStatus::Ok, false));
    assert_eq!(text, format!("Plan not approved: {PLAN_CANCELLED}. Plan mode is off: go on as the user asks, under the normal permissions."));
    assert_eq!(card.kind, ApprovalKind::Plan);
    assert!(r.agent.broker().pending().is_empty(), "the card is gone");
    assert!(!r.agent.planning(&r.session).await.unwrap());
    assert!(r.agent.store().meta(&r.session).await.unwrap().approved_plan.is_none(), "a cancelled plan is not an approved one");
}

#[tokio::test]
async fn plan_enter_sets_the_flag_once_and_says_so_with_one_event() {
    let r = rig(PermissionMode::Ask, vec![]).await;
    let mut rx = r.events.subscribe();
    let (text, is_error, _) = call(&r.agent, &r.session, "c1", "plan_enter", json!({ "reason": "free disk space" })).await;
    assert!(!is_error && text.contains("Plan mode is on"), "{text}");
    assert!(r.agent.planning(&r.session).await.unwrap() && r.agent.permissions().is_planning(&r.session));
    let (again, is_error, _) = call(&r.agent, &r.session, "c2", "plan_enter", json!({})).await;
    assert!(is_error && again.contains("already in plan mode"), "{again}");
    assert!(!r.agent.set_planning(&r.session, true).await.unwrap(), "asking for what is changes nothing");
    assert_eq!(planning_events(&drained(&mut rx).await), [true], "one event");
    assert!(r.agent.store().meta(&r.session).await.unwrap().planning, "and it is in the file");
}

#[tokio::test]
async fn the_engine_follows_the_meta_of_a_loaded_session_and_forgets_it_with_the_session() {
    // A session a daemon run before left in plan mode: still planning after the restart.
    let r = rig(PermissionMode::Full, vec![]).await;
    let mut old = r.agent.store().create("old").await.unwrap();
    old.planning = true;
    r.agent.store().save_meta(&old).await.unwrap();
    let engine = r.agent.permissions();
    let write = r.agent.tools().get("write_file").unwrap().assess(&json!({ "path": "~/x", "content": "x" }));
    assert!(!engine.is_planning(&old.id), "not loaded yet");
    assert!(r.agent.planning(&old.id).await.unwrap(), "loading the session reads the flag");
    assert_eq!(engine.evaluate(&old.id, "write_file", &write), Decision::Deny { reason: PLAN_DENIED.into() });
    r.agent.forget(&old.id);
    assert!(!engine.is_planning(&old.id), "a deleted session leaves nothing in the engine");
}

#[tokio::test]
async fn the_event_the_card_and_a_card_from_before_have_their_json_shapes() {
    let event = AgentEvent::PlanUpdated { session_id: "s".into(), active: true };
    assert_eq!(serde_json::to_value(&event).unwrap(), json!({ "type": "planUpdated", "sessionId": "s", "active": true }));
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    let (card, _) = answered(&r, json!({ "plan": "1. step" }), ApprovalDecision::AllowOnce).await;
    let mut json_card = serde_json::to_value(&card).unwrap();
    assert_eq!(json_card["kind"], "plan");
    json_card.as_object_mut().unwrap().remove("kind");
    assert_eq!(serde_json::from_value::<ApprovalRequest>(json_card).unwrap().kind, ApprovalKind::Tool, "a card from before S24 is a tool call");
}

#[test]
fn the_plan_step_refuses_only_what_is_neither_read_only_nor_allowed() {
    let research = CallAssessment::from_contract(&ToolContract::read_only("x", "", json!({})), String::new(), String::new());
    let mut mutation = research.clone();
    mutation.read_only = false;
    mutation.needs_approval = true;
    assert_eq!(plan_denial(false, "shell", &mutation), None, "not planning");
    assert_eq!(plan_denial(true, "read_file", &research), None, "research");
    for tool in ["shell", "package", "write_file", "skill_save", "agent_run"] {
        assert_eq!(plan_denial(true, tool, &mutation), Some(PLAN_DENIED), "{tool}");
    }
    for tool in PLAN_ALLOWED {
        assert_eq!(plan_denial(true, tool, &mutation), None, "{tool} is allowed even when it is not read-only");
    }
    assert_eq!(approved_prompt("1. a\n2. b"), "\nApproved plan, follow it:\n1. a\n2. b\n");
    assert!(PLAN_PROMPT.starts_with('\n') && PLAN_PROMPT.ends_with('\n') && PLAN_PROMPT.contains("plan_exit"), "a block for the system prompt");
    assert_eq!(serde_json::to_value(ApprovedPlan { path: "/data/plans/s-1.md".into(), text: "1.".into() }).unwrap(), json!({ "path": "/data/plans/s-1.md", "text": "1." }));
}

#[test]
fn the_plan_argument_takes_one_text_or_its_lines_and_the_tools_are_read_only() {
    // The S20 trap: Ollama drops a call whose long Markdown text in a plain `string` it
    // cannot parse, so the schema takes an array of lines too.
    let exit = PlanExit::new(PlanAccess::default(), "/unused");
    let schema = &exit.contract().input_schema["properties"]["plan"];
    assert!(schema.get("anyOf").is_some() && schema.get("type").is_none(), "the schema takes either form: {schema}");
    assert_eq!(exit.contract().name, "plan_exit");
    assert!(exit.contract().read_only && !exit.contract().needs_approval && !exit.contract().always_ask, "no approval of its own (tools.md section 19)");
    assert!(exit.contract().default_timeout_ms >= 3_600_000, "the card can wait for the owner");
    assert!(exit.assess(&json!({ "plan": "x" })).summary.contains("plan"));
    let enter = PlanEnter::new(PlanAccess::default());
    assert!(enter.contract().read_only && enter.contract().name == "plan_enter");
    // Both belong to the `agent` group (tools.md section 19): they are offered and allowed with it.
    assert_eq!((crate::tools::group_of("plan_enter"), crate::tools::group_of("plan_exit")), ("agent", "agent"));
}
