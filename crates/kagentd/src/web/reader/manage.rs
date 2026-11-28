//! What the owner does to the reader from Settings -> Web (web.md section 9): open a
//! window on its profile to pass a human check, forget a warmed site, clear the profile.

use std::sync::atomic::Ordering;

use serde::Serialize;

use super::sites::Warmed;
use super::{browser, drive, Reader};
use crate::web::guard;

/// Why a request of the owner was not done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManageError {
    /// A page is being read, or the window is open: 409.
    Busy(&'static str),
    /// No Chromium-family browser on this machine: 409.
    NoBrowser,
    /// The request is not valid: 400.
    Bad(String),
    Failed(String),
}

impl std::fmt::Display for ManageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(why) => write!(f, "{why}"),
            Self::NoBrowser => write!(f, "no Chromium-family browser was found (brave-browser, google-chrome, chromium)"),
            Self::Bad(why) | Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// `GET /api/web/reader`, without `enabled` (a setting).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub browser: Option<String>,
    pub running: bool,
    pub window_open: bool,
    pub profile: String,
    pub warmed: Vec<Warmed>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default()
}

impl Reader {
    pub async fn status(&self) -> Status {
        Status {
            browser: self.browser(),
            running: self.running().await,
            window_open: self.window.lock().unwrap().is_some(),
            profile: self.profile().display().to_string(),
            warmed: self.sites.list(),
        }
    }

    /// Open the reader's profile in a visible window on `url`. The owner passes the check or
    /// logs in and closes the window; the host is then on the warmed list. Refused while a
    /// page is being read or a window is open. The browser that read headless is closed first.
    pub async fn open_window(&self, url: &str) -> Result<String, ManageError> {
        let parsed = guard::check_url(url).map_err(|r| ManageError::Bad(r.to_string()))?;
        let host = guard::host_of(&parsed).unwrap_or_default();
        let bin = (self.finder)().ok_or(ManageError::NoBrowser)?;
        if self.window.lock().unwrap().is_some() {
            return Err(ManageError::Busy("the reader's window is already open"));
        }
        let mut state = self.state.try_lock().map_err(|_| ManageError::Busy("a page is being read: try again in a moment"))?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(running) = state.take() {
            running.close().await;
        }
        let mut child = browser::start_window(&bin, &self.profile(), parsed.as_str()).map_err(ManageError::Failed)?;
        *self.window.lock().unwrap() = Some(host.clone());
        let me = self.me.clone();
        let warmed = host.clone();
        tokio::spawn(async move {
            let _ = child.wait().await;
            if let Some(reader) = me.upgrade() {
                *reader.window.lock().unwrap() = None;
                if let Err(e) = reader.sites.add(&warmed, now_ms()) {
                    tracing::warn!("the warmed site {warmed} could not be saved: {e}");
                }
            }
        });
        Ok(host)
    }

    /// Forget a warmed site: its cookies leave the profile (through the browser) and the list.
    pub async fn forget(&self, host: &str) -> Result<(), ManageError> {
        if self.window.lock().unwrap().is_some() {
            return Err(ManageError::Busy("the reader's window is open: close it first"));
        }
        let mut state = self.state.try_lock().map_err(|_| ManageError::Busy("a page is being read: try again in a moment"))?;
        if self.profile().is_dir() {
            let bin = (self.finder)().ok_or(ManageError::NoBrowser)?;
            if !state.as_mut().is_some_and(browser::Running::alive) {
                *state = Some(browser::start_headless(&bin, &self.profile()).await.map_err(ManageError::Failed)?);
            }
            let port = state.as_ref().map(|r| r.port).unwrap_or_default();
            drive::forget_cookies(port, host).await.map_err(|e| ManageError::Failed(e.to_string()))?;
            let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
            self.arm_idle(generation);
        }
        self.sites.remove(host).map_err(|e| ManageError::Failed(e.to_string()))
    }

    /// Close the browser, delete the profile folder and empty the warmed list.
    pub async fn clear_profile(&self) -> Result<(), ManageError> {
        if self.window.lock().unwrap().is_some() {
            return Err(ManageError::Busy("the reader's window is open: close it first"));
        }
        let mut state = self.state.try_lock().map_err(|_| ManageError::Busy("a page is being read: try again in a moment"))?;
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(running) = state.take() {
            running.close().await;
        }
        match std::fs::remove_dir_all(self.profile()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(ManageError::Failed(format!("the profile could not be deleted: {e}"))),
            _ => {}
        }
        self.sites.clear().map_err(|e| ManageError::Failed(e.to_string()))
    }
}
