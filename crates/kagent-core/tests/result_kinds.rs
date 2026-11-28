//! agent-loop.md B.1: the result kinds besides ok and error reach the loop, the event and the model as their own status.

mod common;
use common::*;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{Agent, AgentDeps};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{AgentEvent, ToolCallStatus};
use kagent_core::session::SessionStore;
use kagent_core::tool::{Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry};

/// A tool whose result is the one `name` says.
struct Kind(ToolContract);

#[async_trait]
impl Tool for Kind {
    fn contract(&self) -> &ToolContract {
        &self.0
    }
    async fn call(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        Ok(match self.0.name.as_str() {
            "k_not_done" => ToolOutput::not_done("nothing was queued", "ask the owner"),
            "k_shown" => ToolOutput::shown("the settings page"),
            "k_partial" => ToolOutput::partial("3 of 5", "2 are read-only", "ask the owner"),
            "k_error" => ToolOutput::error("failed"),
            _ => ToolOutput::ok("fine"),
        })
    }
}

#[tokio::test]
async fn each_result_kind_is_its_own_status_and_only_not_done_reads_as_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let (etx, mut erx) = broadcast::channel(64);
    let mut tools = ToolRegistry::new();
    for name in ["k_ok", "k_error", "k_not_done", "k_shown", "k_partial"] {
        tools.register(Arc::new(Kind(ToolContract::read_only(name, "k", json!({"type": "object", "properties": {}})))));
    }
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(ScriptedModel::new(vec![])),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(5)),
        store,
        events: etx,
        config: Arc::new(|_| config()),
        on_rules_changed: None,
    });
    let cancel = CancellationToken::new();
    let expected = [
        ("k_ok", ToolCallStatus::Ok, false, "fine", "ok"),
        ("k_error", ToolCallStatus::Error, true, "failed", "error"),
        ("k_not_done", ToolCallStatus::NotDone, true, "not done: nothing was queued. Next: ask the owner.", "notDone"),
        ("k_shown", ToolCallStatus::Shown, false, "shown to the owner: the settings page. Nothing was changed.", "shown"),
        ("k_partial", ToolCallStatus::Partial, false, "partly done: 3 of 5. Missing: 2 are read-only. Next: ask the owner.", "partial"),
    ];
    for (name, status, is_error, text, wire_word) in expected {
        let (msg, got) = agent.execute_call(&meta.id, "t", &call(name, name), &cancel, kagent_core::tool::PrivilegeMethod::None).await;
        assert_eq!((got, msg.is_error, msg.content.as_str()), (status, is_error, text), "{name}");
        // The event carries the status in the same words the web card reads.
        let ev = wait_event(&mut erx, 5, |e| matches!(e, AgentEvent::ToolCallFinished { name: n, .. } if n == name)).await;
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["status"], wire_word, "{json}");
    }
    let wire: Vec<String> = [ToolCallStatus::NotDone, ToolCallStatus::Shown, ToolCallStatus::Partial].iter().map(|s| serde_json::to_value(s).unwrap().as_str().unwrap().to_string()).collect();
    assert_eq!(wire, ["notDone", "shown", "partial"]);
}
