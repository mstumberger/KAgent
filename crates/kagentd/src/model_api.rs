//! `PUT /api/sessions/:id/model` and the choice of `POST /api/sessions` (picker.md section
//! 3): the model and the effort of a conversation. One function checks both writes; the
//! work is a function over the agent and the providers, tested without a server.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use kagent_core::agent::Agent;
use kagent_core::session::{SessionMeta, SessionOrigin, SessionStore};
use kagent_providers::ProviderStore;

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

const SHAPE: &str = "expected an object with model and reasoningLevel (strings, or null to clear)";

/// What a body asks for: the outer `None` is a key that was left out, the inner one a
/// value that clears (`null` or `""`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Choice {
    pub model: Option<Option<String>>,
    pub level: Option<Option<String>>,
}

/// The body as a `Choice`; `title` is the key `POST /api/sessions` adds (returned as well).
fn parse(body: &Value, with_title: bool) -> Result<(Choice, Option<String>), HttpError> {
    let map = body.as_object().ok_or_else(|| bad_request(SHAPE))?;
    let mut choice = Choice::default();
    let mut title = None;
    for (key, value) in map {
        let text = match value {
            Value::Null => None,
            Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
            _ => return Err(bad_request(SHAPE)),
        };
        match key.as_str() {
            "model" => choice.model = Some(text),
            "reasoningLevel" => choice.level = Some(text),
            "title" if with_title => title = value.as_str().map(str::to_string),
            _ => return Err(bad_request(SHAPE)),
        }
    }
    Ok((choice, title))
}

/// The body of `PUT /api/sessions/:id/model` as a `Choice`.
pub fn choice_of(body: &Value) -> Result<Choice, String> {
    parse(body, false).map(|(choice, _)| choice).map_err(|e| e.2)
}

/// The model and the level a session ends with: a key left out keeps its value, and a
/// stored level the new model does not offer goes with the model it was for.
fn merged(store: &ProviderStore, current: Option<&SessionMeta>, default_model: &str, choice: &Choice) -> (Option<String>, Option<String>) {
    let model = choice.model.clone().unwrap_or_else(|| current.and_then(|m| m.model.clone()));
    let stored = current.and_then(|m| m.reasoning_level.clone());
    let level = match &choice.level {
        Some(level) => level.clone(),
        None if choice.model.is_some() => stored.filter(|l| offered(store, model.as_deref().unwrap_or(default_model)).contains(l)),
        None => stored,
    };
    (model, level)
}

/// The efforts `model_ref` offers (`effective.reasoningLevels`); none for a model that is not known.
fn offered(store: &ProviderStore, model_ref: &str) -> Vec<String> {
    store.resolve(model_ref).map(|r| r.config.reasoning_levels).unwrap_or_default()
}

/// The two 400s of a model and of a level. The model that answers is the one the body
/// names, else the session's, else `default_model`; at creation `current` is `None`.
pub fn check(store: &ProviderStore, current: Option<&SessionMeta>, default_model: &str, choice: &Choice) -> Result<(), HttpError> {
    if let Some(Some(model)) = &choice.model {
        if let Some(reason) = store.unusable(model) {
            return Err(bad_request(format!("model '{model}' cannot be used: {reason}")));
        }
    }
    if let Some(Some(level)) = &choice.level {
        let answering = match &choice.model {
            Some(model) => model.clone(),
            None => current.and_then(|m| m.model.clone()),
        };
        let answering = answering.unwrap_or_else(|| default_model.to_string());
        if answering.is_empty() {
            return Err(bad_request(format!("effort '{level}' needs a model: pick a model first")));
        }
        let levels = offered(store, &answering);
        if !levels.contains(level) {
            let list = if levels.is_empty() { "it offers none".to_string() } else { format!("it offers: {}", levels.join(", ")) };
            return Err(bad_request(format!("model '{answering}' has no effort '{level}' ({list})")));
        }
    }
    Ok(())
}

fn internal(e: anyhow::Error) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}"))
}

/// Set the model and the effort of session `id`; the answer is its meta. Every check runs
/// before anything is written.
pub async fn set(agent: &Arc<Agent>, store: &ProviderStore, default_model: &str, id: &str, body: &Value) -> Result<SessionMeta, HttpError> {
    let current = agent.store().meta(id).await.map_err(|_| not_found(format!("session {id} not found")))?;
    if current.origin == SessionOrigin::Subagent {
        return Err(HttpError(StatusCode::CONFLICT, "conflict", "the session of a subagent takes the model of its job: it cannot be changed".into()));
    }
    let choice = choice_of(body).map_err(bad_request)?;
    check(store, Some(&current), default_model, &choice)?;
    let (model, level) = merged(store, Some(&current), default_model, &choice);
    agent.set_model_choice(id, model, level).await.map_err(internal)
}

/// Create a session with the title and the choice of `body` (`POST /api/sessions`). A
/// refused body makes no session.
pub async fn create(sessions: &SessionStore, store: &ProviderStore, default_model: &str, body: &Value) -> Result<SessionMeta, HttpError> {
    let (choice, title) = parse(body, true)?;
    check(store, None, default_model, &choice)?;
    let (model, level) = merged(store, None, default_model, &choice);
    let mut meta = sessions.create(title.as_deref().unwrap_or("")).await.map_err(internal)?;
    if model.is_some() || level.is_some() {
        meta.model = model;
        meta.reasoning_level = level;
        sessions.save_meta(&meta).await.map_err(internal)?;
    }
    Ok(meta)
}

pub async fn put_model(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let default_model = st.settings.read().unwrap().chat_model.clone();
    let meta = set(&st.agent, &st.providers, &default_model, &id, &body).await?;
    Ok(Json(meta).into_response())
}

#[cfg(test)]
mod tests;
