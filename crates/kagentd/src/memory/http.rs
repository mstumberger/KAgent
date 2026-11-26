//! The memory routes of api.md section 6: list, search, create, change, forget, delete.
//! The work is in functions over the hub, so the validation is tested without a server
//! (a second daemon cannot run next to the owner's); the handlers wrap them.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::Value;

use super::file::one_line;
use super::{Memory, MemoryError, MemoryHub, MemorySource, MemoryType, MemoryView, NewMemory, SaveMode, TYPE_NAMES};
use crate::server::{bad_request, ApiResult, HttpError};
use crate::AppState;

/// How many memories a search answers at most.
pub const MAX_RESULTS: usize = 10;
const NEW_FIELDS: [&str; 4] = ["name", "description", "type", "content"];
const EDIT_FIELDS: [&str; 4] = ["description", "type", "content", "reviewed"];

/// `GET /api/memory`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOverview {
    /// settings.memory.enabled.
    pub enabled: bool,
    /// `user.md` and `machine.md`.
    pub core: Vec<MemoryView>,
    /// The topic memories by name, the superseded ones included (they say so).
    pub memories: Vec<MemoryView>,
    pub stats: MemoryStats,
}

#[derive(Debug, Serialize)]
pub struct MemoryStats {
    /// Every memory file, the two core files included (the limit of 200 counts them).
    pub files: usize,
    /// Their size together.
    pub bytes: u64,
    /// Written by the agent and not yet seen by the owner, the current ones.
    pub unreviewed: usize,
}

pub fn overview(hub: &MemoryHub) -> MemoryOverview {
    let all = hub.all();
    let today = super::today();
    let view = |m: &Memory| MemoryView::of(m, &hub.store().path(&m.name), true);
    MemoryOverview {
        enabled: hub.enabled(),
        core: all.iter().filter(|m| m.is_core()).map(view).collect(),
        memories: all.iter().filter(|m| !m.is_core()).map(view).collect(),
        stats: MemoryStats {
            files: all.len(),
            bytes: all.iter().map(|m| m.bytes).sum(),
            unreviewed: all.iter().filter(|m| !m.superseded(today) && m.unreviewed()).count(),
        },
    }
}

/// `q` of `GET /api/memory/search`.
pub fn query_of(params: &HashMap<String, String>) -> Result<&str, HttpError> {
    params.get("q").map(String::as_str).map(str::trim).filter(|q| !q.is_empty()).ok_or_else(|| bad_request("q is required: the words to look for"))
}

/// The current memories that match, without their content.
pub fn find(hub: &MemoryHub, query: &str) -> Vec<MemoryView> {
    hub.search(query, MAX_RESULTS).iter().map(|m| MemoryView::of(m, &hub.store().path(&m.name), false)).collect()
}

impl From<MemoryError> for HttpError {
    fn from(e: MemoryError) -> Self {
        match e {
            MemoryError::Invalid(m) => HttpError(StatusCode::BAD_REQUEST, "invalid_input", m),
            MemoryError::Unknown(m) => HttpError(StatusCode::NOT_FOUND, "not_found", m),
            MemoryError::Exists(m) | MemoryError::Core(m) => HttpError(StatusCode::CONFLICT, "conflict", m),
            MemoryError::Full => HttpError(StatusCode::CONFLICT, "conflict", e.to_string()),
            MemoryError::Io(m) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", m),
        }
    }
}

fn only(body: &Value, fields: &[&str]) -> Result<(), HttpError> {
    let Some(object) = body.as_object() else { return Err(bad_request("expected an object")) };
    for key in object.keys() {
        if !fields.contains(&key.as_str()) {
            return Err(bad_request(format!("unknown field '{key}'")));
        }
    }
    Ok(())
}

/// A field that must be there, as text that is not blank.
fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, HttpError> {
    body.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad_request(format!("{key} is required")))
}

/// A field that may be left out; text when it is there.
fn optional<'a>(body: &'a Value, key: &str) -> Result<Option<&'a str>, HttpError> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(bad_request(format!("{key} must be text"))),
    }
}

/// `type` of the body. For a core file it may only be its own.
fn type_of(value: &Value, core: Option<MemoryType>) -> Result<MemoryType, HttpError> {
    let name = value.as_str().unwrap_or("");
    if let Some(core) = core {
        return if name == core.as_str() { Ok(core) } else { Err(bad_request("the type of a core file does not change")) };
    }
    MemoryType::parse(name).ok_or_else(|| bad_request(format!("type must be {TYPE_NAMES}")))
}

/// `POST /api/memory`: a memory of the owner's.
pub fn insert(hub: &MemoryHub, body: &Value) -> Result<Memory, HttpError> {
    only(body, &NEW_FIELDS)?;
    hub.save(
        &NewMemory {
            name: text(body, "name")?.to_string(),
            description: optional(body, "description")?.unwrap_or_default().to_string(),
            r#type: type_of(body.get("type").unwrap_or(&Value::Null), None)?,
            body: text(body, "content")?.to_string(),
            source: MemorySource::User,
        },
        None,
        SaveMode::Create,
    )
    .map(|(memory, _)| memory)
    .map_err(|e| match e {
        // The name is not one a topic memory can have (it is the owner's `user.md`).
        MemoryError::Core(m) => bad_request(m),
        e => HttpError::from(e),
    })
}

/// `PUT /api/memory/:name`: a field left out keeps its value. `reviewed` alone marks a
/// memory seen; any other change makes it the owner's (`source: user`, `reviewed: true`).
/// A core file changes its content only.
pub fn edit(hub: &MemoryHub, name: &str, body: &Value) -> Result<Memory, HttpError> {
    only(body, &EDIT_FIELDS)?;
    if body.as_object().is_none_or(|o| o.is_empty()) {
        return Err(bad_request(format!("nothing to change: one of {}", EDIT_FIELDS.join(", "))));
    }
    let description = optional(body, "description")?;
    let r#type = body.get("type").filter(|v| !v.is_null()).cloned();
    let content = optional(body, "content")?;
    let reviewed = match body.get("reviewed") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(bad_request("reviewed must be true or false")),
    };
    let current = hub.get(name).ok_or_else(|| HttpError(StatusCode::NOT_FOUND, "not_found", format!("there is no memory named '{name}'")))?;
    if current.is_core() && (description.is_some() || r#type.is_some()) {
        return Err(bad_request(format!("'{name}' is a core file: only its content changes")));
    }
    // A topic memory keeps a content; the text of a core file is the owner's to clear.
    if !current.is_core() && content.is_some_and(|c| c.trim().is_empty()) {
        return Err(bad_request("content is required"));
    }
    // Any change but the flag makes the memory the owner's (memory.md section 1).
    let edits = description.is_some() || r#type.is_some() || content.is_some();
    let r#type = match &r#type { Some(t) => type_of(t, None)?, None => current.r#type };
    hub.update(name, move |m| {
        if edits {
            m.source = MemorySource::User;
            m.reviewed = true;
        }
        if let Some(d) = description {
            m.description = one_line(d);
        }
        m.r#type = r#type;
        if let Some(c) = content {
            m.body = c.trim().to_string();
        }
        if let Some(r) = reviewed {
            m.reviewed = r;
        }
        Ok(())
    })
    .map_err(HttpError::from)
}

/// `POST /api/memory/:name/forget`: "Forget" of the UI, as `memory_forget` does it.
pub fn mark_forgotten(hub: &MemoryHub, name: &str) -> Result<Memory, HttpError> {
    hub.forget(name).map_err(HttpError::from)
}

/// `DELETE /api/memory/:name`.
pub fn erase(hub: &MemoryHub, name: &str) -> Result<(), HttpError> {
    hub.remove(name).map_err(HttpError::from)
}

fn shown(hub: &MemoryHub, memory: &Memory) -> MemoryView {
    MemoryView::of(memory, &hub.store().path(&memory.name), true)
}

pub async fn list(State(st): State<Arc<AppState>>) -> Json<MemoryOverview> {
    Json(overview(&st.memory))
}

pub async fn search(State(st): State<Arc<AppState>>, Query(params): Query<HashMap<String, String>>) -> ApiResult<Json<Vec<MemoryView>>> {
    Ok(Json(find(&st.memory, query_of(&params)?)))
}

pub async fn create(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let memory = insert(&st.memory, &body)?;
    Ok((StatusCode::CREATED, Json(shown(&st.memory, &memory))).into_response())
}

pub async fn update(State(st): State<Arc<AppState>>, Path(name): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<MemoryView>> {
    Ok(Json(shown(&st.memory, &edit(&st.memory, &name, &body)?)))
}

pub async fn forget(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult<Json<MemoryView>> {
    Ok(Json(shown(&st.memory, &mark_forgotten(&st.memory, &name)?)))
}

pub async fn delete(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> ApiResult<StatusCode> {
    erase(&st.memory, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;
