//! v0.0.3 B1 and B3: a call from an outside agent (the MCP server) always asks, its card cannot be
//! remembered and says where it comes from, and its audit record carries the origin.

mod common;
use common::{ProbeTool, Spans};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{Agent, AgentDeps};
use kagent_core::audit::AuditLog;
use kagent_core::permission::{AllowRule, ApprovalDecision, ApprovalRequest, ApprovalVia, PermissionBroker, PermissionEngine, PermissionMode, ToolOverride};
use kagent_core::protocol::{AgentEvent, ToolCall, ToolCallStatus};
use kagent_core::session::SessionStore;
use kagent_core::tool::{PrivilegeMethod, ToolRegistry};

struct Rig {
    agent: Arc<Agent>,
    events: broadcast::Receiver<AgentEvent>,
    audit_path: std::path::PathBuf,
    spans: Arc<Mutex<Spans>>,
    _dir: tempfile::TempDir,
}

async fn rig(mode: PermissionMode, rules: Vec<AllowRule>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let (etx, erx) = broadcast::channel(256);
    let spans = Arc::new(Mutex::new(Spans::default()));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    tools.register(Arc::new(ProbeTool::mut_("probe_c", &spans)));
    let audit_path = dir.path().join("audit.jsonl");
    let cfg = common::config();
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: Some(Arc::new(AuditLog::open(&audit_path).unwrap())),
        model: Arc::new(common::ScriptedModel::new(vec![])),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(mode, rules)),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(30)),
        store,
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    Rig { agent, events: erx, audit_path, spans, _dir: dir }
}

fn call(name: &str) -> ToolCall {
    ToolCall { id: format!("c_{name}"), name: name.into(), input: json!({}) }
}

/// Run `name` as an outside agent; a card that comes up is answered with `decision`. The card, when there was one.
async fn agent_call(r: &mut Rig, name: &str, decision: ApprovalDecision) -> (ToolCallStatus, Option<ApprovalRequest>) {
    let mut events = r.events.resubscribe();
    let broker = r.agent.broker().clone();
    let answerer = tokio::spawn(async move {
        loop {
            match tokio::time::timeout(Duration::from_millis(800), events.recv()).await {
                Ok(Ok(AgentEvent::ApprovalRequested(request))) => {
                    assert!(broker.resolve(&request.request_id, decision.clone(), ApprovalVia::Click));
                    return Some(request);
                }
                Ok(Ok(_)) => {}
                _ => return None,
            }
        }
    });
    let (_, status) = r.agent.execute_agent_call("mcp", &call(name), &CancellationToken::new(), PrivilegeMethod::None).await;
    (status, answerer.await.unwrap())
}

#[tokio::test]
async fn a_mutating_call_of_an_outside_agent_asks_in_every_mode_with_and_without_a_rule() {
    let rule = AllowRule { tool: "probe_c".into(), prefix: String::new() };
    for mode in [PermissionMode::Ask, PermissionMode::Trusted, PermissionMode::Full] {
        for rules in [vec![], vec![rule.clone()]] {
            let mut r = rig(mode, rules.clone()).await;
            let (status, card) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowOnce).await;
            let card = card.unwrap_or_else(|| panic!("{mode:?} with {} rules: a card", rules.len()));
            assert_eq!(status, ToolCallStatus::Ok);
            assert!(!card.can_remember, "{mode:?}: not rememberable");
            assert!(card.summary.starts_with("Outside agent (MCP): "), "{}", card.summary);
        }
    }
}

#[tokio::test]
async fn nothing_the_owner_remembered_satisfies_an_outside_agent() {
    let mut r = rig(PermissionMode::Ask, vec![AllowRule { tool: "probe_c".into(), prefix: String::new() }]).await;
    // A session allowance of the same session, and an override `allow`.
    r.agent.permissions().add_session_rule("mcp", AllowRule { tool: "probe_c".into(), prefix: String::new() });
    r.agent.permissions().set_overrides([("probe_c".to_string(), ToolOverride::Allow)].into());
    let (_, card) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowOnce).await;
    assert!(card.is_some(), "rules, session allowances and overrides do not satisfy it");
    // "Always allow" answered anyway remembers nothing: the next call asks again.
    let mut r = rig(PermissionMode::Ask, vec![]).await;
    let (_, first) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowAlways).await;
    assert!(first.is_some());
    assert!(r.agent.permissions().rules().is_empty(), "no rule was stored");
    let (_, second) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowSession).await;
    assert!(second.is_some(), "and no session allowance either");
}

#[tokio::test]
async fn read_only_calls_run_deny_and_read_only_mode_still_win() {
    let mut r = rig(PermissionMode::Full, vec![]).await;
    let (status, card) = agent_call(&mut r, "probe_a", ApprovalDecision::AllowOnce).await;
    assert!((status, card.is_none()) == (ToolCallStatus::Ok, true), "a read-only call needs no card");
    // deny overrides
    r.agent.permissions().set_overrides([("probe_c".to_string(), ToolOverride::Deny)].into());
    let (status, card) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowOnce).await;
    assert!(status == ToolCallStatus::Denied && card.is_none());
    // read-only mode
    let mut r = rig(PermissionMode::ReadOnly, vec![]).await;
    let (status, card) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowOnce).await;
    assert!(status == ToolCallStatus::Denied && card.is_none());
    assert!(r.spans.lock().unwrap().entries.is_empty(), "nothing ran");
}

#[tokio::test]
async fn a_call_of_the_chat_is_unchanged_and_the_audit_record_carries_the_origin() {
    let mut r = rig(PermissionMode::Full, vec![]).await;
    // The chat in full control: no card, origin chat.
    let (msg, status) = r.agent.execute_call("s1", "t1", &call("probe_c"), &CancellationToken::new(), PrivilegeMethod::None).await;
    assert_eq!((status, msg.content.as_str()), (ToolCallStatus::Ok, "probe_c done"));
    let (_, card) = agent_call(&mut r, "probe_c", ApprovalDecision::AllowOnce).await;
    assert!(card.is_some());
    let records: Vec<Value> = std::fs::read_to_string(&r.audit_path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let tool_calls: Vec<(&str, &str)> = records.iter().filter(|x| x["kind"] == "toolCall").map(|x| (x["sessionId"].as_str().unwrap(), x["data"]["origin"].as_str().unwrap())).collect();
    assert_eq!(tool_calls, [("s1", "chat"), ("mcp", "agent")]);
    let approvals: Vec<&Value> = records.iter().filter(|x| x["kind"] == "approval").collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["data"]["via"], "click", "the approval record keeps its via");
}
