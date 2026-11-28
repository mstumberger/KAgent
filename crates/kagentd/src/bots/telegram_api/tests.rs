//! The real client against a fake of the Bot API on loopback. Nothing here leaves the
//! machine, and no test waits for a wall clock: the client's is a `FakeClock`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Value};

use super::*;

const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

/// What `getMe` answers for the bot.
fn me() -> Value {
    json!({"id": 1, "is_bot": true, "first_name": "KAgent", "username": "kagent_bot"})
}

/// A message the fake says was sent.
fn sent() -> Value {
    json!({"message_id": 7, "chat": {"id": 42, "type": "private"}, "date": 1700000000})
}

/// What the fake was asked: the method (for a file, `file <path>`) and the JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asked {
    method: String,
    body: Value,
}

/// The Bot API on loopback: every request is recorded, and answered from a script in
/// the order the requests come.
#[derive(Default)]
struct Fake {
    asked: Mutex<Vec<Asked>>,
    script: Mutex<VecDeque<(StatusCode, Vec<u8>)>>,
}

async fn handle(State(fake): State<Arc<Fake>>, uri: Uri, body: Bytes) -> Response {
    let path = uri.path();
    let bot = format!("/bot{TOKEN}/");
    let file = format!("/file/bot{TOKEN}/");
    let method = match path.strip_prefix(&file) {
        Some(rest) => format!("file {rest}"),
        None => path.strip_prefix(&bot).unwrap_or(path).to_string(),
    };
    fake.asked.lock().unwrap().push(Asked { method, body: serde_json::from_slice(&body).unwrap_or(Value::Null) });
    let (status, answer) = fake.script.lock().unwrap().pop_front().unwrap_or((StatusCode::IM_A_TEAPOT, br#"{"ok":false,"error_code":418,"description":"nothing is scripted"}"#.to_vec()));
    (status, [(axum::http::header::CONTENT_TYPE, "application/json")], answer).into_response()
}

fn ok(result: Value) -> (StatusCode, Vec<u8>) {
    (StatusCode::OK, json!({ "ok": true, "result": result }).to_string().into_bytes())
}

fn refused(status: u16, description: &str) -> (StatusCode, Vec<u8>) {
    (StatusCode::from_u16(status).unwrap(), json!({ "ok": false, "error_code": status, "description": description }).to_string().into_bytes())
}

fn busy(retry_after: u32) -> (StatusCode, Vec<u8>) {
    (StatusCode::TOO_MANY_REQUESTS, json!({ "ok": false, "error_code": 429, "description": "Too Many Requests", "parameters": { "retry_after": retry_after } }).to_string().into_bytes())
}

/// The fake, serving, and a client on it.
async fn started(fake: Arc<Fake>) -> (Arc<FakeClock>, TelegramClient) {
    let app = Router::new().fallback(handle).with_state(fake);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let clock = Arc::new(FakeClock::new());
    (clock.clone(), TelegramClient::new(&base, TOKEN, clock).unwrap())
}

fn asked(fake: &Fake) -> Vec<Asked> {
    fake.asked.lock().unwrap().clone()
}

#[tokio::test]
async fn get_me_and_one_long_poll() {
    let fake = Arc::new(Fake::default());
    fake.script.lock().unwrap().extend([ok(me()), ok(json!([])), ok(json!([]))]);
    let (clock, client) = started(fake.clone()).await;
    let me = client.get_me().await.unwrap();
    assert_eq!((me.username.as_deref(), me.is_bot), (Some("kagent_bot"), true));
    assert!(client.get_updates(Some(41), POLL_SEC).await.unwrap().is_empty());
    let asked = asked(&fake);
    assert_eq!(asked.iter().map(|a| a.method.as_str()).collect::<Vec<_>>(), ["getMe", "getUpdates"], "the method is the last part of the URL");
    assert_eq!(asked[1].body, json!({"offset": 41, "timeout": 30, "allowed_updates": ["message", "callback_query"]}), "the long poll of bots.md section 3");
    assert!(client.get_updates(None, POLL_SEC).await.unwrap().is_empty());
    let later = self::asked(&fake);
    assert_eq!(later[2].body, json!({"timeout": 30, "allowed_updates": ["message", "callback_query"]}), "no offset yet: none is named, not even as null");
    assert_eq!(clock.timeouts(), [CALL_TIMEOUT, POLL_HTTP_TIMEOUT, POLL_HTTP_TIMEOUT], "the long poll may take 10 s more than its 30 s; nothing else waits that long");
    assert_eq!((CALL_TIMEOUT, POLL_HTTP_TIMEOUT), (Duration::from_secs(30), Duration::from_secs(40)));
}

#[tokio::test]
async fn messages_buttons_callbacks_and_edits_go_as_the_bot_api_wants_them() {
    let fake = Arc::new(Fake::default());
    fake.script.lock().unwrap().extend([ok(sent()), ok(sent()), ok(sent()), ok(json!(true)), ok(json!(true))]);
    let (_clock, client) = started(fake.clone()).await;
    let keyboard = Keyboard::row([
        Button { text: "Allow once".into(), callback_data: "a:r1:allow".into() },
        Button { text: "Deny".into(), callback_data: "a:r1:deny".into() },
    ]);
    let sent = client.send_message(42, "<b>Hi</b>", Markup::Html, Some(keyboard)).await.unwrap();
    assert_eq!(sent.chat.id, 42);
    client.edit_message_text(42, 7, "Plain text", Markup::None, None).await.unwrap();
    client.send_message(42, "no keyboard", Markup::None, None).await.unwrap();
    assert!(client.answer_callback("cb-1", Some("Allowed")).await.unwrap());
    assert!(client.answer_callback("cb-2", None).await.unwrap(), "a press may be answered with no note");
    let bodies = asked(&fake).into_iter().map(|a| (a.method, a.body)).collect::<Vec<_>>();
    assert_eq!(
        bodies,
        vec![
            ("sendMessage".into(), json!({"chat_id": 42, "text": "<b>Hi</b>", "parse_mode": "HTML", "reply_markup": {"inline_keyboard": [[{"text": "Allow once", "callback_data": "a:r1:allow"}, {"text": "Deny", "callback_data": "a:r1:deny"}]]}})),
            ("editMessageText".into(), json!({"chat_id": 42, "text": "Plain text", "message_id": 7, "reply_markup": {"inline_keyboard": []}})),
            ("sendMessage".into(), json!({"chat_id": 42, "text": "no keyboard"}),),
            ("answerCallbackQuery".into(), json!({"callback_query_id": "cb-1", "text": "Allowed"})),
            ("answerCallbackQuery".into(), json!({"callback_query_id": "cb-2"})),
        ]
    );
}

#[tokio::test]
async fn retry_after_is_honoured() {
    let fake = Arc::new(Fake::default());
    fake.script.lock().unwrap().extend([busy(7), ok(sent()), busy(3), busy(3)]);
    let (clock, client) = started(fake.clone()).await;
    client.send_message(42, "once more", Markup::None, None).await.unwrap();
    assert_eq!(asked(&fake).len(), 2, "tried once more after the wait");
    assert_eq!(clock.slept(), [Duration::from_secs(7)], "as long as Telegram asked");
    assert_eq!(client.send_message(42, "again", Markup::None, None).await.unwrap_err(), ApiError::RetryAfter(3), "a second 429 in a row is the caller's to wait for");
    assert_eq!(clock.slept(), [Duration::from_secs(7), Duration::from_secs(3)], "and that wait was made too");
}

#[tokio::test]
async fn twenty_calls_a_minute() {
    let fake = Arc::new(Fake::default());
    fake.script.lock().unwrap().extend((0..21).map(|_| ok(me())));
    let (clock, client) = started(fake.clone()).await;
    for _ in 0..20 {
        client.get_me().await.unwrap();
    }
    assert!(clock.slept().is_empty(), "the first 20 go at once");
    client.get_me().await.unwrap();
    assert_eq!(clock.slept(), [Duration::from_secs(60)], "the 21st waits until the first is a minute old");
}

#[tokio::test]
async fn a_refused_call_is_an_error_that_says_what_telegram_said() {
    let fake = Arc::new(Fake::default());
    fake.script.lock().unwrap().extend([
        refused(401, "Unauthorized"),
        refused(400, "Bad Request: can't parse entities: Missing end tag"),
        (StatusCode::OK, b"\"not the API's JSON\"".to_vec()),
        (StatusCode::OK, json!({"ok": true}).to_string().into_bytes()),
    ]);
    let (_clock, client) = started(fake.clone()).await;
    assert_eq!(client.get_me().await.unwrap_err(), ApiError::Api { status: 401, description: "Unauthorized".into() });
    let parse = client.send_message(42, "<b>x", Markup::Html, None).await.unwrap_err();
    assert!(unparsable(&parse), "{parse:?}");
    assert!(!unparsable(&ApiError::Api { status: 401, description: "Unauthorized".into() }), "only a text Telegram could not parse");
    assert_eq!(client.get_me().await.unwrap_err(), ApiError::Transport("getMe: the answer is not the Bot API's JSON (200 OK): \"not the API's JSON\"".into()));
    assert_eq!(client.get_me().await.unwrap_err(), ApiError::Transport("getMe: the answer has no result".into()));
}

#[tokio::test]
async fn a_file_is_fetched_only_through_a_plain_relative_path() {
    let fake = Arc::new(Fake::default());
    fake.script.lock().unwrap().extend([ok(json!({"file_id": "f1", "file_path": "voice/file_1.oga"})), (StatusCode::OK, vec![1, 2, 3])]);
    let (_clock, client) = started(fake.clone()).await;
    let file = client.get_file("f1").await.unwrap();
    assert_eq!(file.file_path.as_deref(), Some("voice/file_1.oga"));
    assert_eq!(client.download("voice/file_1.oga").await.unwrap(), vec![1, 2, 3]);
    assert_eq!(asked(&fake)[1].method, "file voice/file_1.oga", "under /file/bot<token>/");
    for bad in ["https://evil.example/x", "../secret", "/etc/passwd", "a/../b", ""] {
        assert!(matches!(client.download(bad).await, Err(ApiError::Transport(_))), "{bad:?}");
    }
    assert_eq!(asked(&fake).len(), 2, "a path that is refused is never requested");
}

#[test]
fn a_token_is_checked_before_it_goes_into_a_url() {
    assert!(check_token(TOKEN).is_ok());
    for bad in [
        "",
        "123456789",
        "123456789:",
        ":AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw",
        "a:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw",
        "123456789:../../etc/passwd",
        "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw?x=1",
        "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw/other",
        "123456789:AAHd qTcvCH1vGWJxfSeofSAs0K5PALDsaw",
        "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw\n",
        "123456789:short",
    ] {
        let refused = check_token(bad).unwrap_err();
        assert!(refused.starts_with("a bot token is"), "{bad:?}: {refused}");
    }
    // With an edit, a message loses the buttons it had (the outcome of a card replaces them).
    assert_eq!(text_args(42, "x", Markup::None, None, true)["reply_markup"], json!({ "inline_keyboard": [] }));
    assert!(text_args(42, "x", Markup::None, None, false).get("reply_markup").is_none(), "a new message says nothing about a keyboard it has not");
}

#[tokio::test]
async fn the_token_is_in_no_error_and_no_other_address_is_used() {
    let clock = Arc::new(FakeClock::new());
    // A loopback port nobody listens on: the connection is refused, and the error must
    // not carry the URL (reqwest's own message does).
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let client = TelegramClient::new(&format!("http://{dead}"), TOKEN, clock.clone()).unwrap();
    assert!(TelegramClient::new(API_BASE, "12:../x", clock.clone()).unwrap_err().starts_with("a bot token is"), "the token is checked before any request");
    assert!(!format!("{client:?}").contains(TOKEN), "the debug print names the base only: {client:?}");
    let error = client.get_me().await.unwrap_err();
    assert!(matches!(error, ApiError::Transport(_)), "{error:?}");
    let text = format!("{error:?} {error}");
    assert!(!text.contains(TOKEN) && !text.contains("AAHdq"), "the token is in the URL, and the URL in no error: {text}");

    // A bounded call that does not end is the clock's: the future is dropped untouched.
    clock.expire_timeouts(true);
    let client = TelegramClient::new(API_BASE, TOKEN, clock).unwrap();
    assert_eq!(client.get_me().await.unwrap_err(), ApiError::Transport("getMe: Telegram did not answer in time".into()));

    for base in [API_BASE, "https://api.telegram.org/", "http://127.0.0.1:8000", "http://localhost:1", "http://[::1]:9000"] {
        assert!(TelegramClient::new(base, TOKEN, Arc::new(FakeClock::new())).is_ok(), "{base}");
    }
    for base in [
        "http://api.telegram.org",
        "http://10.0.0.1:8080",
        "ftp://api.telegram.org",
        "api.telegram.org",
        "https://user:pass@api.telegram.org",
        "file:///etc/passwd",
        "",
    ] {
        let refused = TelegramClient::new(base, TOKEN, Arc::new(FakeClock::new())).unwrap_err();
        assert!(refused.contains("refused base address"), "{base}: {refused}");
    }
}

#[tokio::test]
async fn updates_of_every_kind_are_read_and_the_rest_is_left() {
    // Fixtures in the shape Telegram sends (bots.md section 9).
    let text: Update = serde_json::from_value(json!({"update_id": 10, "message": {"message_id": 1, "from": me(), "chat": {"id": 42, "type": "private"}, "date": 1700000000, "text": "how is the disk?"}})).unwrap();
    let command: Update = serde_json::from_value(json!({"update_id": 11, "message": {"message_id": 2, "from": me(), "chat": {"id": 42, "type": "private"}, "date": 1700000001, "text": "/start 123456"}})).unwrap();
    let callback: Update = serde_json::from_value(json!({"update_id": 12, "callback_query": {"id": "cb", "from": me(), "message": {"message_id": 7, "chat": {"id": 42, "type": "private"}, "date": 1700000000, "text": "card"}, "data": "a:r1:allow"}})).unwrap();
    let voice: Update = serde_json::from_value(json!({"update_id": 13, "message": {"message_id": 3, "from": me(), "chat": {"id": 42, "type": "private"}, "date": 1700000002, "voice": {"file_id": "f1", "file_unique_id": "u1", "duration": 4, "mime_type": "audio/ogg", "file_size": 12345}}})).unwrap();
    let photo: Update = serde_json::from_value(json!({"update_id": 14, "message": {"message_id": 4, "from": me(), "chat": {"id": 42, "type": "private"}, "date": 1700000003, "photo": [{"file_id": "p", "width": 1, "height": 1}], "caption": "what is this?"}})).unwrap();
    let unsupported: Update = serde_json::from_value(json!({"update_id": 15, "edited_message": {"message_id": 1, "chat": {"id": 42}, "date": 1700000000, "text": "typo"}})).unwrap();
    let my_chat: Update = serde_json::from_value(json!({"update_id": 16, "my_chat_member": {"chat": {"id": 42}, "date": 1700000000}})).unwrap();

    let message = text.message.as_ref().unwrap();
    assert_eq!((message.message_id, message.chat.id, message.text.as_deref(), message.from.as_ref().unwrap().username.as_deref()), (1, 42, Some("how is the disk?"), Some("kagent_bot")));
    assert!(!message.is_attachment() && message.voice.is_none(), "text only");
    assert_eq!(command.message.as_ref().unwrap().text.as_deref(), Some("/start 123456"));
    let query = callback.callback_query.as_ref().unwrap();
    assert_eq!((query.id.as_str(), query.data.as_deref(), query.message.as_ref().unwrap().chat.id), ("cb", Some("a:r1:allow"), 42));
    let note = voice.message.as_ref().unwrap().voice.as_ref().unwrap();
    assert_eq!((note.file_id.as_str(), note.mime_type.as_deref()), ("f1", Some("audio/ogg")));
    assert!(!voice.message.as_ref().unwrap().is_attachment(), "a voice note is not an attachment here: it is transcribed");
    let shot = photo.message.as_ref().unwrap();
    assert!(!shot.is_attachment() && shot.text.is_none() && !shot.photo.is_empty(), "a photo is read as an image (vision.md section 2), and its caption is not text");
    assert_eq!(shot.caption.as_deref(), Some("what is this?"));
    for update in [&text, &command, &callback, &voice, &photo] {
        assert_eq!(update.message.is_some() || update.callback_query.is_some(), true, "a kind the bot uses");
    }
    for update in [&unsupported, &my_chat] {
        assert!(update.message.is_none() && update.callback_query.is_none(), "a kind the bot drops: {}", update.update_id);
    }
}
