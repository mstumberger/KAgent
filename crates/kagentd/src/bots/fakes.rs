//! Fakes for the tests of the bots module: Telegram and the agent as the bot sees them,
//! and a clock a test moves by hand. Nothing here waits for a wall clock: what the bot
//! asks for is recorded, a test waits for a record to appear (for 5 s at most, so a test
//! that hangs fails), and a sleep returns at once unless the test holds it.

use std::collections::VecDeque;
use std::pin::Pin;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use kagent_core::permission::{ApprovalDecision, ApprovalRequest};
use kagent_core::protocol::TurnStatus;
use tokio::sync::{watch, Notify};

use super::telegram_api::{ApiError, CallbackQuery, Chat, Clock, Keyboard, Markup, Message, RemoteFile, TelegramApi, Update, User};
use super::voice::Transcriber;
use super::{BotAgent, Status, TurnEnd};

/// Never longer than this does a test wait for something the bot should have done.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// One message the bot sent, or one edit of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub chat: i64,
    /// The message that was edited; `None` for a new message.
    pub edit: Option<i64>,
    pub text: String,
    pub markup: Markup,
    pub keyboard: Option<Keyboard>,
}

/// Telegram as the bot sees it: everything it is asked is recorded, and it answers as
/// if all went well, unless a script says otherwise.
#[derive(Default)]
pub struct FakeApi {
    pub sent: Mutex<Vec<Sent>>,
    /// What `getUpdates` answers, call by call. With nothing waiting, a call waits
    /// for `push`: the poller of a test never spins.
    pub updates: Mutex<VecDeque<Result<Vec<Update>, ApiError>>>,
    /// What `getMe` answers; `None`: a bot named `kagent_bot`.
    pub me: Mutex<Option<Result<User, ApiError>>>,
    /// What `getFile` answers, and what `download` then gives.
    pub file: Mutex<(String, Vec<u8>)>,
    /// The paths `download` was asked for.
    pub downloads: Mutex<Vec<String>>,
    pub answered: Mutex<Vec<(String, Option<String>)>>,
    /// The offset of every `getUpdates` call.
    pub offsets: Mutex<Vec<Option<i64>>>,
    /// The next message in HTML is refused as Telegram refuses text it cannot parse.
    pub refuse_html_once: AtomicBool,
    ids: AtomicI64,
    pushed: Notify,
    count: watch::Sender<usize>,
}

impl FakeApi {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { ids: AtomicI64::new(100), count: watch::Sender::new(0), ..Default::default() })
    }

    fn next(&self) -> Message {
        Message { message_id: self.ids.fetch_add(1, Ordering::SeqCst), chat: Chat { id: 0, ..Default::default() }, ..Default::default() }
    }

    fn record(&self, sent: Sent) -> Message {
        self.sent.lock().unwrap().push(sent);
        self.count.send_modify(|n| *n += 1);
        self.next()
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    /// Every text the chat was shown, in order.
    pub fn texts(&self) -> Vec<String> {
        self.sent().into_iter().map(|s| s.text).collect()
    }

    /// The id of the last message the fake gave out (the next one is higher).
    pub fn last_id(&self) -> i64 {
        self.ids.load(Ordering::SeqCst) - 1
    }

    /// Hand the poller this answer of `getUpdates`.
    pub fn push(&self, answer: Result<Vec<Update>, ApiError>) {
        self.updates.lock().unwrap().push_back(answer);
        self.pushed.notify_one();
    }

    pub fn update(&self, update: Update) {
        self.push(Ok(vec![update]));
    }

    /// Wait until a message or an edit meets `what`; false when none does for 5 s.
    pub async fn until(&self, what: impl Fn(&Sent) -> bool) -> bool {
        let mut count = self.count.subscribe();
        tokio::time::timeout(PATIENCE, async {
            loop {
                if self.sent.lock().unwrap().iter().any(&what) {
                    return true;
                }
                if count.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false)
    }
}

#[async_trait]
impl TelegramApi for FakeApi {
    async fn get_me(&self) -> Result<User, ApiError> {
        self.me.lock().unwrap().clone().unwrap_or_else(|| Ok(bot()))
    }

    async fn get_updates(&self, offset: Option<i64>, _timeout_sec: u32) -> Result<Vec<Update>, ApiError> {
        self.offsets.lock().unwrap().push(offset);
        loop {
            // Taken and returned in one step: a call dropped in between loses nothing.
            if let Some(answer) = self.updates.lock().unwrap().pop_front() {
                return answer;
            }
            self.pushed.notified().await;
        }
    }

    async fn send_message(&self, chat_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>) -> Result<Message, ApiError> {
        if markup == Markup::Html && self.refuse_html_once.swap(false, Ordering::SeqCst) {
            return Err(ApiError::Api { status: 400, description: "Bad Request: can't parse entities: Unsupported start tag".into() });
        }
        let mut message = self.record(Sent { chat: chat_id, edit: None, text: text.to_string(), markup, keyboard });
        message.chat.id = chat_id;
        Ok(message)
    }

    async fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str, markup: Markup, keyboard: Option<Keyboard>) -> Result<Message, ApiError> {
        if markup == Markup::Html && self.refuse_html_once.swap(false, Ordering::SeqCst) {
            return Err(ApiError::Api { status: 400, description: "Bad Request: can't parse entities: Unsupported start tag".into() });
        }
        self.record(Sent { chat: chat_id, edit: Some(message_id), text: text.to_string(), markup, keyboard });
        Ok(Message { message_id, chat: Chat { id: chat_id, ..Default::default() }, ..Default::default() })
    }

    async fn answer_callback(&self, callback_id: &str, text: Option<&str>) -> Result<bool, ApiError> {
        self.answered.lock().unwrap().push((callback_id.to_string(), text.map(str::to_string)));
        Ok(true)
    }

    async fn get_file(&self, _file_id: &str) -> Result<RemoteFile, ApiError> {
        Ok(RemoteFile { file_path: Some(self.file.lock().unwrap().0.clone()), ..Default::default() })
    }

    async fn download(&self, path: &str) -> Result<Vec<u8>, ApiError> {
        self.downloads.lock().unwrap().push(path.to_string());
        Ok(self.file.lock().unwrap().1.clone())
    }
}

/// Who sends in the fixtures: the owner, through the bot.
pub fn me() -> User {
    User { id: 7, is_bot: false, first_name: "Marko".into(), username: Some("marko".into()) }
}

/// The bot itself, as `getMe` answers when nothing is scripted.
pub fn bot() -> User {
    User { id: 1, is_bot: true, first_name: "KAgent".into(), username: Some("kagent_bot".into()) }
}

/// A text message of `chat`, as Telegram sends it.
pub fn text(update_id: i64, chat: i64, text: &str) -> Update {
    Update {
        update_id,
        message: Some(Message { message_id: update_id + 500, from: Some(me()), chat: Chat { id: chat, kind: "private".into(), ..Default::default() }, date: 1700000000, text: Some(text.into()), ..Default::default() }),
        ..Default::default()
    }
}

/// A voice note of `chat`.
pub fn voice(update_id: i64, chat: i64, file_id: &str) -> Update {
    let mut update = text(update_id, chat, "");
    let message = update.message.as_mut().unwrap();
    message.text = None;
    message.voice = Some(super::telegram_api::Voice { file_id: file_id.into(), duration: 4, mime_type: Some("audio/ogg".into()), file_size: Some(1234) });
    update
}

/// A press on a button under `message` of `chat`.
pub fn press(update_id: i64, chat: i64, message: i64, data: &str) -> Update {
    Update {
        update_id,
        callback_query: Some(CallbackQuery {
            id: format!("cb{update_id}"),
            from: Some(me()),
            message: Some(Message { message_id: message, chat: Chat { id: chat, kind: "private".into(), ..Default::default() }, date: 1700000000, ..Default::default() }),
            data: Some(data.into()),
        }),
        ..Default::default()
    }
}

/// Wait until `what` holds; false when it does not for 5 s. For what is not a message
/// (the offset of the poller, a call on the agent).
pub async fn eventually(mut what: impl FnMut() -> bool) -> bool {
    tokio::time::timeout(PATIENCE, async {
        while !what() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

/// The clock of a test: `now` moves by `advance` and by every sleep. A sleep returns at
/// once, and is recorded, unless the test holds sleeps: then each one waits for `release`.
pub struct GateClock {
    now: Mutex<Instant>,
    sleeps: Mutex<Vec<Duration>>,
    hold: AtomicBool,
    released: watch::Sender<u64>,
}

impl GateClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { now: Mutex::new(Instant::now()), sleeps: Mutex::new(Vec::new()), hold: AtomicBool::new(false), released: watch::Sender::new(0) })
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }

    /// How long the bot asked to wait, each time.
    pub fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }

    /// From now on every sleep waits until `release`.
    pub fn hold(&self, yes: bool) {
        self.hold.store(yes, Ordering::SeqCst);
    }

    /// Let the sleeps that wait go on.
    pub fn release(&self) {
        self.released.send_modify(|n| *n += 1);
    }
}

#[async_trait]
impl Clock for GateClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    async fn sleep(&self, wait: Duration) {
        let mut released = self.released.subscribe();
        let before = *released.borrow();
        self.sleeps.lock().unwrap().push(wait);
        if self.hold.load(Ordering::SeqCst) {
            while *released.borrow_and_update() <= before {
                if released.changed().await.is_err() {
                    break;
                }
            }
        }
        self.advance(wait);
    }

    async fn within(&self, _wait: Duration, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> bool {
        future.await;
        true
    }
}

/// The agent as the bot sees it: what it was asked is recorded, and it is scripted.
#[derive(Default)]
pub struct FakeAgent {
    /// Every session it made: (id, title).
    pub sessions: Mutex<Vec<(String, String)>>,
    /// Every message of a turn: (session, text).
    pub sends: Mutex<Vec<(String, String)>>,
    /// Every message with pictures: (session, text, the size of each picture).
    pub image_sends: Mutex<Vec<(String, String, Vec<usize>)>>,
    pub resolved: Mutex<Vec<(String, ApprovalDecision)>>,
    pub acked: Mutex<Vec<String>>,
    pub stopped: AtomicUsize,
    pub status: Mutex<Status>,
    pub pending: Mutex<Vec<ApprovalRequest>>,
    pub transcriber: Option<Arc<dyn Transcriber>>,
    /// What `wait_idle` answers, turn by turn; `None`: the turn completed with that text.
    pub ends: Mutex<VecDeque<(Option<TurnStatus>, String)>>,
    /// While set, a turn runs and `wait_idle` does not return.
    pub running: AtomicBool,
    pub release: Notify,
}

impl FakeAgent {
    /// How the next turn ends.
    pub fn ends_with(&self, status: TurnStatus, text: &str) {
        self.ends.lock().unwrap().push_back((Some(status), text.to_string()));
    }

    pub fn sends(&self) -> Vec<(String, String)> {
        self.sends.lock().unwrap().clone()
    }

    pub fn resolved(&self) -> Vec<(String, ApprovalDecision)> {
        self.resolved.lock().unwrap().clone()
    }
}

#[async_trait]
impl BotAgent for FakeAgent {
    async fn new_session(&self, title: &str) -> anyhow::Result<String> {
        let mut sessions = self.sessions.lock().unwrap();
        let id = format!("s{}", sessions.len() + 1);
        sessions.push((id.clone(), title.to_string()));
        Ok(id)
    }

    async fn session_exists(&self, session: &str) -> bool {
        self.sessions.lock().unwrap().iter().any(|(id, _)| id == session)
    }

    async fn send(&self, session: &str, text: &str) -> anyhow::Result<()> {
        self.sends.lock().unwrap().push((session.to_string(), text.to_string()));
        Ok(())
    }

    async fn send_images(&self, session: &str, text: &str, photos: Vec<Vec<u8>>) -> anyhow::Result<()> {
        self.image_sends.lock().unwrap().push((session.to_string(), text.to_string(), photos.iter().map(Vec::len).collect()));
        Ok(())
    }

    async fn wait_idle(&self, _session: &str) -> TurnEnd {
        while self.running.load(Ordering::SeqCst) {
            if tokio::time::timeout(Duration::from_millis(20), self.release.notified()).await.is_ok() {
                break;
            }
        }
        let (status, text) = self.ends.lock().unwrap().pop_front().unwrap_or((None, String::new()));
        match status {
            None => TurnEnd { status: TurnStatus::Completed, error: None, text },
            Some(TurnStatus::Failed) => TurnEnd { status: TurnStatus::Failed, error: Some(text), text: String::new() },
            Some(status) => TurnEnd { status, error: None, text },
        }
    }

    async fn stop_all(&self) -> usize {
        let stopped = self.stopped.load(Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
        stopped
    }

    fn resolve(&self, request: &str, decision: ApprovalDecision) -> bool {
        let mut pending = self.pending.lock().unwrap();
        if !pending.iter().any(|card| card.request_id == request) {
            return false;
        }
        pending.retain(|card| card.request_id != request);
        self.resolved.lock().unwrap().push((request.to_string(), decision));
        true
    }

    fn pending(&self) -> Vec<ApprovalRequest> {
        self.pending.lock().unwrap().clone()
    }

    async fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn ack_alert(&self, id: &str) -> bool {
        self.acked.lock().unwrap().push(id.to_string());
        true
    }

    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        self.transcriber.clone()
    }
}

/// A transcriber with a fixed answer.
pub struct FakeTranscriber(pub &'static str);

#[async_trait]
impl Transcriber for FakeTranscriber {
    async fn transcribe(&self, _audio: Vec<u8>) -> anyhow::Result<String> {
        Ok(self.0.to_string())
    }
}
