//! The route of usage (usage.md section 5, api.md section 11).

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

/// `GET /api/usage`: every provider, with its counters (and, from the limits, its limits).
pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.usage.view()))
}
