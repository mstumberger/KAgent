//! The browser routes (api.md section 6): the status of both backends, and the
//! connection to the owner's running browser (browser.md section 6).

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use super::attach::{AttachStatus, BrowserSession};
use super::{cdp, Browser};
use crate::sections::BrowserSection;
use crate::AppState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnStatus {
    running: bool,
    port: u16,
    tabs: usize,
}

/// The answer of `GET /api/browser/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    /// The backend the daemon runs with; a change of the setting applies after a restart.
    backend: &'static str,
    own: OwnStatus,
    attach: AttachStatus,
}

impl Browser {
    /// The status of both backends. KAgent's own window is asked whether it runs; the
    /// owner's browser is not contacted: its port file is read, nothing else.
    pub async fn status_json(&self) -> Json<BrowserStatus> {
        let running = cdp::version(self.port).await.is_ok();
        let tabs = if running { cdp::tabs(self.port).await.map(|t| t.len()).unwrap_or(0) } else { 0 };
        Json(BrowserStatus { backend: self.backend_name(), own: OwnStatus { running, port: self.port, tabs }, attach: self.attach.status() })
    }

    /// One connect attempt, which waits for the browser's "Allow", then the status.
    pub async fn attach_connect(&self) -> Json<BrowserStatus> {
        if self.attach.connect().await.is_ok() {
            let _ = self.attach.pages().await;
        }
        self.status_json().await
    }

    pub async fn attach_disconnect(&self) {
        self.attach.disconnect().await;
    }

    async fn attach_set_profile(&self, profile: Option<(std::path::PathBuf, String)>) {
        self.attach.set_profile(profile).await;
    }
}

/// The profile folder as it is saved now: an edit applies at once, to the status and to
/// the next connection. (The backend and the trust level apply after a restart.)
async fn sync_profile(st: &AppState) {
    let configured = st.settings.read().unwrap().section::<BrowserSection>("browser").map(|b| b.attach_profile).unwrap_or_default();
    st.browser.attach_set_profile(BrowserSession::locate(&configured)).await;
}

pub async fn status(State(st): State<Arc<AppState>>) -> Response {
    sync_profile(&st).await;
    st.browser.status_json().await.into_response()
}

pub async fn connect(State(st): State<Arc<AppState>>) -> Response {
    sync_profile(&st).await;
    st.browser.attach_connect().await.into_response()
}

pub async fn disconnect(State(st): State<Arc<AppState>>) -> Response {
    sync_profile(&st).await;
    st.browser.attach_disconnect().await;
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::attach::BrowserSession;
    use crate::sections::AttachTrust;

    #[tokio::test]
    async fn the_status_of_a_browser_that_is_off() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("Brave-Browser");
        std::fs::create_dir(&profile).unwrap();
        let b = Browser::attach(dir.path(), Arc::new(BrowserSession::new(Some((profile.clone(), "Brave".into())))), AttachTrust::Strict);
        let st = b.status_json().await.0;
        assert_eq!(st.backend, "attach");
        assert_eq!((st.own.running, st.own.port, st.own.tabs), (false, 9333, 0), "a closed port is a window that does not run");
        assert_eq!(
            st.attach,
            AttachStatus {
                profile: profile.display().to_string(),
                browser: "Brave".into(),
                port: None,
                debugging: "off",
                connected: false,
                tabs: 0,
                detail: "Brave is not running, or remote debugging is off".into()
            }
        );
        std::fs::write(profile.join("DevToolsActivePort"), "45678\n/devtools/browser/abc\n").unwrap();
        let st = b.status_json().await.0;
        assert_eq!((st.attach.debugging, st.attach.port, st.attach.connected, st.attach.detail.as_str()), ("on", Some(45678), false, "waiting for Allow in the browser"), "{:?}", st.attach);
        // Nothing listens on that port: one attempt fails, and the status says why.
        let st = b.attach_connect().await.0;
        assert!(!st.attach.connected && st.attach.debugging == "on", "{:?}", st.attach);
        assert!(st.attach.detail.contains("did not accept the connection"), "{}", st.attach.detail);
        b.attach_disconnect().await;
        let none = Browser::attach(dir.path(), Arc::new(BrowserSession::new(None)), AttachTrust::Strict).status_json().await.0;
        assert_eq!((none.attach.debugging, none.attach.detail.as_str()), ("unknown", "no browser profile found; set the profile folder"));
    }
}
