//! The polling task (bots.md sections 2, 3 and 6): what the bot does now, one long poll
//! at a time, and the state every UI is told of.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;

use kagent_core::protocol::AgentEvent;

use super::telegram_api::{ApiError, Keyboard, Markup, Message, RemoteFile, TelegramApi, Update, User, POLL_SEC, RETRY_DELAY};
use super::{Bots, PollState};

/// What the bot does after a look at the settings, the token and the pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    Off,
    Disabled,
    Poll,
}

/// What Telegram is while no token is set: every call fails, nothing leaves the machine.
pub struct NoToken;

fn no_token() -> ApiError {
    ApiError::Transport("no bot token is set".into())
}

#[async_trait]
impl TelegramApi for NoToken {
    async fn get_me(&self) -> Result<User, ApiError> {
        Err(no_token())
    }

    async fn get_updates(&self, _offset: Option<i64>, _timeout_sec: u32) -> Result<Vec<Update>, ApiError> {
        Err(no_token())
    }

    async fn send_message(&self, _chat_id: i64, _text: &str, _markup: Markup, _keyboard: Option<Keyboard>) -> Result<Message, ApiError> {
        Err(no_token())
    }

    async fn edit_message_text(&self, _chat_id: i64, _message_id: i64, _text: &str, _markup: Markup, _keyboard: Option<Keyboard>) -> Result<Message, ApiError> {
        Err(no_token())
    }

    async fn answer_callback(&self, _callback_id: &str, _text: Option<&str>) -> Result<bool, ApiError> {
        Err(no_token())
    }

    async fn get_file(&self, _file_id: &str) -> Result<RemoteFile, ApiError> {
        Err(no_token())
    }

    async fn download(&self, _path: &str) -> Result<Vec<u8>, ApiError> {
        Err(no_token())
    }
}

impl Bots {
    /// The loop of the daemon: poll while the bot is on, and look again whenever the
    /// settings, the token or the pairing change (`Bots::wake`).
    pub(crate) async fn run(self: Arc<Self>) {
        let mut wake = self.wake.subscribe();
        // What is true now is read below, before anything waits for a change.
        wake.borrow_and_update();
        'poll: loop {
            if self.plan().await != Plan::Poll {
                if wake.changed().await.is_err() {
                    break;
                }
                continue;
            }
            let (api, offset) = (self.bot.api(), self.bot.offset());
            let polled = tokio::select! {
                result = api.get_updates(offset, POLL_SEC) => Some(result),
                changed = wake.changed() => {
                    if changed.is_err() { break 'poll; }
                    None
                }
            };
            match polled {
                Some(Ok(updates)) => {
                    self.rest();
                    for update in &updates {
                        self.bot.handle(update, SystemTime::now()).await;
                        // The next poll starts after this update, whatever came of it.
                        self.bot.handled(update);
                    }
                }
                Some(Err(e)) => {
                    tracing::warn!("telegram: the poll failed: {e}");
                    self.set_state(PollState::Error, e.to_string());
                    self.clock.sleep(RETRY_DELAY).await;
                }
                None => {}
            }
        }
    }

    /// Look at the settings, the token and the pairing: the client of the token, and the
    /// state every UI is told of. An error stays until a poll succeeds: the next poll is
    /// its retry.
    pub(crate) async fn plan(&self) -> Plan {
        let enabled = self.section().enabled;
        let Some(token) = self.token.lock().unwrap().clone() else {
            self.rest();
            return Plan::Off;
        };
        if !enabled {
            self.rest();
            return Plan::Disabled;
        }
        if self.api_for.lock().unwrap().as_deref() != Some(token.as_str()) {
            let api = match (self.maker)(&token) {
                Ok(api) => api,
                Err(why) => {
                    self.set_state(PollState::Error, why);
                    return Plan::Off;
                }
            };
            // Whose token this is: the chat that pairs is told the bot's username, also
            // after a daemon restart.
            if let Ok(me) = api.get_me().await {
                self.bot.set_username(me.username.as_deref().unwrap_or_default());
            }
            self.bot.set_api(api);
            *self.api_for.lock().unwrap() = Some(token);
        }
        if self.known.lock().unwrap().status != PollState::Error {
            self.rest();
        }
        Plan::Poll
    }

    /// The state while nothing is wrong: off, switched off, waiting for pairing or polling.
    pub(crate) fn rest(&self) {
        let paired = self.bot.paired().is_some();
        let state = match (self.has_token(), self.section().enabled, paired) {
            (false, _, _) => PollState::Off,
            (true, false, _) => PollState::Disabled,
            (true, true, false) => PollState::Unpaired,
            (true, true, true) => PollState::Polling,
        };
        self.set_state(state, "");
    }

    /// The state every UI is told of (`telegramStatus`), when it changed. The detail of
    /// an error is kept as the last one.
    pub(crate) fn set_state(&self, status: PollState, detail: impl Into<String>) {
        let detail = detail.into();
        let changed = {
            let mut known = self.known.lock().unwrap();
            let changed = known.status != status || known.detail != detail;
            if changed {
                if status == PollState::Error {
                    known.last_error = Some(detail.clone());
                }
                known.status = status;
                known.detail = detail.clone();
            }
            changed
        };
        if changed {
            tracing::info!(status = status.name(), detail = %detail, "telegram: state changed");
            let _ = self.events.send(AgentEvent::TelegramStatus { status: status.name().to_string(), detail });
        }
    }
}
