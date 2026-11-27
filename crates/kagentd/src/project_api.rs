//! The folder of a conversation over HTTP (sidebar.md section 3): `PUT /api/sessions/:id/project`,
//! `PUT /api/sessions/:id/pin`, the folder of `POST /api/sessions`, `GET /api/projects` and
//! the folder filter of `GET /api/sessions`. The work is in functions over the agent and the
//! settings, tested without a server.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use kagent_core::agent::hooks::protect;
use kagent_core::agent::Agent;
use kagent_core::session::{SessionMeta, SessionOrigin, SessionStore};
use kagent_core::tools::xdg;

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::settings::Settings;
use crate::AppState;

const PATH_SHAPE: &str = "expected {path: a folder, or null to detach}";
const PIN_SHAPE: &str = "expected {pinned: true or false}";

fn internal(e: anyhow::Error) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}"))
}

fn conflict(msg: &str) -> HttpError {
    HttpError(StatusCode::CONFLICT, "conflict", msg.into())
}

/// The folder a body asks for: `None` for `null` or `""` (detach).
pub fn path_of(body: &Value) -> Result<Option<String>, String> {
    match body.as_object().and_then(|m| m.get("path")) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        _ => Err(PATH_SHAPE.into()),
    }
}

/// A folder KAgent may be attached to: the canonical path. 400 for a path that is not
/// absolute (or `~/...`), that is not a directory, or that is KAgent's own `<config>` or
/// `<data>` or inside them (the rule of `coders.md` section 3).
pub async fn check(path: &str) -> Result<PathBuf, HttpError> {
    check_with(path, protect::dirs()).await
}

/// `check` against the given `<config>` and `<data>`.
async fn check_with(path: &str, own: (PathBuf, PathBuf)) -> Result<PathBuf, HttpError> {
    if !path.starts_with('/') && !path.starts_with("~/") {
        return Err(bad_request("path must be absolute, or start with ~/"));
    }
    let given = xdg::expand(path);
    let not_dir = || bad_request(format!("not a directory: {path}"));
    if !tokio::fs::metadata(&given).await.is_ok_and(|m| m.is_dir()) {
        return Err(not_dir());
    }
    let canonical = tokio::fs::canonicalize(&given).await.map_err(|_| not_dir())?;
    for own in [own.0, own.1] {
        let own = std::fs::canonicalize(&own).unwrap_or(own);
        if canonical.starts_with(&own) {
            return Err(bad_request(format!(
                "{path} is KAgent's own folder (<config> or <data>): a conversation cannot be attached to it"
            )));
        }
    }
    Ok(canonical)
}

/// Set or clear the folder of session `id`; the answer is its meta. Every check runs
/// before anything is written.
pub async fn set_project(agent: &Arc<Agent>, id: &str, body: &Value) -> Result<SessionMeta, HttpError> {
    let current = agent.store().meta(id).await.map_err(|_| not_found(format!("session {id} not found")))?;
    if current.origin == SessionOrigin::Subagent {
        return Err(conflict("the session of a subagent cannot be changed: it has the folder of the session that started it"));
    }
    let path = match path_of(body).map_err(bad_request)? {
        Some(path) => Some(check(&path).await?.to_string_lossy().to_string()),
        None => None,
    };
    agent.set_project(id, path).await.map_err(internal)
}

/// Pin or unpin session `id`.
pub async fn set_pinned(agent: &Arc<Agent>, id: &str, body: &Value) -> Result<SessionMeta, HttpError> {
    let current = agent.store().meta(id).await.map_err(|_| not_found(format!("session {id} not found")))?;
    if current.origin == SessionOrigin::Subagent {
        return Err(conflict("the session of a subagent is not in the list: it cannot be pinned"));
    }
    let pinned = body.as_object().and_then(|m| m.get("pinned")).and_then(Value::as_bool).ok_or_else(|| bad_request(PIN_SHAPE))?;
    agent.set_pinned(id, pinned).await.map_err(internal)
}

/// Give a session that was just created its folder (`path` is what `check` answered): the
/// field is set and the meta saved once more. No event: nobody lists the session yet.
pub async fn create_in(sessions: &SessionStore, mut meta: SessionMeta, path: PathBuf) -> Result<SessionMeta, HttpError> {
    meta.project = Some(path.to_string_lossy().to_string());
    sessions.save_meta(&meta).await.map_err(internal)?;
    Ok(meta)
}

/// `GET /api/projects`: the roots and the folders under them that look like projects.
pub async fn projects(settings: &Settings) -> Value {
    let roots = settings.desktop_prefs().project_roots;
    let expanded: Vec<String> = if roots.is_empty() { xdg::DEFAULT_PROJECT_ROOTS.iter().map(|r| xdg::expand(r)).collect::<Vec<_>>() } else { roots.iter().map(|r| xdg::expand(r)).collect() }
        .into_iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let list = tokio::task::spawn_blocking(move || xdg::projects(&roots)).await.unwrap_or_default();
    let list: Vec<Value> = list
        .into_iter()
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let agents_md = std::fs::metadata(p.join("AGENTS.md")).is_ok_and(|m| m.is_file());
            json!({"path": p.to_string_lossy(), "name": name, "agentsMd": agents_md})
        })
        .collect();
    json!({"roots": expanded, "projects": list})
}

/// The sessions whose folder is `path` (canonicalized when it is a directory, else compared as it is).
pub fn in_project(all: Vec<SessionMeta>, path: &str) -> Vec<SessionMeta> {
    let wanted = std::fs::canonicalize(xdg::expand(path)).ok().filter(|p| p.is_dir()).map_or_else(|| path.to_string(), |p| p.to_string_lossy().to_string());
    all.into_iter().filter(|m| m.project.as_deref() == Some(wanted.as_str())).collect()
}

/// `GET /api/sessions` takes `parent` or `project`, not both.
pub fn list_filters(parent: Option<&str>, project: Option<&str>) -> Result<(), HttpError> {
    if parent.is_some() && project.is_some() {
        return Err(bad_request("give parent or project, not both"));
    }
    Ok(())
}

/// `POST /api/sessions` takes `project` beside the keys of `model_api`: the body without it,
/// and the folder (checked, canonical) when there is one.
pub async fn split_create(body: &Value) -> Result<(Value, Option<PathBuf>), HttpError> {
    let mut rest = body.clone();
    let Some(project) = rest.as_object_mut().and_then(|m| m.remove("project")) else { return Ok((rest, None)) };
    match project {
        Value::Null => Ok((rest, None)),
        Value::String(s) if s.trim().is_empty() => Ok((rest, None)),
        Value::String(s) => Ok((rest, Some(check(s.trim()).await?))),
        _ => Err(bad_request(PATH_SHAPE)),
    }
}

pub async fn put_project(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok(Json(set_project(&st.agent, &id, &body).await?).into_response())
}

pub async fn put_pin(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok(Json(set_pinned(&st.agent, &id, &body).await?).into_response())
}

pub async fn get_projects(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    let settings = st.settings.read().unwrap().clone();
    Ok(Json(projects(&settings).await).into_response())
}

#[cfg(test)]
mod tests;
