//! `POST /api/sessions/:id/compact` (api.md section 6, compaction.md section 4): a
//! compaction by hand. The work is a function over the agent, so it is tested without a
//! server (a second daemon cannot run next to the owner's).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use kagent_core::agent::Agent;
use kagent_core::compact::CompactError;

use crate::server::{ApiResult, HttpError};
use crate::AppState;

/// Everything before the last user message of the session, summarized: `{summary,
/// keptMessages}`. 400 for a session too short to compact, 404 for one that does not
/// exist, 502 when the summary failed.
pub async fn compact(agent: &Arc<Agent>, id: &str) -> Result<Value, HttpError> {
    let view = agent.compact(id).await.map_err(|e| match e {
        CompactError::TooShort(message) => HttpError(StatusCode::BAD_REQUEST, "invalid_input", message),
        CompactError::Failed(message) => HttpError(StatusCode::BAD_GATEWAY, "summary_failed", message),
        CompactError::Other(e) => match e.downcast_ref::<std::io::Error>() {
            Some(io) if io.kind() == std::io::ErrorKind::NotFound => HttpError(StatusCode::NOT_FOUND, "not_found", format!("session {id} not found")),
            _ => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}")),
        },
    })?;
    Ok(json!({"summary": view.summary, "keptMessages": view.kept_messages}))
}

pub async fn compact_session(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let body = compact(&st.agent, &id).await?;
    Ok((StatusCode::OK, Json(body)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::{broadcast, mpsc};
    use tokio_util::sync::CancellationToken;

    use kagent_core::agent::{AgentConfig, AgentDeps};
    use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
    use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
    use kagent_core::protocol::Message;
    use kagent_core::session::SessionStore;
    use kagent_core::tool::ToolRegistry;

    /// Answers `Ok(summary)` to a summary request; `Err` when `failing`.
    struct SummaryModel {
        failing: bool,
    }

    #[async_trait]
    impl ModelClient for SummaryModel {
        async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
            if self.failing {
                anyhow::bail!("model unreachable");
            }
            Ok(ModelResponse { text: "  The user asked about swap, twice. ".into(), ..Default::default() })
        }
    }

    /// An agent over a temporary session store, with `messages` stored in its session.
    async fn agent_with(messages: Vec<Message>, failing: bool) -> (Arc<Agent>, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let meta = store.create("t").await.unwrap();
        store.append(&meta.id, &messages).await.unwrap();
        let (etx, _) = broadcast::channel(16);
        let cfg = AgentConfig { model_ref: "p::m".into(), system_prompt: String::new(), max_iterations: 4, options: Default::default(), privilege: Default::default(), context_tokens: 0, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
        let agent = Agent::new(AgentDeps {
            services: None,
            hooks: None,
            audit: None,
            model: Arc::new(SummaryModel { failing }),
            tools: Arc::new(ToolRegistry::new()),
            permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
            broker: PermissionBroker::new(etx.clone(), Duration::from_secs(5)),
            store,
            events: etx,
            config: Arc::new(move |_| cfg.clone()),
            on_rules_changed: None,
        });
        (agent, meta.id, dir) // the directory lives as long as the test keeps it
    }

    fn said(texts: &[&str]) -> Vec<Message> {
        texts.iter().map(|t| Message::user(t.to_string())).collect()
    }

    /// The route's answer, or its status, code and message (`HttpError` has no `Debug`).
    async fn answer(agent: &Arc<Agent>, id: &str) -> Result<Value, (StatusCode, &'static str, String)> {
        compact(agent, id).await.map_err(|HttpError(status, code, message)| (status, code, message))
    }

    #[tokio::test]
    async fn manual_compaction_answers_the_summary_and_refuses_a_short_session() {
        // Three finished turns: everything before the last user message is two turns.
        let mut turns = Vec::new();
        for t in ["one", "two", "three"] {
            turns.push(Message::user(t));
            turns.push(Message::assistant(format!("{t}, done"), vec![]));
        }
        let (agent, id, _dir) = agent_with(turns, false).await;
        let body = answer(&agent, &id).await.unwrap();
        assert_eq!(body, json!({"summary": "The user asked about swap, twice.", "keptMessages": 2}), "everything before the last user message");
        let stored = agent.store().load_messages(&id).await.unwrap();
        assert_eq!(stored.len(), 6, "the session file is not changed by the compaction");

        for stored in [said(&[]), said(&["one"]), said(&["one", "two"]), said(&["one", "two", "three"])] {
            let (agent, id, _dir) = agent_with(stored, false).await;
            let err = answer(&agent, &id).await.unwrap_err();
            assert_eq!((err.0, err.1), (StatusCode::BAD_REQUEST, "invalid_input"), "fewer than 4 stored messages: {}", err.2);
        }
    }

    #[tokio::test]
    async fn a_failed_summary_is_502_and_an_unknown_session_404() {
        let (agent, id, _dir) = agent_with(said(&["one", "two", "three", "four"]), true).await;
        let err = answer(&agent, &id).await.unwrap_err();
        assert_eq!((err.0, err.1), (StatusCode::BAD_GATEWAY, "summary_failed"));
        assert!(err.2.contains("model unreachable"), "{}", err.2);

        let err = answer(&agent, "no-such-session").await.unwrap_err();
        assert_eq!((err.0, err.1), (StatusCode::NOT_FOUND, "not_found"), "{}", err.2);
    }
}
