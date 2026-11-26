//! Token guard (token-guard.md): the daemon's redactor takes the API token out of every
//! tool output, before it is stored or sent to the model.

mod common;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{Agent, AgentDeps};
use kagent_core::journal::ChangeJournal;
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{ToolCall, ToolCallStatus};
use kagent_core::session::SessionStore;
use kagent_core::tool::{PrivilegeMethod, Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry, ToolServices};
use kagent_core::tools::privop::PrivClient;

const SECRET: &str = "abc123secret";
const PLACEHOLDER: &str = "<redacted: KAgent API token>";

struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        unreachable!("execute_call makes no model request")
    }
}

/// A tool whose output holds the token, as a shell, file or journal read would.
struct Leaky {
    contract: ToolContract,
}

#[async_trait]
impl Tool for Leaky {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::ok(format!("open http://127.0.0.1:7878/#token={SECRET}\nsecond line {SECRET}")))
    }
}

async fn agent(journal_dir: &std::path::Path, with_redactor: bool) -> (Arc<Agent>, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let (etx, _erx) = broadcast::channel(64);
    let services = Arc::new(ToolServices::new(
        Arc::new(ChangeJournal::new(journal_dir).unwrap()),
        Arc::new(PrivClient::with_path("/nonexistent/kagent-priv")),
    ));
    if with_redactor {
        *services.redact.write().unwrap() = Some(Arc::new(|text: &str| text.replace(SECRET, PLACEHOLDER)));
    }
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Leaky { contract: ToolContract::read_only("leaky", "probe", json!({"type": "object", "properties": {}})) }));
    let cfg = common::config();
    let agent = Agent::new(AgentDeps {
        services: with_redactor.then_some(services),
        hooks: None,
        audit: None,
        model: Arc::new(NoModel),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Trusted, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(10)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    std::mem::forget(dir);
    (agent, meta.id)
}

#[tokio::test]
async fn every_tool_output_leaves_without_the_api_token() {
    let jdir = tempfile::tempdir().unwrap();
    let (agent, session) = agent(jdir.path(), true).await;
    let call = ToolCall { id: "c1".into(), name: "leaky".into(), input: json!({}) };
    let (msg, status) = agent.execute_call(&session, "t1", &call, &CancellationToken::new(), PrivilegeMethod::None).await;
    assert!(matches!(status, ToolCallStatus::Ok));
    assert!(!msg.content.contains(SECRET), "{}", msg.content);
    assert_eq!(msg.content.matches(PLACEHOLDER).count(), 2);
    // Stored the same way: the session never holds it either.
    let stored = agent.messages(&session).await.unwrap();
    assert!(stored.iter().all(|m| !m.content.contains(SECRET)));
}

#[tokio::test]
async fn without_a_redactor_the_output_passes_unchanged() {
    let jdir = tempfile::tempdir().unwrap();
    let (agent, session) = agent(jdir.path(), false).await;
    let call = ToolCall { id: "c1".into(), name: "leaky".into(), input: json!({}) };
    let (msg, _) = agent.execute_call(&session, "t1", &call, &CancellationToken::new(), PrivilegeMethod::None).await;
    assert!(msg.content.contains(SECRET), "{}", msg.content);
}
