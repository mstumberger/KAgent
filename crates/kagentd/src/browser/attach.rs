//! Attach to the owner's running browser (browser.md sections 1 and 3). Once remote
//! debugging is allowed, the browser writes `DevToolsActivePort` into its user data
//! directory. `BrowserSession` holds one browser-level WebSocket (`socket`) and
//! multiplexes the tabs with session ids; `tabs` says which tab a call acts in and
//! whose it is. Every new connection makes the browser ask "Allow", so it connects
//! lazily, pings every 10 s, and closes itself after ten idle minutes.

mod profile;
mod socket;
mod tabs;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use kagent_core::tool::ToolOutput;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;

pub use profile::{find_profile, parse_devtools_active_port};
use super::cdp;
use socket::Conn;
use super::target::{TabInfo, TabKind, TabTarget};
use tabs::AGENT_TABS_KEEP;

/// One connect attempt waits this long for the browser's "Allow".
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Without a call for this long the socket is closed.
pub const IDLE_CLOSE: Duration = Duration::from_secs(600);
const PING_EVERY: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

/// A page of the attached browser (`Target.getTargets`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub target_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
}

impl From<&TargetInfo> for cdp::Tab {
    fn from(t: &TargetInfo) -> Self {
        Self { id: t.target_id.clone(), kind: t.kind.clone(), title: t.title.clone(), url: t.url.clone(), web_socket_debugger_url: String::new() }
    }
}

/// `attach` of `GET /api/browser/status` (api.md section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachStatus {
    /// The profile folder; empty when none was found.
    pub profile: String,
    /// The browser's product name (its name from the profile folder until it is connected).
    pub browser: String,
    pub port: Option<u16>,
    /// `on`: the browser wrote `DevToolsActivePort`; `off`: it did not; `unknown`: no profile folder.
    pub debugging: &'static str,
    pub connected: bool,
    pub tabs: usize,
    pub detail: String,
}

/// What the owner does once in the browser (browser.md section 6).
pub fn instruction(browser: &str) -> String {
    format!("In {browser}, open chrome://inspect/#remote-debugging and tick 'Allow remote debugging for this browser instance'.")
}

/// Why a connect attempt did not connect.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// No profile folder: nothing to connect to.
    NoProfile,
    /// The browser asked "Allow" and nobody answered in time.
    Waiting,
    /// The profile folder has no `DevToolsActivePort`.
    Off,
    Other(String),
}

impl Failure {
    /// For the status card.
    fn detail(&self, name: &str) -> String {
        match self {
            Self::NoProfile => "no browser profile found; set the profile folder".into(),
            Self::Waiting => "waiting for Allow in the browser".into(),
            Self::Off => format!("{name} is not running, or remote debugging is off"),
            Self::Other(e) => e.clone(),
        }
    }

    /// For the tool and the model: the reason, and what the owner can do.
    fn error(&self, name: &str) -> anyhow::Error {
        match self {
            Self::NoProfile => anyhow!("no browser profile found; set the profile folder in Settings > Browser"),
            Self::Waiting => anyhow!("the browser did not allow the connection within {} s: click Allow in {name}, then call again", CONNECT_TIMEOUT.as_secs()),
            Self::Off => anyhow!("{}\n{}", self.detail(name), instruction(name)),
            Self::Other(e) => anyhow!("{e}. {}", instruction(name)),
        }
    }
}

/// A tab the agent opened in this daemon run.
struct AgentTab {
    id: String,
    title: String,
    url: String,
    used: Instant,
    /// In the last page list the browser gave.
    seen: bool,
}

#[derive(Default)]
struct Inner {
    conn: Option<Arc<Conn>>,
    port: Option<u16>,
    product: Option<String>,
    tabs: usize,
    last_used: Option<Instant>,
    /// The last attempt that did not connect, and when.
    failed: Option<(Instant, Failure)>,
    agent_tabs: VecDeque<AgentTab>,
    assessed: VecDeque<(String, TabKind)>,
}

/// The connection to the owner's running browser, shared by the tool and its routes.
pub struct BrowserSession {
    inner: Arc<Mutex<Inner>>,
    /// The profile folder and the browser's name; `None`: no profile folder. It may be
    /// edited while the daemon runs (`set_profile`).
    profile: Mutex<Option<(PathBuf, String)>>,
    /// One connect attempt at a time.
    connecting: tokio::sync::Mutex<()>,
    idle: Duration,
    ping: Duration,
    watcher: std::sync::OnceLock<()>,
}


impl BrowserSession {

    pub fn new(profile: Option<(PathBuf, String)>) -> Self {
        Self::with_timers(profile, IDLE_CLOSE, PING_EVERY)
    }

    fn with_timers(profile: Option<(PathBuf, String)>, idle: Duration, ping: Duration) -> Self {
        Self { inner: Arc::default(), profile: Mutex::new(profile), connecting: tokio::sync::Mutex::new(()), idle, ping, watcher: std::sync::OnceLock::new() }
    }

    /// The session of the running daemon: the profile folder of `settings.browser.attachProfile`.
    pub fn detected(configured: &str) -> Self {
        Self::new(Self::locate(configured))
    }

    /// The profile folder `settings.browser.attachProfile` names now: the folder itself
    /// (it must exist), else the first of the four browsers that is installed.
    pub fn locate(configured: &str) -> Option<(PathBuf, String)> {
        match (dirs::home_dir(), dirs::config_dir()) {
            (Some(home), Some(config)) => find_profile(&home, &config, configured),
            _ => None,
        }
    }

    /// The folder was edited: the socket to the old browser is closed, and the status
    /// is that of the new one from now on. (The backend and the trust are read at start.)
    pub async fn set_profile(&self, profile: Option<(PathBuf, String)>) {
        let changed = {
            let mut current = self.profile.lock().unwrap();
            let changed = *current != profile;
            *current = profile;
            changed
        };
        if !changed {
            return;
        }
        self.disconnect().await;
        let mut inner = self.inner.lock().unwrap();
        (inner.port, inner.product, inner.failed) = (None, None, None);
    }

    /// The browser's product name, or its name until it is connected.
    fn name(&self) -> String {
        let inner = self.inner.lock().unwrap();
        inner.product.clone().or_else(|| self.profile.lock().unwrap().as_ref().map(|(_, n)| n.clone())).unwrap_or_else(|| "unknown browser".into())
    }

    /// The first line of every output in attach mode (browser.md section 4).
    pub fn line(&self) -> String {
        format!("Browser: {} (the owner's own window)", self.name())
    }

    /// `attach` of the status route, and of the `status` action. Reads the port file; nothing else.
    pub fn status(&self) -> AttachStatus {
        let Some((dir, name)) = self.profile.lock().unwrap().clone() else {
            return AttachStatus { profile: String::new(), browser: String::new(), port: None, debugging: "unknown", connected: false, tabs: 0, detail: "no browser profile found; set the profile folder".into() };
        };
        let inner = self.inner.lock().unwrap();
        let connected = inner.conn.as_ref().is_some_and(|c| c.is_open());
        let file = std::fs::read_to_string(dir.join("DevToolsActivePort")).ok().and_then(|t| parse_devtools_active_port(&t));
        let debugging = if connected || file.is_some() { "on" } else { "off" };
        let detail = match (connected, debugging, &inner.failed) {
            (true, ..) => format!("{} tab{}", inner.tabs, if inner.tabs == 1 { "" } else { "s" }),
            (_, "off", _) => format!("{name} is not running, or remote debugging is off"),
            (_, _, Some((_, f))) => f.detail(&name),
            _ => "waiting for Allow in the browser".to_string(),
        };
        AttachStatus {
            profile: dir.display().to_string(),
            browser: inner.product.clone().unwrap_or(name),
            port: inner.port.or_else(|| file.map(|(p, _)| p)),
            debugging,
            connected,
            tabs: if connected { inner.tabs } else { 0 },
            detail,
        }
    }

    /// The `status` action in attach mode. An error result when the browser cannot be
    /// attached: the model then tells the owner what to do.
    pub fn status_output(&self) -> ToolOutput {
        let st = self.status();
        let line = self.line();
        if st.connected {
            return ToolOutput::ok(format!("{line}\nattached on port {}; {} tabs. Only tabs the agent opened can be closed.", st.port.unwrap_or(0), st.tabs));
        }
        let why = match st.debugging {
            "unknown" => return ToolOutput::error(format!("{line}\nno browser profile found; set the profile folder in Settings > Browser")),
            "off" => return ToolOutput::error(format!("{line}\n{}\n{}", instruction(&st.browser), st.detail)),
            _ => st.detail,
        };
        ToolOutput::ok(format!("{line}\nnot connected yet ({why}): the next action connects, and the browser asks Allow"))
    }

    /// The browser's pages: type `page`, without extensions.
    pub async fn pages(&self) -> Result<Vec<TargetInfo>> {
        let conn = self.conn().await?;
        let r = conn.request(None, "Target.getTargets", json!({}), CALL_TIMEOUT).await?;
        let infos: Vec<TargetInfo> = serde_json::from_value(r.get("targetInfos").cloned().unwrap_or(json!([]))).unwrap_or_default();
        let pages: Vec<TargetInfo> = infos.into_iter().filter(|t| t.kind == "page" && !t.url.starts_with("chrome-extension://")).collect();
        let mut inner = self.inner.lock().unwrap();
        inner.tabs = pages.len();
        for t in &mut inner.agent_tabs {
            t.seen = pages.iter().any(|p| p.target_id == t.id);
        }
        Ok(pages)
    }

    pub async fn attach(&self, target: &TargetInfo) -> Result<AttachedTab> {
        let conn = self.conn().await?;
        let r = conn.request(None, "Target.attachToTarget", json!({"targetId": target.target_id, "flatten": true}), CALL_TIMEOUT).await?;
        let session_id = r.get("sessionId").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("the browser gave no session for tab {}", target.target_id))?.to_string();
        let events = conn.listen(&session_id);
        Ok(AttachedTab {
            conn,
            session_id,
            events,
            info: TabInfo { id: target.target_id.clone(), title: target.title.clone(), url: target.url.clone() },
        })
    }

    /// A new tab. The agent opened it, so it may close it.
    pub async fn create_target(&self, url: &str) -> Result<TargetInfo> {
        let conn = self.conn().await?;
        let r = conn.request(None, "Target.createTarget", json!({"url": url}), CALL_TIMEOUT).await?;
        let id = r.get("targetId").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("the browser gave no id for the new tab"))?.to_string();
        let mut inner = self.inner.lock().unwrap();
        inner.agent_tabs.push_front(AgentTab { id: id.clone(), title: String::new(), url: url.into(), used: Instant::now(), seen: true });
        inner.tabs += 1;
        while inner.agent_tabs.len() > AGENT_TABS_KEEP {
            inner.agent_tabs.pop_back();
        }
        Ok(TargetInfo { target_id: id, kind: "page".into(), title: String::new(), url: url.into() })
    }

    pub async fn close_target(&self, id: &str) -> Result<()> {
        let conn = self.conn().await?;
        let r = conn.request(None, "Target.closeTarget", json!({"targetId": id}), CALL_TIMEOUT).await?;
        if r.get("success").and_then(Value::as_bool) == Some(false) {
            bail!("the browser did not close tab {id}");
        }
        let mut inner = self.inner.lock().unwrap();
        inner.tabs = inner.tabs.saturating_sub(1);
        inner.agent_tabs.iter_mut().find(|t| t.id == id).map(|t| t.seen = false);
        Ok(())
    }

    pub async fn activate(&self, id: &str) -> Result<()> {
        let conn = self.conn().await?;
        conn.request(None, "Target.activateTarget", json!({"targetId": id}), CALL_TIMEOUT).await?;
        Ok(())
    }
}

/// One tab of the attached browser: its commands carry the session id.
pub struct AttachedTab {
    conn: Arc<Conn>,
    session_id: String,
    info: TabInfo,
    events: mpsc::UnboundedReceiver<Value>,
}

impl AttachedTab {
    /// Release the tab when an action is done (its session ends).
    pub async fn detach(mut self) {
        self.events.close();
        self.forget();
        let _ = self.conn.request(None, "Target.detachFromTarget", json!({"sessionId": self.session_id}), CALL_TIMEOUT).await;
    }

    fn forget(&self) {
        self.conn.forget(&self.session_id);
    }
}

impl Drop for AttachedTab {
    fn drop(&mut self) {
        self.forget();
    }
}

#[async_trait]
impl TabTarget for AttachedTab {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.conn.request(Some(&self.session_id), method, params, CALL_TIMEOUT).await
    }

    async fn events(&mut self, listen: Duration) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + listen;
        let mut out = Vec::new();
        while let Ok(Some(v)) = tokio::time::timeout_at(deadline, self.events.recv()).await {
            out.push(v);
        }
        out
    }

    fn info(&self) -> TabInfo {
        self.info.clone()
    }
}

