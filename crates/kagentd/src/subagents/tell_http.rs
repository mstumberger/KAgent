//! `POST /api/agents/:id/message {text, interrupt}` (steering.md sections 3 and 4): the owner's message to a running subagent,
//! read at its next model request, or with `interrupt` after its current request or command is cancelled. The route is the
//! owner's (under the API token); it targets a subagent's session and no other, and it never answers an approval card.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::agent::children::TellError;
use kagent_core::agent::Agent;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Body {
    text: String,
    #[serde(default)]
    interrupt: bool,
}

/// What the route answers for one message: 202 and the receipt, or the refusal.
pub async fn tell(agent: &Arc<Agent>, id: &str, body: &Value) -> Result<Value, HttpError> {
    let b: Body = serde_json::from_value(body.clone()).map_err(|e| bad_request(format!("expected {{text, interrupt?}}: {e}")))?;
    match agent.tell_child(None, id, &b.text, b.interrupt).await {
        Ok(view) => {
            let receipt = view.steering.last().map(|s| json!({"id": s.id, "state": s.state})).unwrap_or(json!({}));
            Ok(receipt)
        }
        Err(TellError::Unknown(_)) => Err(not_found(format!("no subagent '{id}'"))),
        Err(TellError::Invalid(why)) => Err(bad_request(why)),
        Err(e @ (TellError::NotStarted | TellError::Ended(_))) => Err(HttpError(StatusCode::CONFLICT, "conflict", e.to_string())),
    }
}

pub async fn post(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok((StatusCode::ACCEPTED, Json(tell(&st.agent, &id, &body).await?)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use async_trait::async_trait;
    use kagent_core::agent::children::{ChildRequest, Profile};
    use kagent_core::agent::{AgentConfig, AgentDeps};
    use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
    use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
    use kagent_core::session::SessionStore;
    use kagent_core::tool::ToolRegistry;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    struct NoModel;

    #[async_trait]
    impl ModelClient for NoModel {
        async fn complete(&self, _r: ModelRequest, _d: mpsc::UnboundedSender<ModelDelta>, _c: CancellationToken) -> anyhow::Result<ModelResponse> {
            anyhow::bail!("no model in this test")
        }
    }

    async fn agent() -> (Arc<Agent>, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path().join("sessions")).unwrap();
        let parent = store.create("parent").await.unwrap().id;
        let (events, _rx) = tokio::sync::broadcast::channel(16);
        let cfg = AgentConfig { model_ref: String::new(), system_prompt: String::new(), max_iterations: 4, options: Default::default(), privilege: Default::default(), context_tokens: 0, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
        let agent = Agent::new(AgentDeps {
            services: None,
            hooks: None,
            audit: None,
            model: Arc::new(NoModel),
            tools: Arc::new(ToolRegistry::new()),
            permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
            broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
            store,
            events,
            config: Arc::new(move |_| cfg.clone()),
            on_rules_changed: None,
        });
        (agent, parent, dir)
    }

    fn refused(r: Result<Value, HttpError>) -> (StatusCode, &'static str, String) {
        let HttpError(status, code, message) = r.unwrap_err();
        (status, code, message)
    }

    #[tokio::test]
    async fn the_route_refuses_what_it_cannot_deliver_with_a_status_that_says_why() {
        let (agent, parent, _dir) = agent().await;
        // A child that failed at once (no model): it ended.
        let child = agent.start_child(ChildRequest { parent: parent.clone(), description: "job".into(), prompt: "look".into(), profile: Profile::general(), model: None, worker: false }).await.unwrap();
        for _ in 0..200 {
            if agent.children_of(&parent).iter().all(|c| c.status.ended()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let ended = refused(tell(&agent, &child.id, &json!({"text": "hello"})).await);
        assert_eq!((ended.0, ended.1), (StatusCode::CONFLICT, "conflict"));
        assert!(ended.2.starts_with("the worker already ended ("), "{}", ended.2);
        let interrupted = refused(tell(&agent, &child.id, &json!({"text": "hello", "interrupt": true})).await);
        assert_eq!(interrupted.0, StatusCode::CONFLICT);
        // A session that is not a subagent, and one that does not exist: 404.
        assert_eq!(refused(tell(&agent, &parent, &json!({"text": "hello"})).await).0, StatusCode::NOT_FOUND, "the owner's conversation is not a worker");
        assert_eq!(refused(tell(&agent, "nope", &json!({"text": "hello"})).await).0, StatusCode::NOT_FOUND);
        // The body: the text is 1 to 2000 characters, nothing else is a field.
        assert_eq!(refused(tell(&agent, &child.id, &json!({"text": ""})).await).0, StatusCode::BAD_REQUEST);
        assert_eq!(refused(tell(&agent, &child.id, &json!({"text": "x".repeat(2001)})).await).0, StatusCode::BAD_REQUEST);
        let extra = refused(tell(&agent, &child.id, &json!({"text": "hi", "approve": true})).await);
        assert!(extra.0 == StatusCode::BAD_REQUEST && extra.2.contains("unknown field `approve`"), "{}", extra.2);
        assert_eq!(refused(tell(&agent, &child.id, &json!({})).await).0, StatusCode::BAD_REQUEST);
    }
}
