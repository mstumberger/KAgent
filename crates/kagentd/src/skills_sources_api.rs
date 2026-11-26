//! The import and sources routes of api.md section 6 (S21): skills of other tools, and
//! git sources. The handlers parse the requests, hold the lock that every change to the
//! imported skills holds, switch the new skills off before the first file is written,
//! and apply the step. The work is in `steps` (tested without a server).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path as Route, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::{bad_request, ApiResult};
use crate::skills_api::quarantine_all;
use crate::skills_import;
use crate::skills_sources as sources;
use crate::AppState;

mod steps;

pub use steps::{add_step, import_plan, note_failure, remove_step, sync_step};
use steps::internal;

// ------------------------------------------------------------------ shared with the handlers

/// Add `names` to `skills.disabled`, the way a save from the settings page does: every
/// UI is told, and the library reads the list now. Imported and sourced skills are
/// switched off before the first file is written (skills.md sections 7, 8 and 10).
pub async fn quarantine(st: &Arc<AppState>, names: &[String]) -> ApiResult<()> {
    if names.is_empty() {
        return Ok(());
    }
    let current = st.settings.read().unwrap().skills_config().disabled;
    let next = quarantine_all(&current, names);
    if next != current {
        crate::api::put_settings(State(st.clone()), Json(json!({ "skills": { "disabled": next } }))).await?;
    }
    st.skills.refresh(&st.settings.read().unwrap().skills_config());
    Ok(())
}

// ------------------------------------------------------------------ handlers

pub async fn import_sources(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    let home = dirs::home_dir().unwrap_or_default();
    let installed = st.skills.installed();
    let found = tokio::task::spawn_blocking(move || skills_import::detect(&home, &installed)).await.map_err(internal)?;
    Ok(Json(json!({ "sources": found })).into_response())
}

pub async fn import(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let writers = st.skill_sources.lock().await;
    let step = import_plan(&st.skills, &body)?;
    // Off before the first file is written (skills.md section 10).
    quarantine(&st, &step.to_disable()).await?;
    let home = dirs::home_dir().unwrap_or_default();
    let outcome = step.apply(&writers, &st.skills, &st.skill_sources, &home).await?;
    let disabled = outcome.imported.clone();
    Ok(Json(json!({ "imported": outcome.imported, "skipped": outcome.skipped, "disabled": disabled })).into_response())
}

pub async fn list_sources(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    let views: Vec<sources::SourceView> = st.skill_sources.list().iter().map(sources::view_of).collect();
    Ok(Json(json!({ "sources": views, "catalog": sources::catalog() })).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddBody {
    url: String,
    #[serde(default)]
    r#ref: Option<String>,
    #[serde(default)]
    subdir: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// The answer of an add: the source, and what its first install did.
pub async fn add_source(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let req: AddBody = serde_json::from_value(body.clone()).map_err(|e| bad_request(format!("expected an object with url, ref, subdir and name as text: {e}")))?;
    let writers = st.skill_sources.lock().await;
    let step = add_step(&st.skills, &st.skill_sources, &writers, Some(&req.url), req.r#ref.as_deref(), req.subdir.as_deref(), req.name.as_deref()).await?;
    quarantine(&st, &step.to_disable()).await?;
    let id = step.id().to_string();
    let (_, report) = step.apply(&writers, &st.skills, &st.skill_sources).await?;
    let mut answer = serde_json::to_value(st.skill_sources.get(&id).as_ref().map(sources::view_of).ok_or_else(|| internal("the source is not saved"))?).map_err(internal)?;
    answer["added"] = json!(report.added);
    answer["skipped"] = json!(report.skipped);
    Ok((StatusCode::CREATED, Json(answer)).into_response())
}

pub async fn sync_source(State(st): State<Arc<AppState>>, Route(id): Route<String>) -> ApiResult<Response> {
    let writers = st.skill_sources.lock().await;
    let step = match sync_step(&st.skills, &st.skill_sources, &writers, &id).await {
        Ok(step) => step,
        Err(refusal) => {
            note_failure(&st.skill_sources, &id, &refusal.2);
            return Err(refusal.into());
        }
    };
    quarantine(&st, &step.to_disable()).await?;
    match step.apply(&writers, &st.skills, &st.skill_sources).await {
        Ok((_, report)) => Ok(Json(report).into_response()),
        Err(refusal) => {
            note_failure(&st.skill_sources, &id, &refusal.2);
            Err(refusal.into())
        }
    }
}

/// `removeSkills` comes in the body (`{removeSkills: true}`, api.md) or as
/// `?removeSkills=true`; without it the copies stay.
pub async fn remove_source(State(st): State<Arc<AppState>>, Route(id): Route<String>, Query(query): Query<HashMap<String, String>>, body: Bytes) -> ApiResult<Response> {
    let from_body = if body.is_empty() {
        None
    } else {
        let value: Value = serde_json::from_slice(&body).map_err(|e| bad_request(format!("expected an object with removeSkills (true or false): {e}")))?;
        Some(value.get("removeSkills").and_then(Value::as_bool).ok_or_else(|| bad_request("removeSkills must be true or false"))?)
    };
    let remove = from_body.unwrap_or_else(|| query.get("removeSkills").map(String::as_str) == Some("true"));
    let writers = st.skill_sources.lock().await;
    remove_step(&st.skills, &st.skill_sources, &writers, &id, remove).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
