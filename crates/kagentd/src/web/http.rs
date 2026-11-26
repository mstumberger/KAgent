//! The route of the Brave key. api.md names none: a spec correction of S30. The work
//! is in three functions, tested without a server; the handlers parse and answer.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::settings::check_key;
use super::Web;
use crate::server::{bad_request, ApiResult, HttpError};
use crate::AppState;

fn ok_json(value: Value) -> Response {
    (StatusCode::OK, Json(value)).into_response()
}

/// `GET /api/web/brave-key`: whether a key is stored. Never the key.
pub async fn get_key(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(status(&st.web).await?))
}

/// `PUT /api/web/brave-key` `{key}`: the Secret Service, else the 0600 file of this home.
pub async fn put_key(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok(ok_json(put(&st.web, body).await?))
}

/// `DELETE /api/web/brave-key`: 204.
pub async fn delete_key(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    clear(&st.web).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Whether a key is stored.
pub async fn status(web: &Web) -> ApiResult<Value> {
    let configured = web.brave_key().await.map_err(internal)?.is_some();
    Ok(json!({ "configured": configured }))
}

/// Keep a key of the shape the store takes (400 otherwise).
pub async fn put(web: &Web, body: Value) -> ApiResult<Value> {
    let key = body.get("key").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    check_key(&key).map_err(bad_request)?;
    web.set_brave_key(&key).await.map_err(internal)?;
    Ok(json!({ "configured": true }))
}

/// Forget the key; forgetting none is not an error.
pub async fn clear(web: &Web) -> ApiResult<()> {
    web.clear_brave_key().await.map_err(internal)
}

fn internal(e: anyhow::Error) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::error_body;
    use crate::web::fake::MemoryKeys;
    use crate::web::guard::Unchecked;
    use crate::web::{Engines, Web};
    use std::sync::OnceLock;

    /// The answer of a call that had to succeed (`HttpError` is not `Debug`).
    fn ok<T>(result: ApiResult<T>) -> T {
        match result {
            Ok(value) => value,
            Err(e) => panic!("refused: {} {}", e.1, e.2),
        }
    }

    /// The refusal of a call that had to fail, with the body the client would read.
    fn refused<T>(result: ApiResult<T>) -> (StatusCode, String) {
        let error = result.err().expect("must be refused");
        (error.0, error_body(error.1, error.2))
    }

    async fn web() -> (Arc<Web>, Arc<MemoryKeys>) {
        let keys = Arc::new(MemoryKeys::default());
        (Arc::new(Web::with(Arc::new(OnceLock::new()), keys.clone(), Arc::new(Unchecked), Engines::default())), keys)
    }

    /// Only `configured` is ever answered, and a key of another shape is refused before the store sees it.
    #[tokio::test]
    async fn the_key_is_stored_cleared_and_never_shown() {
        let (web, keys) = web().await;
        assert_eq!(ok(status(&web).await), json!({ "configured": false }));
        assert_eq!(ok(put(&web, json!({ "key": "BSAtest-key_that_is_long_enough" })).await), json!({ "configured": true }));
        assert_eq!(keys.0.lock().unwrap().as_deref(), Some("BSAtest-key_that_is_long_enough"));
        let shown = ok(status(&web).await).to_string();
        assert_eq!(shown, "{\"configured\":true}");
        assert!(!shown.contains("BSAtest"), "the key is never in an answer");
        for body in [json!({ "key": "short" }), json!({ "key": "  " }), json!({ "key": 42 }), json!({}), json!({ "token": "BSAtest-key_that_is_long_enough" })] {
            assert_eq!(refused(put(&web, body.clone()).await).0, StatusCode::BAD_REQUEST, "{body}");
        }
        assert_eq!(keys.0.lock().unwrap().as_deref(), Some("BSAtest-key_that_is_long_enough"), "a refused key changes nothing");
        assert_eq!(ok(put(&web, json!({ "key": "  BSAtest-a_second_key_now_x \n" })).await), json!({ "configured": true }), "what surrounds the key is not part of it");
        assert_eq!(keys.0.lock().unwrap().as_deref(), Some("BSAtest-a_second_key_now_x"));
        ok(clear(&web).await);
        assert!(keys.0.lock().unwrap().is_none());
        assert_eq!(ok(status(&web).await), json!({ "configured": false }));
        ok(clear(&web).await);
    }
}
