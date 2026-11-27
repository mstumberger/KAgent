//! A fake Google for the tests (as `bots/fakes.rs` is a fake Telegram): one loopback server
//! that answers the sign-in, the token calls, the revoke, the Calendar API, Places and Routes
//! the way Google's documentation says, and keeps a log of every request. No test reaches a
//! real Google host: `Hosts::loopback` is the only way a test builds a client.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axum::extract::{Form, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::Hosts;

pub const CLIENT_ID: &str = "1234-test.apps.googleusercontent.com";
pub const CLIENT_SECRET: &str = "GOCSPX-fake-client-secret-0001";
pub const REFRESH_TOKEN: &str = "1//fake-refresh-token-0001";

/// One request the fake saw.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: String,
    pub bearer: Option<String>,
    /// `X-Goog-Api-Key`, the key of Places and Routes.
    pub key: Option<String>,
    pub field_mask: Option<String>,
}

impl Seen {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

#[derive(Default)]
pub struct Fake {
    /// code -> the PKCE challenge it was authorised with.
    pub codes: HashMap<String, String>,
    pub access: HashSet<String>,
    /// Whether the refresh token is still good (false: `invalid_grant`).
    pub refresh_ok: bool,
    /// The next access tokens are not issued (`error` of the token endpoint), when set.
    pub token_error: Option<String>,
    pub issued: u32,
    pub revoked: Vec<String>,
    pub seen: Vec<Seen>,
}

pub type Shared = Arc<Mutex<Fake>>;

pub struct FakeGoogle {
    pub base: String,
    pub state: Shared,
}

impl FakeGoogle {
    pub async fn start() -> Self {
        let state: Shared = Arc::new(Mutex::new(Fake { refresh_ok: true, ..Default::default() }));
        let app = Router::new()
            .route("/token", post(token))
            .route("/revoke", post(revoke))
            .layer(middleware::from_fn_with_state(state.clone(), log))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Self { base, state }
    }

    pub fn hosts(&self) -> Hosts {
        Hosts::loopback(&self.base)
    }

    /// The owner approves at Google: a code for this challenge.
    pub fn authorize(&self, challenge: &str) -> String {
        let mut f = self.state.lock().unwrap();
        f.issued += 1;
        let code = format!("4/fake-code-{}", f.issued);
        f.codes.insert(code.clone(), challenge.to_string());
        code
    }

    /// Every access token stops working (the calls answer 401 until a refresh).
    pub fn expire_access_tokens(&self) {
        self.state.lock().unwrap().access.clear();
    }

    pub fn revoke_grant(&self) {
        self.state.lock().unwrap().refresh_ok = false;
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    /// The requests whose path starts with `prefix`.
    pub fn seen_at(&self, prefix: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.path.starts_with(prefix)).collect()
    }
}

/// The access token of a request, when the fake issued it and it still stands.
pub fn authorised(f: &Fake, bearer: &Option<String>) -> bool {
    bearer.as_ref().is_some_and(|t| f.access.contains(t))
}

async fn log(State(state): State<Shared>, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap_or_default();
    let header = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(String::from);
    state.lock().unwrap().seen.push(Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or_default().to_string(),
        body: String::from_utf8_lossy(&bytes).into_owned(),
        bearer: header("authorization").and_then(|h| h.strip_prefix("Bearer ").map(String::from)),
        key: header("x-goog-api-key"),
        field_mask: header("x-goog-fieldmask"),
    });
    next.run(Request::from_parts(parts, axum::body::Body::from(bytes))).await
}

fn error(status: StatusCode, error: &str, description: &str) -> Response {
    (status, Json(json!({"error": error, "error_description": description}))).into_response()
}

async fn token(State(state): State<Shared>, Form(form): Form<HashMap<String, String>>) -> Response {
    let mut f = state.lock().unwrap();
    if let Some(e) = f.token_error.clone() {
        return error(StatusCode::BAD_REQUEST, &e, "refused on purpose");
    }
    if form.get("client_id").map(String::as_str) != Some(CLIENT_ID) || form.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET) {
        return error(StatusCode::UNAUTHORIZED, "invalid_client", "The OAuth client was not found.");
    }
    f.issued += 1;
    let access = format!("ya29.fake-access-{}", f.issued);
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let (Some(code), Some(verifier)) = (form.get("code"), form.get("code_verifier")) else {
                return error(StatusCode::BAD_REQUEST, "invalid_request", "code and code_verifier are required");
            };
            if form.get("redirect_uri").is_none_or(|r| !r.starts_with("http://127.0.0.1")) {
                return error(StatusCode::BAD_REQUEST, "redirect_uri_mismatch", "the redirect is not the loopback one");
            }
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            // A code is used once, and only with the verifier of its challenge.
            match f.codes.remove(code) {
                Some(c) if c == challenge => {}
                _ => return error(StatusCode::BAD_REQUEST, "invalid_grant", "Bad Request"),
            }
            f.access.insert(access.clone());
            f.refresh_ok = true;
            Json(json!({"access_token": access, "expires_in": 3599, "refresh_token": REFRESH_TOKEN, "scope": "calendar", "token_type": "Bearer"})).into_response()
        }
        Some("refresh_token") => {
            if form.get("refresh_token").map(String::as_str) != Some(REFRESH_TOKEN) || !f.refresh_ok {
                return error(StatusCode::BAD_REQUEST, "invalid_grant", "Token has been expired or revoked.");
            }
            f.access.insert(access.clone());
            Json(json!({"access_token": access, "expires_in": 3599, "token_type": "Bearer"})).into_response()
        }
        _ => error(StatusCode::BAD_REQUEST, "unsupported_grant_type", "no"),
    }
}

async fn revoke(State(state): State<Shared>, Form(form): Form<HashMap<String, String>>) -> Response {
    let mut f = state.lock().unwrap();
    f.revoked.push(form.get("token").cloned().unwrap_or_default());
    f.refresh_ok = false;
    StatusCode::OK.into_response()
}
