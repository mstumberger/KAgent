//! The Telegram Bot API as KAgent uses it (bots.md sections 3, 4 and 8): the types of
//! its JSON, the `TelegramApi` trait the bot is written against, and the real client.
//! Nothing here knows a chat or a card: that is `telegram.rs`. The client talks only to
//! its base URL (`https://api.telegram.org`, or a loopback fake in a test), makes 20
//! calls a minute, honours `retry_after`, and never puts the token in an error.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use kagent_core::util::truncate_str;

mod limits;
pub use limits::{bounded, check_base, check_token, safe_file_path, Clock, RateLimiter, TokioClock, CALLS_PER_MINUTE};
#[cfg(test)]
pub(crate) use limits::FakeClock;
#[cfg(test)]
mod tests;

/// The only base the daemon talks to Telegram on. A test points the client at a fake on loopback.
pub const API_BASE: &str = "https://api.telegram.org";
/// `getUpdates` holds the connection this long.
pub const POLL_SEC: u32 = 30;
/// The HTTP call of a long poll may take 10 s longer than the poll itself.
pub const POLL_HTTP_TIMEOUT: Duration = Duration::from_secs(40);
/// Every other call.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// After a failed poll, the next one starts this late.
pub const RETRY_DELAY: Duration = Duration::from_secs(5);
/// Telegram gives bots files up to this size.
pub const MAX_FILE_BYTES: usize = 20 * 1024 * 1024;

// ------------------------------------------------------------------ the JSON

/// The types below mirror the Bot API's JSON, so they keep its snake_case names, and a
/// field the type does not name is simply ignored (or, for `Message`, collected).

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct User {
    pub id: i64,
    pub is_bot: bool,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    pub username: Option<String>,
    pub first_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Voice {
    pub file_id: String,
    pub duration: i64,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

/// One size of a photo: Telegram sends the same picture in several, smallest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct PhotoSize {
    pub file_id: String,
    pub width: i64,
    pub height: i64,
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub date: i64,
    pub text: Option<String>,
    pub voice: Option<Voice>,
    /// A photo (vision.md section 2): its sizes, and the words sent with it.
    pub photo: Vec<PhotoSize>,
    pub caption: Option<String>,
    /// Every other field of the message: a photo, a document, a sticker, ... is here.
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

impl Message {
    /// The biggest size of the photo that Telegram lets a bot download.
    pub fn best_photo(&self) -> Option<&PhotoSize> {
        self.photo.iter().filter(|p| p.file_size.is_none_or(|n| n as usize <= MAX_FILE_BYTES)).max_by_key(|p| p.width * p.height)
    }

    /// A photo, a document, ...: anything but text and a voice note.
    pub fn is_attachment(&self) -> bool {
        !self.other.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct CallbackQuery {
    pub id: String,
    pub from: Option<User>,
    /// The message that carries the button.
    pub message: Option<Message>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
}

/// What `getFile` answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct RemoteFile {
    pub file_id: String,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Parameters {
    #[serde(default)]
    retry_after: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct Answer<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
    parameters: Option<Parameters>,
}

// ------------------------------------------------------------------ what the bot sends

/// How Telegram reads the text of a message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Markup {
    /// Plain text, read as it is.
    #[default]
    None,
    /// Telegram HTML (the tags `format.rs` makes).
    Html,
}

/// A button that answers with `callback_data`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub text: String,
    pub callback_data: String,
}

/// An inline keyboard under a message: rows of buttons.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keyboard(pub Vec<Vec<Button>>);

impl Keyboard {
    pub fn row(buttons: impl IntoIterator<Item = Button>) -> Self {
        Self(vec![buttons.into_iter().collect()])
    }

    /// `reply_markup` of the Bot API.
    pub fn json(&self) -> Value {
        json!({ "inline_keyboard": self.0.iter().map(|row| row.iter().map(|b| json!({"text": b.text, "callback_data": b.callback_data})).collect::<Vec<_>>()).collect::<Vec<_>>() })
    }
}

/// A call that did not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    /// Telegram refused the call; what it said never contains the token.
    Api { status: u16, description: String },
    /// 429, twice in a row: how many seconds Telegram asked to wait.
    RetryAfter(u32),
    /// No answer, a late one, or one that is not the Bot API's.
    Transport(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api { status, description } => write!(f, "Telegram answered {status}: {description}"),
            Self::RetryAfter(secs) => write!(f, "Telegram asks to wait {secs} s"),
            Self::Transport(what) => write!(f, "no answer from Telegram: {what}"),
        }
    }
}

/// Telegram says the text cannot be parsed (`can't parse entities`): the markup is at fault, not the network.
pub fn unparsable(error: &ApiError) -> bool {
    matches!(error, ApiError::Api { status: 400, description } if description.contains("parse"))
}

// ------------------------------------------------------------------ the trait the bot is written against

/// What the bot needs from Telegram. The daemon uses `TelegramClient`; the tests a fake.
#[async_trait]
pub trait TelegramApi: Send + Sync {
    /// The bot itself.
    async fn get_me(&self) -> Result<User, ApiError>;
    /// One long poll: the updates after `offset`.
    async fn get_updates(&self, offset: Option<i64>, timeout_sec: u32) -> Result<Vec<Update>, ApiError>;
    async fn send_message(&self, chat_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>) -> Result<Message, ApiError>;
    /// Replace the text of a message of the bot, and its keyboard.
    async fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>) -> Result<Message, ApiError>;
    /// Answer a button press with a short note.
    async fn answer_callback(&self, callback_id: &str, text: Option<&str>) -> Result<bool, ApiError>;
    /// Where a file is (`file_path`, to be checked with `safe_file_path`).
    async fn get_file(&self, file_id: &str) -> Result<RemoteFile, ApiError>;
    /// The bytes of a file at a path that passed `safe_file_path`.
    async fn download(&self, path: &str) -> Result<Vec<u8>, ApiError>;
}

/// `edit`: the body of `editMessageText`, which says its keyboard every time.
fn text_args(chat_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>, edit: bool) -> Value {
    let mut body = json!({ "chat_id": chat_id, "text": text });
    if markup == Markup::Html {
        body["parse_mode"] = json!("HTML");
    }
    match keyboard {
        Some(keyboard) => body["reply_markup"] = keyboard.json(),
        // An edit without a keyboard takes the buttons off the message; a new message
        // needs no empty keyboard.
        None if edit => body["reply_markup"] = json!({ "inline_keyboard": [] }),
        None => {}
    }
    body
}

// ------------------------------------------------------------------ the real client

pub struct TelegramClient {
    http: reqwest::Client,
    base: String,
    token: String,
    clock: Arc<dyn Clock>,
    /// The calls of the last minute, and until when a 429 holds the client.
    guard: Mutex<(RateLimiter, Option<std::time::Instant>)>,
}

impl std::fmt::Debug for TelegramClient {
    /// Where it talks to; the token is never shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramClient").field("base", &self.base).finish_non_exhaustive()
    }
}

impl TelegramClient {
    pub fn new(base: &str, token: &str, clock: Arc<dyn Clock>) -> Result<Self, String> {
        check_base(base)?;
        check_token(token)?;
        let http = reqwest::Client::builder()
            // A redirect would carry the token in its URL to another host.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            clock,
            guard: Mutex::new((RateLimiter::new(CALLS_PER_MINUTE, Duration::from_secs(60)), None)),
        })
    }

    /// One Bot API method. It waits for a free call, and for a 429 that still holds; a
    /// 429 is tried once more after the wait Telegram named.
    async fn call<T: DeserializeOwned>(&self, method: &str, body: Value, timeout: Duration) -> Result<T, ApiError> {
        let url = format!("{}/bot{}/{}", self.base, self.token, method);
        for attempt in 0..2 {
            self.admit().await;
            let http = self.http.clone();
            let url = url.clone();
            let body = body.clone();
            let (status, bytes) = match bounded(self.clock.as_ref(), timeout, async move {
                let resp = http.post(&url).json(&body).send().await.map_err(transport)?;
                let status = resp.status();
                let bytes = resp.bytes().await.map_err(transport)?;
                Ok::<_, ApiError>((status, bytes))
            })
            .await
            {
                Some(Ok(answer)) => answer,
                Some(Err(error)) => return Err(error),
                None => return Err(ApiError::Transport(format!("{method}: Telegram did not answer in time"))),
            };
            let shown = truncate_str(&String::from_utf8_lossy(&bytes), 200).to_string();
            let answer: Answer<T> = serde_json::from_slice(&bytes).map_err(|_| ApiError::Transport(format!("{method}: the answer is not the Bot API's JSON ({status}): {shown}")))?;
            if answer.ok {
                return answer.result.ok_or_else(|| ApiError::Transport(format!("{method}: the answer has no result")));
            }
            if let Some(secs) = answer.parameters.and_then(|p| p.retry_after) {
                self.hold(u64::from(secs.max(1)));
                if attempt == 0 {
                    continue;
                }
                return Err(ApiError::RetryAfter(secs));
            }
            return Err(ApiError::Api { status: status.as_u16(), description: answer.description.unwrap_or_default() });
        }
        Err(ApiError::Transport(format!("{method}: not tried")))
    }

    /// Wait until a call may go: 20 a minute, and not while a `retry_after` holds.
    async fn admit(&self) {
        loop {
            let wait = {
                let mut guard = self.guard.lock().unwrap();
                let now = self.clock.now();
                let held = guard.1.map_or(Duration::ZERO, |until| until.saturating_duration_since(now));
                if held.is_zero() {
                    guard.0.reserve(now)
                } else {
                    held
                }
            };
            if wait.is_zero() {
                return;
            }
            self.clock.sleep(wait).await;
        }
    }

    /// A 429: no call goes before `now + retry_after`.
    fn hold(&self, retry_after_sec: u64) {
        let now = self.clock.now();
        self.guard.lock().unwrap().1 = Some(now.checked_add(Duration::from_secs(retry_after_sec)).unwrap_or(now));
    }
}

/// A reqwest error says its URL, and the URL has the token: the message is taken without it.
fn transport(error: reqwest::Error) -> ApiError {
    ApiError::Transport(error.without_url().to_string())
}

#[async_trait]
impl TelegramApi for TelegramClient {
    async fn get_me(&self) -> Result<User, ApiError> {
        self.call("getMe", json!({}), CALL_TIMEOUT).await
    }

    async fn get_updates(&self, offset: Option<i64>, timeout_sec: u32) -> Result<Vec<Update>, ApiError> {
        let mut body = json!({ "timeout": timeout_sec, "allowed_updates": ["message", "callback_query"] });
        // No offset yet: the updates Telegram still holds, none named.
        if let Some(offset) = offset {
            body["offset"] = json!(offset);
        }
        self.call("getUpdates", body, POLL_HTTP_TIMEOUT).await
    }

    async fn send_message(&self, chat_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>) -> Result<Message, ApiError> {
        self.call("sendMessage", text_args(chat_id, text, markup, keyboard, false), CALL_TIMEOUT).await
    }

    async fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>) -> Result<Message, ApiError> {
        let mut body = text_args(chat_id, text, markup, keyboard, true);
        body["message_id"] = json!(message_id);
        self.call("editMessageText", body, CALL_TIMEOUT).await
    }

    async fn answer_callback(&self, callback_id: &str, text: Option<&str>) -> Result<bool, ApiError> {
        let mut body = json!({ "callback_query_id": callback_id });
        if let Some(text) = text {
            body["text"] = json!(text);
        }
        self.call("answerCallbackQuery", body, CALL_TIMEOUT).await
    }

    async fn get_file(&self, file_id: &str) -> Result<RemoteFile, ApiError> {
        self.call("getFile", json!({ "file_id": file_id }), CALL_TIMEOUT).await
    }

    async fn download(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        let path = safe_file_path(path).ok_or_else(|| ApiError::Transport("the file path Telegram answered is not a plain relative path".into()))?;
        let url = format!("{}/file/bot{}/{}", self.base, self.token, path);
        self.admit().await;
        let http = self.http.clone();
        let (status, bytes) = match bounded(self.clock.as_ref(), CALL_TIMEOUT, async move {
            let resp = http.get(&url).send().await.map_err(transport)?;
            if resp.content_length().is_some_and(|len| len > MAX_FILE_BYTES as u64) {
                return Err(ApiError::Transport("the file is larger than 20 MB".into()));
            }
            let status = resp.status();
            let bytes = resp.bytes().await.map_err(transport)?;
            Ok::<_, ApiError>((status, bytes))
        })
        .await
        {
            Some(Ok(answer)) => answer,
            Some(Err(error)) => return Err(error),
            None => return Err(ApiError::Transport("file: Telegram did not answer in time".into())),
        };
        if !status.is_success() {
            return Err(ApiError::Api { status: status.as_u16(), description: truncate_str(&String::from_utf8_lossy(&bytes), 200).to_string() });
        }
        if bytes.len() > MAX_FILE_BYTES {
            return Err(ApiError::Transport("the file is larger than 20 MB".into()));
        }
        Ok(bytes.to_vec())
    }
}
