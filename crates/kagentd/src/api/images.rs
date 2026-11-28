//! The image routes (vision.md section 1, api.md section 3): upload one image to a session,
//! serve it back. The work is in functions over the session store, tested without a server
//! (`tests.rs`); the handlers wrap them.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::protocol::ImageRef;
use kagent_core::session::images::ImageError;
use kagent_core::session::SessionStore;

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

/// The request body of an upload may be a little over the 8 MB the image may have, so the
/// 413 below names the limit; the router's own 2 MB limit does not apply to this route.
pub const UPLOAD_BODY_BYTES: usize = 9 * 1024 * 1024;

/// The HTTP error of an image error.
pub fn http_error(e: ImageError) -> HttpError {
    let (status, code) = match &e {
        ImageError::TooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "too_large"),
        ImageError::UnsupportedType => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_type"),
        ImageError::Unreadable(_) | ImageError::TooMany(_) => (StatusCode::BAD_REQUEST, "invalid_input"),
        ImageError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
        ImageError::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
    };
    HttpError(status, code, e.to_string())
}

/// Store one upload in the session: the session must exist.
pub async fn upload(store: &SessionStore, session_id: &str, bytes: Vec<u8>) -> ApiResult<ImageRef> {
    store.meta(session_id).await.map_err(|e| not_found(format!("{e:#}")))?;
    store.put_image(session_id, bytes).await.map_err(http_error)
}

/// The references a message names by id, in order (400 over 4, 404 for an id the session does not have).
pub async fn message_images(store: &SessionStore, session_id: &str, ids: &[String]) -> ApiResult<Vec<ImageRef>> {
    store.image_refs(session_id, ids).await.map_err(http_error)
}

/// `images: [id]` of a message body: strings only.
pub fn image_ids(body: &serde_json::Value) -> ApiResult<Vec<String>> {
    match body.get("images") {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad_request("images must be a list of image ids"))).collect(),
        Some(_) => Err(bad_request("images must be a list of image ids")),
    }
}

/// `POST /api/sessions/:id/images`: one file in the multipart field `file`.
pub async fn post_image(State(st): State<Arc<AppState>>, Path(id): Path<String>, form: Multipart) -> ApiResult<Response> {
    handle_upload(st.agent.store(), &id, form).await
}

/// The body of `post_image` over a store, so a test can serve it without the daemon's state.
pub async fn handle_upload(store: &SessionStore, id: &str, mut form: Multipart) -> ApiResult<Response> {
    let field = loop {
        match form.next_field().await.map_err(|e| bad_request(format!("the upload could not be read ({e}): send one file in the field `file`")))? {
            Some(f) if f.name() == Some("file") => break f,
            Some(_) => continue,
            None => return Err(bad_request("no file: send the image in the multipart field `file`")),
        }
    };
    let bytes = field.bytes().await.map_err(|e| HttpError(StatusCode::PAYLOAD_TOO_LARGE, "too_large", format!("the upload was cut ({e}): the limit is 8 MB, send a smaller image")))?;
    let image = upload(store, id, bytes.to_vec()).await?;
    Ok((StatusCode::OK, Json(image)).into_response())
}

/// `GET /api/sessions/:id/images/:imageId`.
pub async fn get_image(State(st): State<Arc<AppState>>, Path((id, image_id)): Path<(String, String)>) -> ApiResult<Response> {
    let (bytes, mime) = st.agent.store().image_file(&id, &image_id).await.map_err(http_error)?;
    Ok(([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "private".to_string())], bytes).into_response())
}
