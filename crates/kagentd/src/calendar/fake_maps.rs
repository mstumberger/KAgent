//! Places (New) and Routes of the fake Google (`fake.rs`): `searchText` and `computeRoutes`
//! with the key in `X-Goog-Api-Key`, the field mask in `X-Goog-FieldMask` (both are in the
//! request log), and a key that is not the fake's one refused with 403.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};

use super::fake::{Shared, MAPS_KEY};

pub fn routes() -> Router<Shared> {
    Router::new().route("/v1/places:searchText", post(search)).route("/directions/v2:computeRoutes", post(compute))
}

fn gate(state: &Shared, headers: &HeaderMap) -> Option<Response> {
    let down = state.lock().unwrap().maps_down;
    if down {
        return Some((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"code": 500, "message": "backend error"}}))).into_response());
    }
    let key = headers.get("x-goog-api-key").and_then(|v| v.to_str().ok());
    // Google's own message may repeat the key it was sent: the client must not pass that on.
    (key != Some(MAPS_KEY)).then(|| (StatusCode::FORBIDDEN, Json(json!({"error": {"code": 403, "message": format!("API key not valid: {}. Please pass a valid API key.", key.unwrap_or("")), "status": "PERMISSION_DENIED"}}))).into_response())
}

async fn search(State(state): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if let Some(refused) = gate(&state, &headers) {
        return refused;
    }
    let query = body["textQuery"].as_str().unwrap_or("").to_lowercase();
    let limit = body["maxResultCount"].as_u64().unwrap_or(3) as usize;
    let f = state.lock().unwrap();
    let found: Vec<&Value> = f
        .places
        .iter()
        .filter(|p| {
            let name = p["displayName"]["text"].as_str().unwrap_or("").to_lowercase();
            let address = p["formattedAddress"].as_str().unwrap_or("").to_lowercase();
            query.contains(&name) || name.contains(&query) || query.contains(&address) || address.contains(&query)
        })
        .take(limit)
        .collect();
    if found.is_empty() {
        return Json(json!({})).into_response();
    }
    Json(json!({"places": found})).into_response()
}

async fn compute(State(state): State<Shared>, headers: HeaderMap, Json(_body): Json<Value>) -> Response {
    if let Some(refused) = gate(&state, &headers) {
        return refused;
    }
    let (no_traffic, with_traffic) = state.lock().unwrap().drive;
    Json(json!({"routes": [{"duration": format!("{with_traffic}s"), "staticDuration": format!("{no_traffic}s")}]})).into_response()
}
