//! Bots (bots.md): the Telegram bot. `telegram_api.rs` speaks to Telegram, `format.rs`
//! prepares text, `cards.rs` says what goes to the chat with buttons, `settings.rs` holds
//! the settings section and the pairing record, `telegram.rs` is the bot of the paired
//! chat and `agent.rs` what it needs from the agent. This file is the glue in the daemon:
//! `Bots` keeps the token, the state and the answer of the settings API; `poll.rs` is its
//! polling task, `forward.rs` the Telegram channel of the `Notifier`, the push of alerts
//! and the forwarding of approval cards, and `http.rs` the routes.

pub mod agent;
pub mod cards;
pub mod format;
pub mod forward;
pub mod http;
mod poll;
pub mod settings;
pub mod telegram;
pub mod telegram_api;
pub mod voice;

#[cfg(test)]
mod fakes;
#[cfg(test)]
mod tests;

pub use agent::{BotAgent, DaemonAgent, Status, TurnEnd};
pub use telegram::Bot;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};

use kagent_core::protocol::AgentEvent;

use self::settings::{Pairing, TelegramSection};
use self::telegram_api::{ApiError, Clock, TelegramApi, TelegramClient, TokioClock, API_BASE};
use crate::notify::Notifier;
use crate::secrets::TokenStore;
use crate::server::{bad_request, HttpError};
use crate::settings::Settings;
use crate::watch::{Alert, AlertManager};

/// The polling state of bots.md section 6, as the event and the settings API name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PollState {
    /// No token: there is no bot yet.
    Off,
    /// A token, but `telegram.enabled` is off.
    Disabled,
    /// On, and no chat is paired: `/start <code>` is waited for.
    Unpaired,
    /// On, paired, and polling.
    Polling,
    /// The last poll failed; `detail` says why.
    Error,
}

impl PollState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Disabled => "disabled",
            Self::Unpaired => "unpaired",
            Self::Polling => "polling",
            Self::Error => "error",
        }
    }
}

/// What `Bots` is built from. The daemon fills it in `Bots::daemon`; a test with fakes.
pub struct Parts {
    pub agent: Arc<dyn BotAgent>,
    /// The client of a token: the real one in the daemon, a fake in a test.
    pub maker: Arc<dyn Fn(&str) -> Result<Arc<dyn TelegramApi>, String> + Send + Sync>,
    pub clock: Arc<dyn Clock>,
    pub tokens: TokenStore,
    pub settings: Arc<RwLock<Settings>>,
    /// Where the settings are saved after `PUT /api/telegram`.
    pub settings_path: PathBuf,
    /// `<config>`: the pairing file is written here.
    pub config_dir: PathBuf,
    pub events: broadcast::Sender<AgentEvent>,
    /// Whether the watchers hold notifications back now (`watchers.quietHours`).
    pub quiet: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// What is known of the bot: the state of the poll task and the last error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Known {
    pub status: PollState,
    /// More about the state: what Telegram said when the poll failed.
    pub detail: String,
    /// The last error, kept after the bot recovered.
    pub last_error: Option<String>,
}

/// What `GET /api/telegram` answers (api.md section 6) and `status`, the state of the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct View {
    enabled: bool,
    configured: bool,
    status: PollState,
    /// The bot the token belongs to; `None` until Telegram was asked.
    bot_username: Option<String>,
    paired: bool,
    chat_id: Option<i64>,
    paired_at: Option<String>,
    last_error: Option<String>,
    /// The pairing code still waiting: `{code, expiresAt}`.
    pending_code: Option<Value>,
    /// Until when no pairing code is checked, after too many wrong ones.
    pairing_locked_until: Option<String>,
}

/// The keys `PUT /api/telegram` takes (config.md section 11).
const FIELDS: [&str; 5] = ["enabled", "notifyAlerts", "forwardApprovals", "approvalForwardSec", "allowPrivileged"];

fn internal(e: anyhow::Error) -> HttpError {
    HttpError(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}"))
}

/// A token Telegram would not confirm: refused by Telegram, or no answer.
fn refused(e: ApiError) -> HttpError {
    match e {
        ApiError::Api { .. } => bad_request(format!("Telegram refused the token: {e}")),
        other => HttpError(axum::http::StatusCode::BAD_GATEWAY, "telegram_unreachable", format!("Telegram did not answer: {other}")),
    }
}

/// The Telegram bot of this daemon: one per process, behind the routes and the poll task.
pub struct Bots {
    pub bot: Arc<Bot>,
    agent: Arc<dyn BotAgent>,
    maker: Arc<dyn Fn(&str) -> Result<Arc<dyn TelegramApi>, String> + Send + Sync>,
    clock: Arc<dyn Clock>,
    tokens: TokenStore,
    settings: Arc<RwLock<Settings>>,
    settings_path: PathBuf,
    events: broadcast::Sender<AgentEvent>,
    quiet: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The token, in memory only: it never leaves the daemon.
    token: Mutex<Option<String>>,
    /// The token the bot's client was made for.
    api_for: Mutex<Option<String>>,
    known: Mutex<Known>,
    /// Tells the poll task that the settings, the token or the pairing changed.
    wake: watch::Sender<()>,
}

impl Bots {
    pub async fn new(parts: Parts) -> Arc<Self> {
        let bot = Bot::new(Arc::new(poll::NoToken), parts.agent.clone(), parts.clock.clone(), parts.config_dir.clone());
        bot.restore();
        let token = match parts.tokens.get().await {
            Ok(token) => token,
            Err(e) => {
                tracing::warn!("telegram: the token could not be read: {e:#}");
                None
            }
        };
        Arc::new(Self {
            bot,
            agent: parts.agent,
            maker: parts.maker,
            clock: parts.clock,
            tokens: parts.tokens,
            settings: parts.settings,
            settings_path: parts.settings_path,
            events: parts.events,
            quiet: parts.quiet,
            token: Mutex::new(token),
            api_for: Mutex::new(None),
            known: Mutex::new(Known { status: PollState::Off, detail: String::new(), last_error: None }),
            wake: watch::channel(()).0,
        })
    }

    /// The bots of the daemon: the agent, the alerts, the real client of a token and the
    /// token store of this home.
    pub async fn daemon(
        agent: Arc<kagent_core::agent::Agent>,
        alerts: Arc<AlertManager>,
        providers: Arc<kagent_providers::ProviderStore>,
        settings: Arc<RwLock<Settings>>,
        paths: &crate::paths::Paths,
        events: broadcast::Sender<AgentEvent>,
    ) -> Arc<Self> {
        Self::new(Parts {
            agent: Arc::new(DaemonAgent { agent, alerts: alerts.clone(), providers, settings: settings.clone() }),
            maker: Arc::new(|token: &str| -> Result<Arc<dyn TelegramApi>, String> {
                Ok(Arc::new(TelegramClient::new(API_BASE, token, Arc::new(TokioClock))?))
            }),
            clock: Arc::new(TokioClock),
            tokens: TokenStore::telegram(&paths.config),
            settings,
            settings_path: paths.settings(),
            config_dir: paths.config.clone(),
            events,
            quiet: Arc::new(move || alerts.quiet_now()),
        })
        .await
    }

    /// Start: the Telegram channel of the notifier, the push of alerts, the poll task and
    /// the cards of the daemon.
    pub fn start(self: &Arc<Self>, notifier: Arc<Notifier>, alerts: Arc<AlertManager>) {
        let to_chat = Arc::new(forward::ToChat { bots: self.clone() });
        notifier.set_telegram_sink(to_chat.clone());
        alerts.set_alert_push(to_chat.clone());
        tokio::spawn(self.clone().run());
        tokio::spawn(to_chat.watch_cards());
    }

    pub fn section(&self) -> TelegramSection {
        self.settings.read().unwrap().telegram_config()
    }

    pub fn has_token(&self) -> bool {
        self.token.lock().unwrap().is_some()
    }

    pub(crate) fn known(&self) -> Known {
        self.known.lock().unwrap().clone()
    }

    /// Something changed: the poll task looks again.
    pub fn wake(&self) {
        self.wake.send_replace(());
    }

    /// The paired chat, and only while the bot is on (enabled, a token, a pairing). Every
    /// way into the chat asks here first.
    pub fn served(&self) -> Option<Pairing> {
        if !self.has_token() || !self.section().enabled {
            return None;
        }
        self.bot.paired()
    }

    /// A raised alert goes to the chat (bots.md section 5); the chat it goes to, if any.
    pub(crate) fn alert_goes(&self, alert: &Alert) -> Option<i64> {
        let chat = self.served()?.chat_id;
        cards::alert_goes(alert.severity, self.section().notify_alerts, (self.quiet)()).then_some(chat)
    }

    /// `GET /api/telegram`: no token in it, only whether there is one.
    pub fn view(&self) -> Value {
        let section = self.section();
        let pairing = self.bot.paired();
        let known = self.known();
        let username = self.bot.username();
        serde_json::to_value(View {
            enabled: section.enabled,
            configured: self.has_token(),
            status: known.status,
            bot_username: (!username.is_empty()).then_some(username),
            paired: pairing.is_some(),
            chat_id: pairing.as_ref().map(|p| p.chat_id),
            paired_at: pairing.map(|p| p.paired_at),
            last_error: known.last_error,
            pending_code: self.bot.code(SystemTime::now()).map(|code| code.view()),
            pairing_locked_until: self.bot.pairing_locked_until(SystemTime::now()).map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
        })
        .unwrap_or(Value::Null)
    }

    /// `PUT /api/telegram`: the five settings of the section, saved like `PUT /api/settings`.
    pub fn patch(&self, patch: Value) -> Result<(), HttpError> {
        let Value::Object(fields) = &patch else { return Err(bad_request("the patch must be an object")) };
        for key in fields.keys() {
            if !FIELDS.contains(&key.as_str()) {
                return Err(bad_request(format!("unknown key '{key}': one of {}", FIELDS.join(", "))));
            }
        }
        let settings = {
            let mut s = self.settings.write().unwrap();
            s.patch(json!({ "telegram": patch })).map_err(bad_request)?;
            s.save(&self.settings_path).map_err(internal)?;
            s.clone()
        };
        let _ = self.events.send(AgentEvent::SettingsChanged { settings: serde_json::to_value(&settings).unwrap_or(Value::Null) });
        self.wake();
        Ok(())
    }

    /// `POST /api/telegram/token`: asked of Telegram first (`getMe`); kept only when
    /// Telegram knows it and says it is a bot.
    pub async fn set_token(&self, token: &str) -> Result<String, HttpError> {
        telegram_api::check_token(token).map_err(bad_request)?;
        let api = (self.maker)(token).map_err(bad_request)?;
        let me = api.get_me().await.map_err(refused)?;
        let username = me.username.filter(|_| me.is_bot).ok_or_else(|| bad_request("Telegram says this token is not a bot"))?;
        self.tokens.set(token).await.map_err(internal)?;
        *self.token.lock().unwrap() = Some(token.to_string());
        *self.api_for.lock().unwrap() = Some(token.to_string());
        self.bot.set_username(&username);
        self.bot.set_api(api);
        self.known.lock().unwrap().last_error = None;
        tracing::info!(bot = %username, "telegram: a token is stored");
        self.rest();
        self.wake();
        Ok(username)
    }

    /// `DELETE /api/telegram/token`: the bot is off, and the chat is forgotten.
    pub async fn clear_token(&self) -> anyhow::Result<()> {
        self.tokens.clear().await?;
        if let Err(e) = self.bot.unpair() {
            tracing::warn!("telegram: the pairing could not be forgotten: {e:#}");
        }
        *self.token.lock().unwrap() = None;
        *self.api_for.lock().unwrap() = None;
        self.bot.set_api(Arc::new(poll::NoToken));
        self.bot.set_username("");
        self.known.lock().unwrap().last_error = None;
        tracing::info!("telegram: the token is deleted, the chat is forgotten");
        self.rest();
        self.wake();
        Ok(())
    }

    /// `DELETE /api/telegram/pairing`: the chat is forgotten; the bot waits for a new one.
    pub fn unpair(&self) -> anyhow::Result<()> {
        let forgotten = self.bot.unpair();
        self.rest();
        self.wake();
        forgotten
    }

    /// `POST /api/telegram/pairing-code`: six digits, ten minutes, one use.
    pub fn pairing_code(&self) -> Result<Value, HttpError> {
        if !self.has_token() {
            return Err(bad_request("no bot token: paste the token of your bot first"));
        }
        if !self.section().enabled {
            return Err(bad_request("Telegram is switched off"));
        }
        if self.bot.paired().is_some() {
            return Err(HttpError(axum::http::StatusCode::CONFLICT, "conflict", "a chat is already paired; unpair it first".into()));
        }
        let code = self.bot.ask_code(SystemTime::now());
        self.wake();
        Ok(code.view())
    }

    /// `POST /api/telegram/test`: one message to the paired chat.
    pub async fn test_message(&self) -> Value {
        let Some(pairing) = self.served() else {
            return json!({ "ok": false, "error": "the bot is off, has no token or no paired chat" });
        };
        match self.bot.api().send_message(pairing.chat_id, "Test message from KAgent: Telegram is connected.", telegram_api::Markup::None, None).await {
            Ok(_) => json!({ "ok": true }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    }
}
