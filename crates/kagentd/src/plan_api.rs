//! `POST /api/sessions/:id/plan` (api.md section 6, plan-todo.md section 2): what "Cancel
//! plan" sends, and the plan flag of a message for `api::post_message`. The work is a
//! function over the agent, so it is tested without a server (a second daemon cannot run
//! next to the owner's).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use kagent_core::agent::Agent;

use crate::server::{ApiResult, HttpError};
use crate::AppState;

/// The text `post_message` sends, and whether its turn starts in plan mode (plan-todo.md
/// section 2): `/plan <request>` loses the prefix and plans; any other text goes as it is
/// and `flag`, the `plan` field of the body, says.
pub fn split_plan(text: String, flag: Option<bool>) -> (String, bool) {
    match text.trim_start().strip_prefix("/plan") {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => (rest.trim().to_string(), true),
        _ => (text, flag == Some(true)),
    }
}

/// The `active` of the body (`{active: boolean}`).
pub fn active_of(body: &Value) -> Result<bool, String> {
    match body.get("active") {
        Some(Value::Bool(active)) => Ok(*active),
        Some(_) => Err("active must be true or false".to_string()),
        None => Err("active required: false leaves plan mode".to_string()),
    }
}

/// Put session `id` in or out of plan mode; the answer is its flag. 404 for a session that
/// does not exist.
pub async fn set(agent: &Arc<Agent>, id: &str, active: bool) -> Result<bool, HttpError> {
    agent.set_planning(id, active).await.map(|_| active).map_err(|e| match e.downcast_ref::<std::io::Error>() {
        Some(io) if io.kind() == std::io::ErrorKind::NotFound => HttpError(StatusCode::NOT_FOUND, "not_found", format!("session {id} not found")),
        _ => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}")),
    })
}

pub async fn post_plan(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let active = active_of(&body).map_err(|m| HttpError(StatusCode::BAD_REQUEST, "invalid_input", m))?;
    let planning = set(&st.agent, &id, active).await?;
    Ok(Json(json!({ "planning": planning })).into_response())
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
    use kagent_core::plan::{PlanAccess, PlanEnter, PlanExit};
    use kagent_core::protocol::AgentEvent;
    use kagent_core::session::SessionStore;
    use kagent_core::tool::ToolRegistry;

    struct NoModel;

    #[async_trait]
    impl ModelClient for NoModel {
        async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
            anyhow::bail!("no model in this test")
        }
    }

    /// An agent with the two plan tools over a temporary session store: the agent, its
    /// event sender, the session's id and the directory that lives as long as the test.
    async fn agent_with() -> (Arc<Agent>, broadcast::Sender<AgentEvent>, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let meta = store.create("t").await.unwrap();
        let access = PlanAccess::default();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(PlanEnter::new(access.clone())));
        registry.register(Arc::new(PlanExit::new(access.clone(), dir.path().join("plans"))));
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

    /// The flag the route answers, or its status, code and message (`HttpError` has no `Debug`).
    async fn answer(agent: &Arc<Agent>, id: &str, active: bool) -> Result<bool, (StatusCode, &'static str, String)> {
        set(agent, id, active).await.map_err(|HttpError(status, code, message)| (status, code, message))
    }

    #[tokio::test]
    async fn the_route_sets_the_flag_says_so_and_answers_it() {
        let (agent, events, id, _dir) = agent_with().await;
        assert!(agent.set_planning(&id, true).await.unwrap(), "the session plans");
        let mut rx = events.subscribe();
        assert_eq!(answer(&agent, &id, false).await, Ok(false), "`{{active: false}}` answers `{{planning: false}}`");
        assert!(!agent.planning(&id).await.unwrap());
        let AgentEvent::PlanUpdated { session_id, active } = rx.try_recv().unwrap() else { panic!("the change is said with planUpdated") };
        assert_eq!((session_id, active), (id.clone(), false));

        // `{active: true}` plans without a turn; asking again changes nothing.
        assert_eq!(answer(&agent, &id, true).await, Ok(true));
        assert!(agent.planning(&id).await.unwrap());
        assert!(matches!(rx.try_recv(), Ok(AgentEvent::PlanUpdated { active: true, .. })));
        assert_eq!(answer(&agent, &id, true).await, Ok(true));
        assert!(rx.try_recv().is_err(), "no second event");

        let error = answer(&agent, "no-such-session", false).await.unwrap_err();
        assert_eq!((error.0, error.1), (StatusCode::NOT_FOUND, "not_found"), "{}", error.2);
        assert!(answer(&agent, "no-such-session", true).await.is_err(), "planning an unknown session is 404 too");

        // The body.
        assert_eq!(active_of(&json!({ "active": false })), Ok(false));
        assert_eq!(active_of(&json!({ "active": true })), Ok(true));
        assert!(active_of(&json!({})).is_err(), "active is required");
        assert!(active_of(&json!({ "active": "false" })).is_err(), "a boolean, not its text");
    }

    #[test]
    fn a_plan_prefix_starts_the_turn_in_plan_mode_and_is_dropped() {
        assert_eq!(split_plan("/plan free 20 GB of disk".into(), None), ("free 20 GB of disk".into(), true));
        assert_eq!(split_plan("  /plan\tfree disk ".into(), Some(false)), ("free disk".into(), true), "the flag of the body does not come between");
        assert_eq!(split_plan("/plan".into(), None), (String::new(), true), "the bare command: no text, so the route answers `text required`");
        assert_eq!(split_plan("/planner notes".into(), None), ("/planner notes".into(), false), "only the command, as a word");
        // A skill behind the prefix keeps its turn: the text is what `turn_opts` sees.
        assert_eq!(split_plan("/plan /slow-machine why so slow".into(), None), ("/slow-machine why so slow".into(), true));
        assert_eq!(split_plan("fix it".into(), Some(true)), ("fix it".into(), true), "the Plan toggle sends `plan: true`");
        assert_eq!(split_plan("fix it".into(), Some(false)), ("fix it".into(), false));
        assert_eq!(split_plan(" fix it ".into(), None), (" fix it ".into(), false), "any other message goes as it was");
    }
}
