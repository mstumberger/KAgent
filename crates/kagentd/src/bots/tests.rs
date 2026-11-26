//! The whole road (bots.md): the work of the handlers over `Bots`, the polling task, the
//! forwarding of cards, the Telegram channel and the push of alerts, with the fakes of
//! `fakes.rs` and no server.

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use kagent_core::permission::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use kagent_core::protocol::{AgentEvent, TurnStatus};
use kagent_core::tool::Risk;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::fakes::{eventually, press, text, FakeAgent, FakeApi, GateClock};
use super::telegram_api::{ApiError, Clock, TelegramApi};
use super::{cards, forward, poll, Bots, Pairing, Parts, PollState, Settings, HttpError, TokenStore};
use crate::notify::{Channel, Notification, Notifier, Urgency};
use crate::watch::{AlertManager, Hit, Severity};

mod routes;

const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";
const OTHER: &str = "987654321:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

/// `unwrap` for what the routes answer: their error has no `Debug`.
fn ok<T>(result: Result<T, HttpError>) -> T {
    result.unwrap_or_else(|e| panic!("refused: {} {} {}", e.0, e.1, e.2))
}

/// The refusal of the routes: its status, code and message.
fn refusal<T>(result: Result<T, HttpError>) -> (StatusCode, &'static str, String) {
    match result {
        Ok(_) => panic!("not refused"),
        Err(e) => (e.0, e.1, e.2),
    }
}

struct Rig {
    bots: Arc<Bots>,
    api: Arc<FakeApi>,
    agent: Arc<FakeAgent>,
    clock: Arc<GateClock>,
    events: broadcast::Sender<AgentEvent>,
    quiet: Arc<Mutex<bool>>,
    notifier: Arc<Notifier>,
    alerts: Arc<AlertManager>,
    dir: tempfile::TempDir,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let api = FakeApi::new();
    let agent = Arc::new(FakeAgent::default());
    let clock = GateClock::new();
    let quiet = Arc::new(Mutex::new(false));
    let (for_bots, for_maker) = (quiet.clone(), api.clone());
    let (events, _) = broadcast::channel(1024);
    let bots = Bots::new(Parts {
        agent: agent.clone(),
        maker: Arc::new(move |_| Ok(for_maker.clone() as Arc<dyn TelegramApi>)),
        clock: clock.clone(),
        tokens: TokenStore::File(dir.path().join("telegram.secret")),
        settings: Arc::new(RwLock::new(Settings::defaults())),
        settings_path: dir.path().join("settings.json"),
        config_dir: dir.path().to_path_buf(),
        events: events.clone(),
        quiet: Arc::new(move || *for_bots.lock().unwrap()),
    })
    .await;
    let alerts = AlertManager::new(dir.path().join("alerts.json"), events.clone(), None, false);
    Rig {
        bots,
        api,
        agent,
        clock,
        events,
        quiet,
        notifier: Notifier::new(),
        alerts,
        dir,
    }
}

impl Rig {
    /// What the daemon does at its start.
    fn start(&self) {
        self.bots.start(self.notifier.clone(), self.alerts.clone());
    }

    /// On, with a token, and paired with chat 42 through `/start <code>`.
    async fn paired(self, settings: Value) -> Rig {
        ok(self.bots.set_token(TOKEN).await);
        let mut patch = json!({ "enabled": true });
        for (key, value) in settings.as_object().into_iter().flatten() {
            patch[key] = value.clone();
        }
        ok(self.bots.patch(patch));
        self.start();
        let code = ok(self.bots.pairing_code());
        self.api.update(text(1, 42, &format!("/start {}", code["code"].as_str().unwrap())));
        assert!(self.api.until(|s| s.text.starts_with("Paired.")).await, "{:?}", self.api.sent());
        assert!(eventually(|| self.bots.known().status == PollState::Polling).await);
        self
    }

    fn view(&self) -> Value {
        self.bots.view()
    }

    fn sent(&self) -> usize {
        self.api.sent().len()
    }

    fn cards(&self) -> usize {
        self.api.sent().iter().filter(|s| s.keyboard.is_some()).count()
    }

    /// Let the tasks the events started run, so that none is left behind.
    async fn settle(&self) {
        for _ in 0..200 {
            tokio::task::yield_now().await;
        }
    }
}

fn card(request: &str, destructive: bool, privileged: bool) -> ApprovalRequest {
    ApprovalRequest {
        request_id: request.into(),
        session_id: "s1".into(),
        call_id: "c1".into(),
        tool: "shell".into(),
        kind: ApprovalKind::Tool,
        input: json!({"command": "systemctl restart kagent"}),
        summary: "restart the kagent service".into(),
        reason: "changes system state".into(),
        risk: Risk::Medium,
        privileged,
        destructive,
        can_remember: true,
        rule_key: "shell".into(),
        preview: None,
    }
}

/// A card that asks for a plan: approved on the desktop only.
fn plan_card(request: &str) -> ApprovalRequest {
    let mut card = card(request, false, false);
    card.kind = ApprovalKind::Plan;
    card
}

fn hit(key: &str, severity: Severity) -> Hit {
    Hit { rule: "r".into(), key: key.into(), severity, title: "t".into(), cause: "c".into(), evidence: "e".into(), proposal: "p".into(), threshold: None, cooldown: Duration::from_secs(3600) }
}

/// The `telegramStatus` events that came, as `status:detail`.
fn statuses(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::TelegramStatus { status, detail } = event {
            out.push(format!("{status}:{detail}"));
        }
    }
    out
}

#[tokio::test]
async fn without_a_token_nothing_is_asked_of_telegram() {
    let none = poll::NoToken;
    assert!(matches!(none.get_me().await, Err(ApiError::Transport(_))));
    assert!(none.send_message(1, "x", super::telegram_api::Markup::None, None).await.is_err());
    assert!(none.get_updates(None, 30).await.is_err());
}

#[tokio::test]
async fn nothing_configured_is_off_and_a_token_keeps_the_bot_switched_off() {
    let r = rig().await;
    assert_eq!(r.bots.known().status, PollState::Off);
    assert_eq!(
        r.view(),
        json!({"enabled": false, "configured": false, "status": "off", "botUsername": null, "paired": false, "chatId": null, "pairedAt": null, "lastError": null, "pendingCode": null}),
        "off; the answer says whether there is a token, never the token"
    );
    assert_eq!(refusal(r.bots.pairing_code()).2, "no bot token: paste the token of your bot first");
    assert_eq!(r.bots.test_message().await["ok"], false);

    // What is no token is refused before Telegram is asked.
    assert!(r.bots.set_token("../x").await.is_err());
    assert!(r.bots.set_token("   ").await.is_err());
    assert!(!r.bots.has_token());
    // A token Telegram refuses is kept by nobody; nor is one Telegram did not answer for.
    *r.api.me.lock().unwrap() = Some(Err(ApiError::Api { status: 401, description: "Unauthorized".into() }));
    let refused = refusal(r.bots.set_token(OTHER).await);
    assert_eq!((refused.0, refused.1, refused.2.as_str()), (StatusCode::BAD_REQUEST, "invalid_input", "Telegram refused the token: Telegram answered 401: Unauthorized"));
    *r.api.me.lock().unwrap() = Some(Err(ApiError::Transport("connection refused".into())));
    assert_eq!(refusal(r.bots.set_token(OTHER).await).0, StatusCode::BAD_GATEWAY);
    assert!(!r.bots.has_token() && r.bots.known().status == PollState::Off);

    *r.api.me.lock().unwrap() = None;
    assert_eq!(ok(r.bots.set_token(TOKEN).await), "kagent_bot");
    assert!(!r.view().to_string().contains(TOKEN), "the token is in no answer");
    let view = r.view();
    assert_eq!((view["configured"].as_bool(), view["status"].as_str(), view["botUsername"].as_str(), view["enabled"].as_bool()), (Some(true), Some("disabled"), Some("kagent_bot"), Some(false)));
    assert_eq!(refusal(r.bots.pairing_code()).2, "Telegram is switched off");
    assert_eq!(r.bots.test_message().await["error"].as_str().unwrap(), "the bot is off, has no token or no paired chat");
}

#[tokio::test]
async fn a_message_of_the_paired_chat_starts_a_turn_through_the_polling_task() {
    let r = rig().await.paired(json!({})).await;
    assert_eq!((r.view()["chatId"].as_i64(), r.view()["status"].as_str(), r.view()["botUsername"].as_str()), (Some(42), Some("polling"), Some("kagent_bot")));
    assert!(r.view()["pairedAt"].as_str().unwrap().starts_with("20"), "since when");
    assert_eq!(refusal(r.bots.pairing_code()).1, "conflict", "one chat is paired");
    assert_eq!(r.bots.test_message().await, json!({"ok": true}));
    assert_eq!(r.api.texts().last().unwrap(), "Test message from KAgent: Telegram is connected.");

    // Scenario S29 item 3: a text from the phone is a turn, answered in Telegram.
    r.agent.ends_with(TurnStatus::Completed, "All **good**.");
    r.api.update(text(2, 42, "how is the disk?"));
    assert!(r.api.until(|s| s.text == "All <b>good</b>.").await, "{:?}", r.api.sent());
    assert_eq!(r.agent.sends(), [("s1".to_string(), "how is the disk?".to_string())]);
    assert_eq!(*r.agent.sessions.lock().unwrap(), [("s1".to_string(), "Telegram".to_string())]);

    // Forwarding is off by default: a card does not go to the chat.
    r.agent.pending.lock().unwrap().push(card("early", false, false));
    r.events.send(AgentEvent::ApprovalRequested(card("early", false, false))).unwrap();
    r.settle().await;
    assert!(r.clock.sleeps().is_empty() && r.cards() == 0);

    // A message from a second chat gets no answer, and starts no turn.
    let before = r.sent();
    r.api.update(text(3, 43, "let me in"));
    assert!(eventually(|| r.bots.bot.offset() == Some(4)).await, "the update was handled, and dropped");
    assert_eq!((r.sent(), r.agent.sends().len()), (before, 1));
    assert_eq!(*r.api.offsets.lock().unwrap().last().unwrap(), Some(4), "the next poll asks for what follows the last update handled");

    // Unpair: the chat is forgotten, nothing more goes to it, and a new chat can pair.
    r.bots.unpair().unwrap();
    assert_eq!((r.view()["paired"].clone(), r.view()["chatId"].clone()), (json!(false), Value::Null));
    assert!(eventually(|| r.bots.known().status == PollState::Unpaired).await);
    assert_eq!(r.bots.test_message().await["ok"], false);
    assert!(!Pairing::path(r.dir.path()).exists());
}

/// Scenario S29 item 4: a pending card arrives with buttons, and Allow once resolves it.
#[tokio::test]
async fn an_approval_card_is_forwarded_after_its_delay_and_a_button_resolves_it() {
    let r = rig().await.paired(json!({"forwardApprovals": true, "approvalForwardSec": 5})).await;
    let request = "6f1c2a0e-9b7d-4c3e-8a2f-0d1e2f3a4b5c";
    r.agent.pending.lock().unwrap().push(card(request, false, false));
    r.events.send(AgentEvent::ApprovalRequested(card(request, false, false))).unwrap();
    assert!(r.api.until(|s| s.keyboard.is_some()).await, "{:?}", r.api.sent());
    assert_eq!(r.clock.sleeps(), [Duration::from_secs(5)], "sent once it waited approvalForwardSec");
    let sent = r.api.sent().last().unwrap().clone();
    assert_eq!((sent.chat, sent.text.as_str()), (42, "KAgent needs your approval\n\nTool: shell\nrestart the kagent service\nRisk: medium\nWhy: changes system state"));
    let keyboard = sent.keyboard.clone().unwrap();
    let data = |label: &str| forward::button_data(&keyboard, label).unwrap();
    assert_eq!((data("Allow once"), data("Deny")), (format!("a:{request}:allow"), format!("a:{request}:deny")));

    // Allow once from the phone: the card is resolved and the message shows it.
    let message = r.api.last_id();
    r.api.update(press(2, 42, message, &data("Allow once")));
    assert!(eventually(|| r.agent.resolved().len() == 1).await);
    assert_eq!(r.agent.resolved()[0], (request.to_string(), ApprovalDecision::AllowOnce));
    assert!(r.api.until(|s| s.text.ends_with("Allowed once from Telegram.")).await);
    assert!(r.agent.pending.lock().unwrap().is_empty());
    // Pressed again: the card is gone, and nothing is resolved twice.
    r.api.update(press(3, 42, message, &data("Deny")));
    assert!(eventually(|| r.api.answered.lock().unwrap().len() == 2).await);
    assert_eq!(r.api.answered.lock().unwrap()[1], ("cb3".to_string(), Some("Already resolved".to_string())));
    assert_eq!(r.agent.resolved().len(), 1);

    // A card answered on the desktop is forgotten: a later press is told so, and edits nothing.
    r.agent.pending.lock().unwrap().push(card("second", false, false));
    r.events.send(AgentEvent::ApprovalRequested(card("second", false, false))).unwrap();
    assert!(eventually(|| r.cards() == 2).await, "{:?}", r.api.sent());
    r.events.send(AgentEvent::ApprovalResolved { request_id: "second".into(), decision: "allowOnce".into() }).unwrap();
    r.agent.pending.lock().unwrap().clear();
    r.settle().await;
    let before = r.sent();
    r.api.update(press(4, 42, r.api.last_id(), "a:second:allow"));
    assert!(eventually(|| r.api.answered.lock().unwrap().len() == 3).await);
    assert_eq!((r.api.answered.lock().unwrap()[2].1.as_deref(), r.sent(), r.agent.resolved().len()), (Some("Already resolved"), before, 1));
}

#[tokio::test]
async fn a_destructive_or_plan_card_is_never_forwarded_and_one_answered_on_the_desktop_is_not() {
    let r = rig().await.paired(json!({"forwardApprovals": true, "allowPrivileged": true})).await;
    r.clock.hold(true);
    for never in [card("d1", true, false), plan_card("p1"), card("d2", true, true)] {
        r.agent.pending.lock().unwrap().push(never.clone());
        r.events.send(AgentEvent::ApprovalRequested(never)).unwrap();
    }
    r.settle().await;
    assert!(r.clock.sleeps().is_empty() && r.cards() == 0, "destructive calls and plans are not waited for, not sent");

    // A plain card waits the default 20 s, and is not sent before that.
    r.agent.pending.lock().unwrap().push(card("plain", false, false));
    r.events.send(AgentEvent::ApprovalRequested(card("plain", false, false))).unwrap();
    assert!(eventually(|| !r.clock.sleeps().is_empty()).await);
    assert_eq!(r.clock.sleeps(), [Duration::from_secs(20)]);
    assert!(r.cards() == 0);
    // Answered on the desktop during the delay: it is not forwarded.
    r.agent.pending.lock().unwrap().retain(|c| c.request_id != "plain");
    let before = r.clock.now();
    r.clock.release();
    assert!(eventually(|| r.clock.now() > before).await, "the wait ended");
    assert!(r.cards() == 0, "the card answered on the desktop was not forwarded: {:?}", r.api.sent());

    // A root action goes, with allowPrivileged on: Allow once and Deny, never Always.
    r.agent.pending.lock().unwrap().push(card("root", false, true));
    r.events.send(AgentEvent::ApprovalRequested(card("root", false, true))).unwrap();
    assert!(eventually(|| r.clock.sleeps().len() == 2).await);
    assert!(r.cards() == 0, "not before its delay");
    r.clock.release();
    assert!(r.api.until(|s| s.keyboard.is_some()).await, "{:?}", r.api.sent());
    let forwarded = r.api.sent().last().cloned().unwrap();
    assert!(forwarded.text.contains("Tool: shell") && forwarded.text.ends_with("This is a root action."), "{}", forwarded.text);
    let keyboard = forwarded.keyboard.clone().unwrap();
    assert_eq!(keyboard.0.len(), 1, "one row of buttons");
    assert_eq!((forward::button_data(&keyboard, "Allow once"), forward::button_data(&keyboard, "Deny")), (Some("a:root:allow".to_string()), Some("a:root:deny".to_string())));
    assert!(forward::button_data(&keyboard, "Always").is_none());

    // allowPrivileged off again: a root card is not waited for either.
    ok(r.bots.patch(json!({"allowPrivileged": false})));
    for next in [card("root2", false, true), card("plain2", false, false)] {
        r.agent.pending.lock().unwrap().push(next.clone());
        r.events.send(AgentEvent::ApprovalRequested(next)).unwrap();
    }
    assert!(eventually(|| r.clock.sleeps().len() == 3).await, "only the plain card waited");
    r.clock.release();
    assert!(eventually(|| r.cards() == 2).await, "{:?}", r.api.sent());
}
