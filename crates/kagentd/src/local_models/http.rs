//! The routes of local models (local-models.md section 3, api.md section 3c). The work is in
//! `LocalModels`; these handlers parse and answer.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::pull::Refusal;
use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

fn ok_json(v: Value) -> Response {
    (StatusCode::OK, Json(v)).into_response()
}

/// `GET /api/local-models`.
pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.local_models.view().await))
}

/// A refusal of the module as the status of an answer.
pub fn http_error(r: Refusal) -> HttpError {
    match r {
        Refusal::Invalid(m) => bad_request(m),
        Refusal::Conflict(m) => HttpError(StatusCode::CONFLICT, "conflict", m),
        Refusal::Missing(m) => not_found(m),
        Refusal::Upstream(m) => HttpError(StatusCode::BAD_GATEWAY, "ollama_error", m),
    }
}

/// `POST /api/local-models/pull {id}`: 202 once the download is queued (or running).
pub async fn pull(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let id = body.get("id").and_then(Value::as_str).ok_or_else(|| bad_request("id required: an id of the list of recommended models"))?;
    st.local_models.pull(id).await.map_err(http_error)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"id": id, "pulls": st.local_models.pull_rows()}))).into_response())
}

/// `DELETE /api/local-models/pull/:id`: cancel (or dismiss a failed row).
pub async fn cancel(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    st.local_models.cancel(&id).map_err(http_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /api/local-models/:id`: remove an installed model of the list.
pub async fn remove(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    st.local_models.remove(&id).await.map_err(http_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
