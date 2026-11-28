//! The routes of the screen watch (screen-watch.md section 4, api.md).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::server::{not_found, ApiResult};
use crate::AppState;

fn ok_json(v: Value) -> Response {
    (StatusCode::OK, Json(v)).into_response()
}

/// `GET /api/screen-watch`: the running watch, or `null`.
pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(serde_json::to_value(st.screen_watch.current()).unwrap_or(Value::Null)))
}

/// `DELETE /api/screen-watch`: stop it. The watch as it ended; 404 when none runs.
pub async fn delete(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    match st.screen_watch.stop() {
        Some(w) => Ok(ok_json(serde_json::to_value(w).unwrap_or(Value::Null))),
        None => Err(not_found("no screen watch is running")),
    }
}

/// `GET /api/screen-watch/model`: the local vision model a watch would use, for the card of the settings page.
pub async fn model(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(match st.screen_watch.env.local_model().await {
        Ok(m) => json!({"model": m.id, "server": m.base, "refusal": null}),
        Err(refusal) => json!({"model": null, "server": null, "refusal": refusal}),
    }))
}
