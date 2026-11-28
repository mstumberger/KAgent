//! The routes of local models (local-models.md section 3, api.md section 3c). The work is in
//! `LocalModels`; these handlers parse and answer.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::server::ApiResult;
use crate::AppState;

fn ok_json(v: Value) -> Response {
    (StatusCode::OK, Json(v)).into_response()
}

/// `GET /api/local-models`.
pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.local_models.view().await))
}
