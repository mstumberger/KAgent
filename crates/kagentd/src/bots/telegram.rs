//! The bot of the paired chat (bots.md sections 2 and 3): pairing, the commands, the
//! turns and their answers, and the buttons of the cards. It reaches Telegram through
//! `TelegramApi` and the agent through `BotAgent`, so its tests use fakes, and its
//! clock is passed in.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};


use super::format;
use super::settings::{Pairing, PairingCode, CODE_TTL};
use super::telegram_api::{Clock, Keyboard, Markup, Message, RateLimiter, TelegramApi, Update};
use super::BotAgent;

/// At most this many turns a minute in the paired chat.
const TURNS_PER_MINUTE: usize = 3;
/// The session of the chat is titled so.
pub const SESSION_TITLE: &str = "Telegram";
/// How many sent cards a press may still refer to.
const CARDS_KEPT: usize = 256;
/// After this many wrong pairing codes the code is replaced and pairing is locked
/// (bots.md section 2, decision of 2026-11-27).
pub const WRONG_CODES: u32 = 5;
/// How long pairing stays locked after `WRONG_CODES` wrong codes.
pub const PAIRING_LOCK: Duration = Duration::from_secs(600);

const WORKING: &str = "Working on it...";
const NO_ANSWER: &str = "The agent gave no answer.";
const HELP: &str = "Commands:\n/new - a new session\n/status - health, open alerts, running turns\n/stop - stop every running turn\n/help - this list\n\nAnything else goes to KAgent as a message. Voice notes are transcribed when a Gemini model is enabled.";

/// What a message of the chat asks for. A command may name the bot (`/status@my_bot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Start(Option<String>),
    New,
    Status,
    Stop,
    Help,
    /// Free text: a turn.
    Text(String),
    /// A command that names another bot.
    Other,
}

/// `command(text, bot_username)`: the command of a message.
pub fn command(text: &str, bot_username: &str) -> Command {
    let text = text.trim();
    let Some(after_slash) = text.strip_prefix('/') else {
        return Command::Text(text.to_string());
    };
    let word = after_slash.split_whitespace().next().unwrap_or("");
    let (name, bot) = word.split_once('@').unwrap_or((word, ""));
    if !bot.is_empty() && !bot_username.is_empty() && !bot.eq_ignore_ascii_case(bot_username) {
        return Command::Other;
    }
    let rest = &text[word.len() + 1..];
    let arg = rest.split_whitespace().next();
    match name {
        "start" => Command::Start(arg.map(str::to_string)),
        "new" => Command::New,
        "status" => Command::Status,
        "stop" => Command::Stop,
        "help" => Command::Help,
        _ => Command::Text(text.to_string()),
    }
}

/// `/status`: the doctor score, the open alerts and the running turns.
pub fn status_text(status: &super::Status) -> String {
    let doctor = match status.doctor {
        Some(score) => format!("Health: {score}/100"),
        None => "Health: not checked now".to_string(),
    };
    format!("{doctor}\nOpen alerts: {}\nRunning turns: {}", status.open_alerts, status.running)
}


/// A card this bot sent to the chat, so a press on its buttons can be answered.
struct Card {
    text: String,
}

/// The turn of one session that the chat waits for: its placeholder, and how many more messages were queued behind it.
#[derive(Default)]
struct Working {
    /// The message that says "Working on it...", when one could be sent.
    message: Option<i64>,
    queued: usize,
}

#[derive(Default)]
struct Inner {
    pairing: Option<Pairing>,
    /// The username of the bot the token belongs to.
    username: String,
    /// The session of the chat: `None` until its first message and after `/new`.
    session: Option<String>,
    code: Option<PairingCode>,
    /// Wrong pairing codes since the last lock or pairing.
    wrong_codes: u32,
    /// No code is checked before this time.
    locked_until: Option<SystemTime>,
    turns: RateLimiter,
    /// By session: the turn the chat is waiting for.
    working: HashMap<String, Working>,
    /// By key (a request id, or what `cards::ack_data` makes): the cards a press may still answer.
    cards: HashMap<String, Card>,
    /// The next update Telegram may send: the last one's id plus one.
    offset: Option<i64>,
}

pub struct Bot {
    api: RwLock<Arc<dyn TelegramApi>>,
    agent: Arc<dyn BotAgent>,
    clock: Arc<dyn Clock>,
    config_dir: PathBuf,
    inner: Mutex<Inner>,
}

impl Bot {
    pub fn new(api: Arc<dyn TelegramApi>, agent: Arc<dyn BotAgent>, clock: Arc<dyn Clock>, config_dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            api: RwLock::new(api),
            agent,
            clock,
            config_dir: config_dir.into(),
            inner: Mutex::new(Inner { turns: RateLimiter::new(TURNS_PER_MINUTE, Duration::from_secs(60)), ..Default::default() }),
        })
    }

    /// Telegram as the bot talks to it now: the client of the current token.
    pub fn api(&self) -> Arc<dyn TelegramApi> {
        self.api.read().unwrap().clone()
    }

    /// A new token: the bot talks to Telegram through this from now on.
    pub fn set_api(&self, api: Arc<dyn TelegramApi>) {
        *self.api.write().unwrap() = api;
    }

    pub fn paired(&self) -> Option<Pairing> {
        self.inner.lock().unwrap().pairing.clone()
    }

    /// The username of the bot the token belongs to, as a new pairing records it.
    pub fn username(&self) -> String {
        self.inner.lock().unwrap().username.clone()
    }

    pub fn set_username(&self, username: &str) {
        self.inner.lock().unwrap().username = username.trim_start_matches('@').to_string();
    }

    /// Pair the chat for good: the file first, then memory.
    pub fn pair(&self, pairing: Pairing) -> anyhow::Result<()> {
        pairing.save(&self.config_dir)?;
        let mut inner = self.inner.lock().unwrap();
        inner.pairing = Some(pairing);
        inner.session = None;
        Ok(())
    }

    /// Forget the chat, its session and its cards.
    pub fn unpair(&self) -> anyhow::Result<()> {
        Pairing::forget(&self.config_dir)?;
        let mut inner = self.inner.lock().unwrap();
        inner.pairing = None;
        inner.session = None;
        inner.cards.clear();
        Ok(())
    }

    /// The pairing the daemon kept, read when the bot starts.
    pub fn restore(&self) -> Option<Pairing> {
        let pairing = Pairing::load(&self.config_dir);
        self.inner.lock().unwrap().pairing = pairing.clone();
        pairing
    }

    /// A new pairing code; it replaces the one before.
    /// While pairing is locked, the code is good for ten minutes from the end of the lock.
    pub fn ask_code(&self, now: SystemTime) -> PairingCode {
        let mut code = PairingCode::new(now);
        let mut inner = self.inner.lock().unwrap();
        if let Some(until) = inner.locked_until.filter(|t| *t > now) {
            code.expires_at = until + CODE_TTL;
        }
        inner.code = Some(code.clone());
        code
    }

    /// The code still waiting at `now`; one that expired is dropped.
    pub fn code(&self, now: SystemTime) -> Option<PairingCode> {
        let mut inner = self.inner.lock().unwrap();
        if inner.code.as_ref().is_some_and(|c| c.expired(now)) {
            inner.code = None;
        }
        inner.code.clone()
    }

    /// Until when pairing is locked after too many wrong codes, while it is.
    pub fn pairing_locked_until(&self, now: SystemTime) -> Option<SystemTime> {
        self.inner.lock().unwrap().locked_until.filter(|t| *t > now)
    }

    /// The offset of the next `getUpdates`: in memory, so a restart reads what Telegram still holds.
    pub fn offset(&self) -> Option<i64> {
        self.inner.lock().unwrap().offset
    }

    /// `update` was handled: the next poll starts after it.
    pub fn handled(&self, update: &Update) {
        self.inner.lock().unwrap().offset = Some(update.update_id + 1);
    }

    /// Send a card (a forwarded approval, a pushed alert) with its buttons, and keep it for a press.
    pub async fn card(&self, chat: i64, key: &str, text: &str, keyboard: Keyboard) -> Result<(), super::telegram_api::ApiError> {
        let text = format::cut(text, format::MAX_INPUT).to_string();
        self.api().send_message(chat, &text, Markup::None, Some(keyboard)).await?;
        let mut inner = self.inner.lock().unwrap();
        if inner.cards.len() >= CARDS_KEPT {
            inner.cards.clear();
        }
        inner.cards.insert(key.to_string(), Card { text });
        Ok(())
    }

    /// The card is over (answered, or its turn ended): a later press is "Already resolved".
    pub fn card_gone(&self, key: &str) {
        self.inner.lock().unwrap().cards.remove(key);
    }

    /// A text of the bot's own, plain, in a new message.
    pub async fn say(&self, chat: i64, text: &str) {
        if let Err(e) = self.api().send_message(chat, format::cut(text, format::MAX_INPUT), Markup::None, None).await {
            tracing::warn!("telegram: not sent: {e}");
        }
    }

    /// One update from Telegram. An update from a chat that is not served is dropped and logged before anything else happens.
    pub async fn handle(self: &Arc<Self>, update: &Update, now: SystemTime) {
        if let Some(message) = &update.message {
            self.message(message, now).await;
        } else if let Some(query) = &update.callback_query {
            self.press(query).await;
        } else {
            tracing::info!(update = update.update_id, "telegram: an update of a kind the bot does not use is dropped");
        }
    }

    async fn message(self: &Arc<Self>, m: &Message, now: SystemTime) {
        let chat = m.chat.id;
        let pairing = self.paired();
        if pairing.as_ref().is_some_and(|p| p.chat_id != chat) {
            // Before anything else: only the paired chat is served (bots.md section 8).
            tracing::info!(chat, user = m.from.as_ref().map_or(0, |u| u.id), "telegram: an update from a chat that is not the paired one is dropped");
            return;
        }
        if let Some(text) = m.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            return self.command(chat, m, command(text, &self.username()), now).await;
        }
        if pairing.is_none() {
            // Nothing is paired: only `/start <code>` (above) is looked at.
            tracing::info!(chat, "telegram: an update from a chat is dropped: nothing is paired");
            return;
        }
        if let Some(voice) = &m.voice {
            return self.voice(chat, voice).await;
        }
        if !m.photo.is_empty() {
            return self.photo(chat, m).await;
        }
        self.say(chat, "I cannot read attachments here yet. Photos and voice notes work.").await;
    }

    async fn command(self: &Arc<Self>, chat: i64, m: &Message, command: Command, now: SystemTime) {
        match command {
            Command::Other => tracing::info!(chat, "telegram: a command for another bot is dropped"),
            Command::Start(code) => self.start(chat, m, code, now).await,
            Command::Text(text) => match self.paired() {
                None => tracing::info!(chat, "telegram: a message from a chat is dropped: nothing is paired"),
                Some(_) => self.turn(chat, &text).await,
            },
            Command::New => {
                self.inner.lock().unwrap().session = None;
                self.say(chat, "New session. The next message starts a fresh conversation.").await;
            }
            Command::Status => {
                let status = self.agent.status().await;
                self.say(chat, &status_text(&status)).await;
            }
            Command::Stop => {
                let stopped = self.agent.stop_all().await;
                self.say(chat, &format!("Stopped {stopped} running turn(s).")).await;
            }
            Command::Help => self.say(chat, HELP).await,
        }
    }

    /// `/start`: in the paired chat it answers with the status. With a code, and nothing paired, the first chat with the right one pairs.
    async fn start(&self, chat: i64, m: &Message, code: Option<String>, now: SystemTime) {
        if let Some(p) = self.paired() {
            let status = self.agent.status().await;
            return self.say(chat, &format!("Paired with @{} since {}.\n{}", self.username(), p.paired_at, status_text(&status))).await;
        }
        let Some(code) = code else {
            return self.say(chat, "To pair this chat, open Settings > Telegram in KAgent, click \"Show pairing code\", and send the code as /start <code>.").await;
        };
        let verdict = {
            let mut inner = self.inner.lock().unwrap();
            let locked = inner.locked_until.is_some_and(|t| t > now);
            match inner.code.as_ref() {
                // Locked: no code is looked at, not even the right one.
                _ if locked => Verdict::Locked(inner.locked_until.unwrap_or(now)),
                Some(waiting) if waiting.matches(&code, now) => {
                    // One use: it goes the moment it is right.
                    inner.code = None;
                    inner.wrong_codes = 0;
                    inner.locked_until = None;
                    Verdict::Right
                }
                Some(waiting) if waiting.expired(now) => {
                    inner.code = None;
                    Verdict::Expired
                }
                Some(_) => {
                    inner.wrong_codes += 1;
                    if inner.wrong_codes >= WRONG_CODES {
                        // The guesses so far count for nothing: a new code, good from the end of the lock.
                        let until = now + PAIRING_LOCK;
                        inner.wrong_codes = 0;
                        inner.locked_until = Some(until);
                        let mut fresh = PairingCode::new(now);
                        fresh.expires_at = until + CODE_TTL;
                        inner.code = Some(fresh);
                        Verdict::Locked(until)
                    } else {
                        Verdict::Wrong
                    }
                }
                None => Verdict::NoCode,
            }
        };
        match verdict {
            Verdict::Right => {
                let pairing = Pairing { bot_username: self.username(), chat_id: chat, user_id: m.from.as_ref().map_or(0, |u| u.id), paired_at: chrono::DateTime::<chrono::Local>::from(now).to_rfc3339() };
                if let Err(e) = self.pair(pairing) {
                    tracing::warn!("telegram: the pairing could not be saved: {e:#}");
                    return self.say(chat, "The pairing could not be saved. Try again, or look at KAgent's log.").await;
                }
                tracing::info!(chat, "telegram: paired");
                self.say(chat, &format!("Paired. This chat now talks to KAgent.\n\n{HELP}")).await;
            }
            Verdict::Wrong => {
                tracing::info!(chat, "telegram: a wrong pairing code");
                self.say(chat, "That pairing code is not right.").await;
            }
            Verdict::Locked(until) => {
                let minutes = until.duration_since(now).unwrap_or_default().as_secs().div_ceil(60).max(1);
                tracing::warn!(chat, "telegram: pairing is locked after {WRONG_CODES} wrong codes");
                self.say(chat, &format!("Too many wrong pairing codes. Pairing is locked for {minutes} min; then use the new code shown in Settings > Telegram.")).await;
            }
            Verdict::Expired => self.say(chat, "That pairing code has expired. Ask for a new one in Settings > Telegram.").await,
            Verdict::NoCode => self.say(chat, "No pairing code is waiting. Click \"Show pairing code\" in Settings > Telegram.").await,
        }
    }

}

enum Verdict {
    Right,
    Wrong,
    /// Too many wrong codes: nothing is checked until this time.
    Locked(SystemTime),
    Expired,
    /// No code was asked for.
    NoCode,
}

mod press;
mod turns;

#[cfg(test)]
mod photo_tests;
#[cfg(test)]
mod tests;
