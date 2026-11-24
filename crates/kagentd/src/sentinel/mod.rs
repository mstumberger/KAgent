//! Intrusion detection on this PC (S14): honeytokens, persistence watch, heartbeat.

pub mod honeytokens;
pub mod persistence;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::watch::AlertManager;

pub struct Sentinel {
    data: PathBuf,
    home: PathBuf,
    alerts: Arc<AlertManager>,
    honey_cancel: Mutex<Option<CancellationToken>>,
}

impl Sentinel {
    pub fn new(data: PathBuf, alerts: Arc<AlertManager>) -> Arc<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        Arc::new(Self { data, home, alerts, honey_cancel: Mutex::new(None) })
    }

    pub fn start(self: &Arc<Self>, cancel: CancellationToken, persistence: bool) {
        if persistence {
            let (d, h, a, c) = (self.data.clone(), self.home.clone(), self.alerts.clone(), cancel.clone());
            tokio::spawn(async move {
                if let Err(e) = persistence::watch(d, h, a, c).await {
                    tracing::warn!("persistence watch stopped: {e:#}");
                }
            });
        }
        if !honeytokens::load(&self.data).is_empty() {
            self.start_honeytokens();
        }
        // Heartbeat for the external "KAgent went silent" timer (scripts/install-heartbeat.sh).
        let hb = self.data.join("heartbeat");
        tokio::spawn(async move {
            loop {
                let _ = std::fs::write(&hb, chrono::Utc::now().to_rfc3339());
                tokio::select! { _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {}, _ = cancel.cancelled() => break }
            }
        });
    }

    fn start_honeytokens(self: &Arc<Self>) {
        let cancel = CancellationToken::new();
        if let Some(old) = self.honey_cancel.lock().unwrap().replace(cancel.clone()) {
            old.cancel();
        }
        let this = self.clone();
        tokio::spawn(async move {
            // Restart after a token is moved/deleted (watch bails) until cancelled.
            while !cancel.is_cancelled() {
                let tokens = honeytokens::load(&this.data);
                if let Err(e) = honeytokens::watch(tokens, this.alerts.clone(), cancel.clone()).await {
                    tracing::warn!("honeytoken watch: {e:#}");
                }
                tokio::select! { _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}, _ = cancel.cancelled() => break }
            }
        });
    }

    pub fn set_honeytokens(self: &Arc<Self>, enable: bool) -> anyhow::Result<Vec<honeytokens::Token>> {
        if enable {
            let t = honeytokens::enable(&self.data, &self.home)?;
            self.start_honeytokens();
            Ok(t)
        } else {
            if let Some(c) = self.honey_cancel.lock().unwrap().take() {
                c.cancel();
            }
            honeytokens::disable(&self.data)?;
            Ok(vec![])
        }
    }

    pub fn tokens(&self) -> Vec<honeytokens::Token> {
        honeytokens::load(&self.data)
    }

    pub fn rebaseline(&self) -> anyhow::Result<usize> {
        persistence::rebaseline(&self.data, &self.home)
    }
}

pub mod http {
    use std::sync::Arc;

    use axum::extract::State;
    use axum::response::{IntoResponse, Response};
    use axum::Json;
    use serde_json::{json, Value};

    use crate::AppState;

    fn err(e: anyhow::Error) -> Response {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"code": "internal", "message": format!("{e:#}")}}))).into_response()
    }

    pub async fn status(State(st): State<Arc<AppState>>) -> Response {
        Json(json!({"honeytokens": st.sentinel.tokens(), "baseline": st.sentinel.data.join("sentinel/baseline.json").exists()})).into_response()
    }

    pub async fn honeytokens(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
        let enable = body.get("enable").and_then(Value::as_bool).unwrap_or(false);
        match st.sentinel.set_honeytokens(enable) {
            Ok(t) => Json(json!({"tokens": t, "note": "exclude these from Baloo: System Settings -> Search -> File Search -> exclude ~/.aws, ~/.ssh/id_backup, ~/.config/backup.env, ~/Documents/passwords-old.kdbx"})).into_response(),
            Err(e) => err(e),
        }
    }

    pub async fn rebaseline(State(st): State<Arc<AppState>>) -> Response {
        match st.sentinel.rebaseline() {
            Ok(n) => Json(json!({"takenAt": chrono::Utc::now(), "items": n})).into_response(),
            Err(e) => err(e),
        }
    }
}
