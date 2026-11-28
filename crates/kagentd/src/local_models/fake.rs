//! A fake Ollama for the tests: one loopback server that answers `/api/version`, `/api/tags`,
//! `/api/pull` (a stream of the lines a test scripts) and `/api/delete`, and keeps a log of
//! the requests. No test reaches the real Ollama: nothing is ever pulled for real.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::Notify;

/// One step of a scripted pull stream.
#[derive(Clone)]
pub enum Step {
    /// A line of the stream.
    Line(Value),
    Wait(Duration),
    /// Wait until the test calls `release()`.
    Gate,
    /// The model is now installed (as `GET /api/tags` lists it).
    Install(u64),
    /// The model is installed under another name (`nomic-embed-text` is `nomic-embed-text:latest`).
    InstallAs(String, u64),
    /// Keep the stream open until the client goes away.
    Hang,
}

#[derive(Default)]
pub struct State_ {
    pub models: Vec<(String, u64)>,
    pub scripts: HashMap<String, Vec<Step>>,
    pub seen: Vec<(String, String, String)>,
    pub starts: Vec<String>,
    pub dropped: Vec<String>,
}

type Shared = Arc<Mutex<State_>>;

#[derive(Clone)]
struct Ctx {
    state: Shared,
    gate: Arc<Notify>,
}

pub struct FakeOllama {
    pub base: String,
    pub state: Shared,
    gate: Arc<Notify>,
}

/// `pulling manifest`, a layer's progress, `success`: what Ollama's stream looks like.
pub fn happy(layers: &[(&str, u64)], chunks: u64) -> Vec<Step> {
    let mut steps = vec![Step::Line(json!({"status": "pulling manifest"}))];
    for c in 1..=chunks {
        for (digest, total) in layers {
            steps.push(Step::Line(json!({"status": format!("pulling {digest}"), "digest": format!("sha256:{digest}"), "total": total, "completed": total * c / chunks})));
        }
    }
    steps.push(Step::Line(json!({"status": "verifying sha256 digest"})));
    steps.push(Step::Install(layers.iter().map(|l| l.1).sum()));
    steps.push(Step::Line(json!({"status": "success"})));
    steps
}

impl FakeOllama {
    pub async fn start() -> Self {
        let state: Shared = Arc::new(Mutex::new(State_::default()));
        let gate = Arc::new(Notify::new());
        let ctx = Ctx { state: state.clone(), gate: gate.clone() };
        let app = Router::new()
            .route("/api/version", get(version))
            .route("/api/tags", get(tags))
            .route("/api/pull", post(pull))
            .route("/api/delete", delete(remove))
            .with_state(ctx);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        Self { base, state, gate }
    }

    pub fn install(&self, id: &str, bytes: u64) {
        self.state.lock().unwrap().models.push((id.into(), bytes));
    }

    pub fn script(&self, id: &str, steps: Vec<Step>) {
        self.state.lock().unwrap().scripts.insert(id.into(), steps);
    }

    pub fn release(&self) {
        self.gate.notify_one();
    }

    pub fn starts(&self) -> Vec<String> {
        self.state.lock().unwrap().starts.clone()
    }

    pub fn dropped(&self) -> Vec<String> {
        self.state.lock().unwrap().dropped.clone()
    }

    pub fn has(&self, id: &str) -> bool {
        self.state.lock().unwrap().models.iter().any(|m| m.0 == id)
    }

    pub fn seen_at(&self, path: &str) -> Vec<(String, String, String)> {
        self.state.lock().unwrap().seen.iter().filter(|s| s.1 == path).cloned().collect()
    }
}

async fn version() -> Json<Value> {
    Json(json!({"version": "0.9.1"}))
}

async fn tags(State(c): State<Ctx>) -> Json<Value> {
    let s = c.state.lock().unwrap();
    Json(json!({"models": s.models.iter().map(|(n, b)| json!({"name": n, "model": n, "size": b})).collect::<Vec<_>>()}))
}

async fn remove(State(c): State<Ctx>, Json(body): Json<Value>) -> Response {
    let id = body["model"].as_str().unwrap_or("").to_string();
    let mut s = c.state.lock().unwrap();
    s.seen.push(("DELETE".into(), "/api/delete".into(), body.to_string()));
    let before = s.models.len();
    s.models.retain(|m| m.0 != id);
    if s.models.len() == before {
        return (StatusCode::NOT_FOUND, Json(json!({"error": format!("model '{id}' not found")}))).into_response();
    }
    StatusCode::OK.into_response()
}

/// Marks the stream's end for the log when the client closes it.
struct Dropped(Shared, String);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.lock().unwrap().dropped.push(self.1.clone());
    }
}

async fn pull(State(c): State<Ctx>, Json(body): Json<Value>) -> Response {
    let id = body["model"].as_str().or(body["name"].as_str()).unwrap_or("").to_string();
    let steps = {
        let mut s = c.state.lock().unwrap();
        s.seen.push(("POST".into(), "/api/pull".into(), body.to_string()));
        s.starts.push(id.clone());
        s.scripts.get(&id).cloned()
    };
    let Some(steps) = steps else {
        let line = json!({"error": format!("pull model manifest: file does not exist: {id}")}).to_string();
        return Body::from(format!("{line}\n")).into_response();
    };
    let guard = Dropped(c.state.clone(), id.clone());
    let stream = futures::stream::unfold((0usize, steps, c, id, guard), |(i, steps, c, id, guard)| async move {
        let mut i = i;
        while i < steps.len() {
            let step = steps[i].clone();
            i += 1;
            match step {
                Step::Line(v) => return Some((Ok::<_, std::io::Error>(format!("{v}\n").into_bytes()), (i, steps, c, id, guard))),
                Step::Wait(d) => tokio::time::sleep(d).await,
                Step::Gate => c.gate.notified().await,
                Step::Install(bytes) => c.state.lock().unwrap().models.push((id.clone(), bytes)),
                Step::InstallAs(name, bytes) => c.state.lock().unwrap().models.push((name, bytes)),
                Step::Hang => std::future::pending::<()>().await,
            }
        }
        None
    });
    Body::from_stream(stream).into_response()
}
