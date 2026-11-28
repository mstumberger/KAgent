//! The routes of the reader (web.md section 9, api.md): its state, the window for a human
//! check, and forgetting. The work is in `Reader`; the handlers parse and answer.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::manage::ManageError;
use super::Reader;
use crate::server::{bad_request, ApiResult, HttpError};
use crate::web::Web;
use crate::AppState;

fn reader(web: &Web) -> ApiResult<&Arc<Reader>> {
    web.reader_admin.as_ref().ok_or_else(|| HttpError(StatusCode::NOT_FOUND, "not_found", "this daemon has no reader".into()))
}

fn refused(error: ManageError) -> HttpError {
    match error {
        ManageError::Busy(why) => HttpError(StatusCode::CONFLICT, "busy", why.into()),
        ManageError::NoBrowser => HttpError(StatusCode::CONFLICT, "no_browser", error.to_string()),
        ManageError::Bad(why) => bad_request(why),
        ManageError::Failed(why) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", why),
    }
}

/// `GET /api/web/reader`.
pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(Json(status(&st.web).await?).into_response())
}

/// `POST /api/web/reader/open` `{url}`: 200 `{host}`; 409 while a page is read or a window is open.
pub async fn open(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok(Json(open_window(&st.web, body).await?).into_response())
}

/// `DELETE /api/web/reader/sites/{host}`: 204.
pub async fn forget(State(st): State<Arc<AppState>>, Path(host): Path<String>) -> ApiResult<Response> {
    reader(&st.web)?.forget(&host).await.map_err(refused)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `DELETE /api/web/reader/profile`: 204.
pub async fn clear(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    reader(&st.web)?.clear_profile().await.map_err(refused)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn status(web: &Web) -> ApiResult<Value> {
    let status = reader(web)?.status().await;
    let mut value = serde_json::to_value(status).unwrap_or_default();
    value["enabled"] = json!(web.web_config().reader);
    Ok(value)
}

pub async fn open_window(web: &Web, body: Value) -> ApiResult<Value> {
    let url = body.get("url").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let host = reader(web)?.open_window(&url).await.map_err(refused)?;
    Ok(json!({ "host": host }))
}
