//! `POST /api/voice/listen-start {marker}` (voice.md section 14.3).

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Body {
    marker: String,
}

/// `{start: true}` once for the marker the daemon made; `{start: false}` for anything else (a stale link is not an error).
pub async fn listen_start(State(st): State<Arc<AppState>>, Json(body): Json<Body>) -> Json<Value> {
    let start = st.listen_start.consume(&body.marker, Instant::now());
    if start {
        tracing::info!("listen on start: the page presented the marker, the session starts");
    }
    Json(json!({"start": start}))
}
