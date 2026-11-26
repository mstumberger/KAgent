//! The telegram routes (api.md section 6). The work is in `Bots` (`mod.rs`), where it is
//! tested without a server: these handlers parse, answer, and name the status of a refusal.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::server::{bad_request, ApiResult, HttpError};
use crate::AppState;

fn ok_json(v: Value) -> Response {
    (StatusCode::OK, Json(v)).into_response()
}

/// `GET /api/telegram`: the settings, the token (`configured`, never the token itself),
/// the pairing, the last error and the state of the bot.
pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.bots.view()))
}

/// `PUT /api/telegram`: the five settings of bots.md section 1.
pub async fn put(State(st): State<Arc<AppState>>, Json(patch): Json<Value>) -> ApiResult<Response> {
    st.bots.patch(patch)?;
    Ok(ok_json(st.bots.view()))
}

/// `POST /api/telegram/token`: the token is kept only when Telegram knows it.
pub async fn set_token(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let token = body.get("token").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad_request("token required"))?;
    let username = st.bots.set_token(token).await?;
    Ok(ok_json(json!({ "botUsername": username })))
}

/// `DELETE /api/telegram/token`: 204; also unpairs.
pub async fn delete_token(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    st.bots.clear_token().await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/telegram/pairing-code`: six digits, ten minutes, one use.
pub async fn pairing_code(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.bots.pairing_code()?))
}

/// `DELETE /api/telegram/pairing`: 204; the chat is forgotten.
pub async fn unpair(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    st.bots.unpair().map_err(internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/telegram/test`: a message to the paired chat.
pub async fn test_message(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.bots.test_message().await))
}

fn internal(e: anyhow::Error) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}"))
}
