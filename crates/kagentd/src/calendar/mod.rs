//! Google Calendar as a module of the daemon (calendar.md): the owner's own OAuth client,
//! the events of the calendars switched on, places and the morning report. This file is the
//! hub: the connection state, the token (kept in the keyring, the access token in memory
//! only) and the settings it saves. `oauth.rs` speaks to Google's sign-in and `settings.rs`
//! holds the section.

pub mod change;
pub mod connection;
pub mod google;
pub mod http;
pub mod model;
pub mod oauth;
pub mod places;
pub mod report;
pub mod report_run;
pub mod report_tool;
pub mod plan;
pub mod settings;
pub mod state;
pub mod place_tool;
pub mod tools;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod fake_api;
#[cfg(test)]
mod fake_maps;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use kagent_core::protocol::AgentEvent;

use self::model::Zone;
use self::oauth::Pending;
use self::settings::CalendarSection;
use crate::secrets::TokenStore;
use crate::settings::Settings;

/// Where Google is asked (calendar.md section 5): fixed hosts; a test passes a loopback fake.
#[derive(Debug, Clone)]
pub struct Hosts {
    pub auth: String,
    pub token: String,
    pub revoke: String,
    pub api: String,
    pub places: String,
    pub routes: String,
}

impl Hosts {
    pub fn real() -> Self {
        Self {
            auth: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            revoke: "https://oauth2.googleapis.com/revoke".into(),
            api: "https://www.googleapis.com".into(),
            places: "https://places.googleapis.com".into(),
            routes: "https://routes.googleapis.com".into(),
        }
    }

    #[cfg(test)]
    pub fn loopback(base: &str) -> Self {
        Self { auth: format!("{base}/o/oauth2/v2/auth"), token: format!("{base}/token"), revoke: format!("{base}/revoke"), api: base.into(), places: base.into(), routes: base.into() }
    }
}

/// The redirect Google sends the owner back to (calendar.md section 1).
pub const REDIRECT: &str = "http://127.0.0.1:7878/api/calendar/oauth/callback";

/// The connection, as the page and the event name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnState {
    /// No refresh token.
    Off,
    /// The owner was sent to Google and the callback is awaited.
    Connecting,
    Connected,
    /// Google no longer accepts the token (`invalid_grant`): the owner connects again.
    Reconnect,
    /// The last call failed for another reason; `error` says why.
    Error,
}

impl ConnState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Reconnect => "reconnect",
            Self::Error => "error",
        }
    }
}

/// Why a call of the module did not give what was asked. Shown to the model and the page,
/// so it says what to do next, and never holds a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalError {
    /// The owner has to do something first (a client, a switch).
    Setup(String),
    /// Not connected, or the connection must be made again.
    NotConnected(String),
    /// Google answered with something other than success.
    Upstream(String),
    /// Refused by a rule of KAgent: nothing was sent to Google.
    Refused(String),
}

impl std::fmt::Display for CalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(m) | Self::NotConnected(m) | Self::Upstream(m) | Self::Refused(m) => write!(f, "{m}"),
        }
    }
}

const RECONNECT_TEXT: &str = "Google Calendar needs to be connected again: the owner opens Settings -> Calendar and presses Reconnect.";

/// The record in the keyring (or `<config>/calendar.json`): one JSON text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Secrets {
    client_secret: String,
    refresh_token: String,
    /// The address of the account (the id of its primary calendar).
    account: String,
}

/// What `Calendar` is built from. The daemon fills it in `Calendar::daemon`; a test with fakes.
pub struct Parts {
    pub settings: Arc<RwLock<Settings>>,
    /// Where the settings are saved after the module changed one.
    pub settings_path: PathBuf,
    pub secrets: TokenStore,
    pub hosts: Hosts,
    pub redirect: String,
    pub events: broadcast::Sender<AgentEvent>,
    pub clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    /// `<data>`: where the module keeps its small state.
    pub data_dir: PathBuf,
    /// The Maps key of the places lookup.
    pub maps: TokenStore,
    /// The web tools, for the place search without a map.
    pub web: Option<Arc<crate::web::Web>>,
    /// Where the Telegram report goes, and whether a chat is paired to receive it.
    pub notifier: Option<Arc<crate::notify::Notifier>>,
    pub paired: Arc<dyn Fn() -> bool + Send + Sync>,
    /// KAgent's own reminders and tasks due on a day (the report's last lines).
    pub tasks: Arc<dyn Fn(chrono::NaiveDate) -> Vec<report::TaskLine> + Send + Sync>,
    /// The machine's offset at an instant (the system's time zone; a test passes a fixed one).
    pub zone: Zone,
}

struct Inner {
    secrets: Secrets,
    /// The access token and when it ends: in memory only.
    access: Option<(String, DateTime<Utc>)>,
    pending: Pending,
    /// Set by `invalid_grant` or another failure; cleared by a connection.
    sticky: Option<(ConnState, String)>,
    /// One notice per break, not one per call.
    told_reconnect: bool,
}

/// The calendar module: one per process, behind the tools, the routes and the report.
pub struct Calendar {
    settings: Arc<RwLock<Settings>>,
    settings_path: PathBuf,
    store: TokenStore,
    pub(crate) hosts: Hosts,
    redirect: String,
    events: broadcast::Sender<AgentEvent>,
    pub(crate) clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    pub(crate) zone: Zone,
    pub(crate) data_dir: PathBuf,
    pub(crate) maps: TokenStore,
    pub(crate) maps_cache: Mutex<Option<String>>,
    pub(crate) web: Option<Arc<crate::web::Web>>,
    pub(crate) notifier: Option<Arc<crate::notify::Notifier>>,
    pub(crate) paired: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) tasks: Arc<dyn Fn(chrono::NaiveDate) -> Vec<report::TaskLine> + Send + Sync>,
    /// `state.rs`: where home was resolved to.
    pub(crate) saved: Mutex<state::Saved>,
    pub(crate) http: reqwest::Client,
    /// What was read before a question (`change.rs`).
    pub(crate) memo: Mutex<change::Memo>,
    inner: Mutex<Inner>,
    /// One refresh at a time.
    refreshing: tokio::sync::Mutex<()>,
}

impl Calendar {
    pub async fn new(parts: Parts) -> Arc<Self> {
        let secrets = match parts.secrets.get().await {
            Ok(Some(text)) => serde_json::from_str::<Secrets>(&text).unwrap_or_else(|_| {
                tracing::warn!("calendar: the stored connection could not be read; connect again");
                Secrets::default()
            }),
            Ok(None) => Secrets::default(),
            Err(e) => {
                tracing::warn!("calendar: the stored connection could not be read: {e:#}");
                Secrets::default()
            }
        };
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).user_agent(format!("KAgent/{}", env!("CARGO_PKG_VERSION"))).build().expect("an HTTP client");
        Arc::new(Self {
            settings: parts.settings,
            settings_path: parts.settings_path,
            store: parts.secrets,
            hosts: parts.hosts,
            redirect: parts.redirect,
            events: parts.events,
            clock: parts.clock,
            zone: parts.zone,
            saved: Mutex::new(state::load(&parts.data_dir)),
            data_dir: parts.data_dir,
            maps: parts.maps,
            maps_cache: Mutex::new(None),
            web: parts.web,
            notifier: parts.notifier,
            paired: parts.paired,
            tasks: parts.tasks,
            http,
            memo: Default::default(),
            inner: Mutex::new(Inner { secrets, access: None, pending: Pending::default(), sticky: None, told_reconnect: false }),
            refreshing: tokio::sync::Mutex::new(()),
        })
    }

    /// The module of the daemon: real hosts, the keyring (or this home's file) and the clock.
    #[allow(clippy::too_many_arguments)]
    pub async fn daemon(
        settings: Arc<RwLock<Settings>>,
        paths: &crate::paths::Paths,
        events: broadcast::Sender<AgentEvent>,
        web: Arc<crate::web::Web>,
        notifier: Arc<crate::notify::Notifier>,
        bots: Arc<crate::bots::Bots>,
        task_store: Arc<crate::tasks::TaskStore>,
    ) -> Arc<Self> {
        Self::new(Parts {
            settings,
            settings_path: paths.settings(),
            secrets: TokenStore::slot(&paths.config, crate::secrets::CALENDAR),
            hosts: Hosts::real(),
            redirect: REDIRECT.into(),
            events,
            clock: Arc::new(Utc::now),
            data_dir: paths.data.clone(),
            maps: TokenStore::slot(&paths.config, crate::secrets::MAPS),
            web: Some(web),
            notifier: Some(notifier),
            paired: Arc::new(move || bots.served().is_some()),
            tasks: Arc::new(move |day| report_run::tasks_due(&task_store, day)),
            zone: Arc::new(|at| *at.with_timezone(&chrono::Local).offset()),
        })
        .await
    }

    pub fn config(&self) -> CalendarSection {
        self.settings.read().unwrap().calendar_config()
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub fn state(&self) -> ConnState {
        self.state_and_error().0
    }

    /// The state, and what went wrong when it is `reconnect` or `error`.
    pub fn state_and_error(&self) -> (ConnState, Option<String>) {
        let inner = self.inner.lock().unwrap();
        state_of(&inner, self.now())
    }

    pub fn has_client_secret(&self) -> bool {
        !self.inner.lock().unwrap().secrets.client_secret.is_empty()
    }

    pub fn account(&self) -> Option<String> {
        Some(self.inner.lock().unwrap().secrets.account.clone()).filter(|a| !a.is_empty())
    }
}

fn state_of(inner: &Inner, now: DateTime<Utc>) -> (ConnState, Option<String>) {
    if let Some((state, why)) = &inner.sticky {
        return (*state, Some(why.clone()));
    }
    if !inner.secrets.refresh_token.is_empty() {
        (ConnState::Connected, None)
    } else if inner.pending.any(now) {
        (ConnState::Connecting, None)
    } else {
        (ConnState::Off, None)
    }
}
