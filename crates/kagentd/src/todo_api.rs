//! `PUT /api/sessions/:id/todo` (api.md section 6, plan-todo.md section 1): what the chat
//! panel sends for a tick and for Clear. The work is a function over the agent, so it is
//! tested without a server (a second daemon cannot run next to the owner's).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use kagent_core::agent::Agent;
use kagent_core::todo::{TodoError, TodoItem, TodoItemInput};

use crate::server::{ApiResult, HttpError};
use crate::AppState;

/// The `items` of the body (`{items: TodoItem[]}`): the whole list, `[]` to clear it.
/// `status` and `priority` may be left out.
pub fn items_of(body: &Value) -> Result<Vec<TodoItemInput>, String> {
    let items = body.get("items").ok_or_else(|| "items required: the whole list ([] clears it)".to_string())?;
    serde_json::from_value(items.clone()).map_err(|e| format!("items: {e}"))
}

/// Replace the list of session `id`, normalised as `todo_write` does; the answer is the
/// list that was stored. 400 for a list that is refused (over 50 items, an item without
/// content or over 200 characters, a field that is not one of the three), 404 for a
/// session that does not exist.
pub async fn replace(agent: &Arc<Agent>, id: &str, items: Vec<TodoItemInput>) -> Result<Vec<TodoItem>, HttpError> {
    agent.set_todo(id, items).await.map_err(|e| match e.downcast_ref::<TodoError>() {
        Some(_) => HttpError(StatusCode::BAD_REQUEST, "invalid_input", format!("{e:#}")),
        None if not_found(&e) => HttpError(StatusCode::NOT_FOUND, "not_found", format!("session {id} not found")),
        None => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}")),
    })
}

fn not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
}

pub async fn put_todo(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let items = items_of(&body).map_err(|m| HttpError(StatusCode::BAD_REQUEST, "invalid_input", m))?;
    let items = replace(&st.agent, &id, items).await?;
    Ok((StatusCode::OK, Json(json!({ "items": items }))).into_response())
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
    use kagent_core::protocol::{AgentEvent, ToolCall};
    use kagent_core::session::SessionStore;
    use kagent_core::todo::{TodoAccess, TodoWrite};
    use kagent_core::tool::{PrivilegeMethod, ToolRegistry};

    struct NoModel;

    #[async_trait]
    impl ModelClient for NoModel {
        async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
            anyhow::bail!("no model in this test")
        }
    }

    /// An agent with the `todo_write` tool over a temporary session store: the agent, its
    /// event sender, the session's id and the directory that lives as long as the test.
    async fn agent_with() -> (Arc<Agent>, broadcast::Sender<AgentEvent>, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let meta = store.create("t").await.unwrap();
        let access = TodoAccess::default();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TodoWrite::new(access.clone())));
        let (events, _) = broadcast::channel(16);
        let cfg = AgentConfig { model_ref: "p::m".into(), system_prompt: String::new(), max_iterations: 4, options: Default::default(), privilege: Default::default(), context_tokens: 0, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
        let agent = Agent::new(AgentDeps {
            services: None,
            audit: None,
            model: Arc::new(NoModel),
            tools: Arc::new(registry),
            permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
            broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
            store,
            events: events.clone(),
            config: Arc::new(move |_| cfg.clone()),
            on_rules_changed: None,
        });
        access.attach(&agent);
        (agent, events, meta.id, dir)
    }

    /// The session as `GET /api/sessions/:id` answers it (`api::get_session`).
    async fn shown(agent: &Arc<Agent>, id: &str) -> Value {
        json!({"meta": agent.store().meta(id).await.unwrap(), "messages": agent.messages(id).await.unwrap()})
    }

    /// The route's answer, or its status, code and message (`HttpError` has no `Debug`).
    async fn answer(agent: &Arc<Agent>, id: &str, items: Vec<TodoItemInput>) -> Result<Vec<TodoItem>, (StatusCode, &'static str, String)> {
        replace(agent, id, items).await.map_err(|HttpError(status, code, message)| (status, code, message))
    }

    #[tokio::test]
    async fn put_todo_stores_a_normalised_list_like_todo_write() {
        let (agent, events, id, _dir) = agent_with().await;
        let mut rx = events.subscribe();
        let body = json!({"items": [
            {"content": "Check disk", "status": "in_progress", "priority": "high"},
            {"content": "Check memory", "status": "in_progress"},
            {"content": "Summarize"}
        ]});
        let stored = answer(&agent, &id, items_of(&body).unwrap()).await.unwrap();
        let want = json!([
            {"content": "Check disk", "status": "in_progress", "priority": "high"},
            {"content": "Check memory", "status": "pending", "priority": "medium"},
            {"content": "Summarize", "status": "pending", "priority": "medium"}
        ]);
        assert_eq!(json!({ "items": stored }), json!({ "items": want }), "the answer `{{items: TodoItem[]}}` is the list the daemon stored");
        assert_eq!(shown(&agent, &id).await["meta"]["todo"], want, "GET /api/sessions/:id carries it in the meta");
        let AgentEvent::TodoUpdated { session_id, items } = rx.try_recv().unwrap() else { panic!("the route sends todoUpdated too") };
        assert_eq!((session_id, json!(items)), (id.clone(), want.clone()));
        assert!(rx.try_recv().is_err(), "one event per change");

        // Clear: an empty list.
        assert!(answer(&agent, &id, vec![]).await.unwrap().is_empty());
        assert_eq!(shown(&agent, &id).await["meta"]["todo"], json!([]), "the session has no list again");
        assert!(matches!(rx.try_recv(), Ok(AgentEvent::TodoUpdated { items, .. }) if items.is_empty()), "said with the empty list");

        // The same input through the tool (another session) gives the same list.
        let other = agent.store().create("tool").await.unwrap().id;
        let call = ToolCall { id: "c".into(), name: "todo_write".into(), input: body };
        let (msg, _) = agent.execute_call(&other, "ui", &call, &CancellationToken::new(), PrivilegeMethod::None).await;
        assert!(!msg.is_error, "{}", msg.content);
        assert_eq!(shown(&agent, &other).await["meta"]["todo"], want, "the route normalises like todo_write does");
    }

    #[tokio::test]
    async fn a_refused_list_is_400_and_an_unknown_session_404() {
        let (agent, _events, id, _dir) = agent_with().await;
        let many: Vec<TodoItemInput> = (0..51).map(|i| TodoItemInput { content: format!("step {i}"), status: None, priority: None }).collect();
        let err = answer(&agent, &id, many).await.unwrap_err();
        assert_eq!((err.0, err.1), (StatusCode::BAD_REQUEST, "invalid_input"), "{}", err.2);
        assert!(err.2.contains("at most 50"), "{}", err.2);
        assert!(shown(&agent, &id).await["meta"]["todo"].as_array().unwrap().is_empty(), "nothing was stored");

        let one = items_of(&json!({"items": [{"content": "a"}]})).unwrap();
        let err = answer(&agent, "no-such-session", one).await.unwrap_err();
        assert_eq!((err.0, err.1), (StatusCode::NOT_FOUND, "not_found"), "{}", err.2);

        // The body: `items` is a list of items; status and priority are left out or one of their values.
        assert!(items_of(&json!({})).is_err(), "items is required");
        assert!(items_of(&json!({"items": "a"})).is_err());
        assert!(items_of(&json!({"items": [{"content": "a", "status": "done"}]})).is_err(), "a status that does not exist");
        assert!(items_of(&json!({"items": [{"content": "a", "stat": "done"}]})).is_err(), "a misspelt field is refused, not ignored");
        assert!(items_of(&json!({"items": [{"status": "pending"}]})).is_err(), "content is required");
        assert!(items_of(&json!({"items": []})).unwrap().is_empty(), "an empty list clears");
    }
}
