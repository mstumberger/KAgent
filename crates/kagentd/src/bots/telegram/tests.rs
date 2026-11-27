//! The bot of the paired chat, with a fake Telegram, a fake agent and a clock by hand.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use kagent_core::permission::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use kagent_core::protocol::TurnStatus;
use kagent_core::tool::Risk;
use serde_json::json;

use super::*;
use crate::bots::cards;
use crate::bots::fakes::{eventually, press, text, voice, FakeAgent, FakeApi, FakeTranscriber, GateClock};
use crate::bots::telegram::turns::final_text;
use crate::bots::telegram_api::Button;
use crate::bots::{Status, TurnEnd};

const CHAT: i64 = 42;
const T0: Duration = Duration::from_secs(1_800_000_000);

/// A moment far from the epoch, for the pairing code.
fn t0() -> SystemTime {
    SystemTime::UNIX_EPOCH + T0
}

fn now(after: u64) -> SystemTime {
    t0() + Duration::from_secs(after)
}

struct Rig {
    bot: Arc<Bot>,
    api: Arc<FakeApi>,
    agent: Arc<FakeAgent>,
    _clock: Arc<GateClock>,
    dir: tempfile::TempDir,
}

fn with(agent: FakeAgent) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let api = FakeApi::new();
    let agent = Arc::new(agent);
    let clock = GateClock::new();
    let bot = Bot::new(api.clone(), agent.clone(), clock.clone(), dir.path());
    Rig { bot, api, agent, _clock: clock, dir }
}

/// A bot whose chat `CHAT` is paired, as `/start <code>` leaves it.
fn paired() -> Rig {
    let r = with(FakeAgent::default());
    r.bot.set_username("kagent_bot");
    r.bot.pair(Pairing { bot_username: "kagent_bot".into(), chat_id: CHAT, user_id: 7, paired_at: "2026-11-26T10:00:00+00:00".into() }).unwrap();
    r.api.sent().clear();
    r
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

/// How many messages or edits show `text`.
fn showing(api: &FakeApi, text: &str) -> usize {
    api.sent().iter().filter(|s| s.text == text).count()
}

#[test]
fn commands_are_read_from_the_text() {
    assert_eq!(command("/start 123456", "kagent_bot"), Command::Start(Some("123456".into())));
    assert_eq!(command("/start@kagent_bot 42", "kagent_bot"), Command::Start(Some("42".into())));
    assert_eq!(command("  /new ", "kagent_bot"), Command::New);
    assert_eq!(command("/status@KAgent_Bot", "kagent_bot"), Command::Status, "the bot's name is not case-sensitive");
    assert_eq!(command("/stop", ""), Command::Stop, "nothing is known of the bot yet");
    assert_eq!(command("/help", "kagent_bot"), Command::Help);
    assert_eq!(command("/start", "kagent_bot"), Command::Start(None));
    assert_eq!(command("/status@another_bot", "kagent_bot"), Command::Other, "a command for another bot is left alone");
    assert_eq!(command("/plan the day", "kagent_bot"), Command::Text("/plan the day".into()), "an unknown command is text for the agent");
    assert_eq!(command(" how is the disk? ", "kagent_bot"), Command::Text("how is the disk?".into()));
    assert_eq!(status_text(&Status { doctor: Some(91), open_alerts: 2, running: 1 }), "Health: 91/100\nOpen alerts: 2\nRunning turns: 1");
    assert_eq!(status_text(&Status::default()), "Health: not checked now\nOpen alerts: 0\nRunning turns: 0");
}

#[test]
fn what_the_chat_gets_for_a_turn_that_ended() {
    let end = |status, text: &str| TurnEnd { status, error: None, text: text.into() };
    assert_eq!(final_text(&end(TurnStatus::Completed, " Fine. ")), "Fine.");
    assert_eq!(final_text(&end(TurnStatus::Completed, "  ")), "The agent gave no answer.");
    assert_eq!(final_text(&end(TurnStatus::Cancelled, "so far")), "Stopped.\n\nso far");
    assert_eq!(final_text(&end(TurnStatus::Cancelled, "")), "Stopped.");
    let mut failed = end(TurnStatus::Failed, "");
    failed.error = Some(" no model ".into());
    assert_eq!(final_text(&failed), "The turn failed: no model");
    failed.error = None;
    assert_eq!(final_text(&failed), "The turn failed: see the session in KAgent");
}

/// Scenario S29 item 1: the pairing table.
#[tokio::test]
async fn a_wrong_an_expired_or_a_used_code_does_not_pair_and_a_second_chat_is_refused() {
    let r = with(FakeAgent::default());
    r.bot.set_username("kagent_bot");
    // Nothing is paired and no code waits: a message is dropped before anything else.
    r.bot.handle(&text(1, CHAT, "hello?"), now(0)).await;
    assert!(r.api.sent().is_empty() && r.agent.sends().is_empty());
    r.bot.handle(&text(2, CHAT, "/start 000000"), now(0)).await;
    assert_eq!(r.api.texts(), ["No pairing code is waiting. Click \"Show pairing code\" in Settings > Telegram."]);

    let code = r.bot.ask_code(now(0));
    let wrong = if code.code == "000000" { "111111" } else { "000000" };
    r.bot.handle(&text(3, CHAT, &format!("/start {wrong}")), now(60)).await;
    assert_eq!(r.api.texts()[1], "That pairing code is not right.");
    assert!(r.bot.code(now(60)).is_some(), "a wrong code does not use the right one up");
    assert!(r.bot.paired().is_none());

    // Expired: ten minutes and a second later (the clock is the test's).
    r.bot.handle(&text(4, CHAT, &format!("/start {}", code.code)), now(601)).await;
    assert_eq!(r.api.texts()[2], "That pairing code has expired. Ask for a new one in Settings > Telegram.");
    assert!(r.bot.code(now(601)).is_none() && r.bot.paired().is_none());

    // The first chat with the right code pairs; the code is used up.
    let code = r.bot.ask_code(now(700));
    r.bot.handle(&text(5, 77, &format!("/start {}", code.code)), now(705)).await;
    assert!(r.api.texts()[3].starts_with("Paired. This chat now talks to KAgent."), "{}", r.api.texts()[3]);
    assert_eq!(r.bot.paired().map(|p| (p.chat_id, p.user_id)), Some((77, 7)));
    assert_eq!(Pairing::load(r.dir.path()).map(|p| p.chat_id), Some(77), "the pairing is on disk, for the next start");
    assert_eq!(Bot::new(r.api.clone(), r.agent.clone(), GateClock::new(), r.dir.path()).restore().map(|p| p.chat_id), Some(77));

    // Paired: another chat is dropped before anything else, even with the code.
    *r.agent.status.lock().unwrap() = Status { doctor: Some(91), open_alerts: 2, running: 1 };
    r.bot.handle(&text(6, 78, &format!("/start {}", code.code)), now(706)).await;
    assert_eq!(r.api.sent().len(), 4, "a second chat gets no answer");
    // `/start` in the paired chat answers with the status.
    r.bot.handle(&text(7, 77, "/start"), now(707)).await;
    let reply = &r.api.texts()[4];
    assert!(reply.starts_with("Paired with @kagent_bot since ") && reply.ends_with("\nHealth: 91/100\nOpen alerts: 2\nRunning turns: 1"), "{reply}");
    assert!(r.bot.code(now(707)).is_none(), "the code went the moment it was right");

    // Unpair forgets the chat and the file.
    r.bot.unpair().unwrap();
    assert!(r.bot.paired().is_none() && !Pairing::path(r.dir.path()).exists());
}

/// Decision of 2026-11-27 (bots.md section 2): five wrong codes make a new code and lock
/// pairing for ten minutes; then the new code pairs.
#[tokio::test]
async fn five_wrong_codes_make_a_new_code_and_lock_pairing_for_ten_minutes() {
    let r = with(FakeAgent::default());
    let old = r.bot.ask_code(now(0));
    let wrong = if old.code == "000000" { "111111" } else { "000000" };
    for i in 0..WRONG_CODES - 1 {
        r.bot.handle(&text(i64::from(i) + 1, CHAT, &format!("/start {wrong}")), now(10)).await;
    }
    assert_eq!(r.api.texts().last().map(String::as_str), Some("That pairing code is not right."));
    assert!(r.bot.pairing_locked_until(now(10)).is_none(), "four wrong codes do not lock");
    r.bot.handle(&text(10, CHAT, &format!("/start {wrong}")), now(20)).await;
    assert_eq!(r.api.texts().last().map(String::as_str), Some("Too many wrong pairing codes. Pairing is locked for 10 min; then use the new code shown in Settings > Telegram."));
    assert_eq!(r.bot.pairing_locked_until(now(20)), Some(now(20) + PAIRING_LOCK));
    let fresh = r.bot.code(now(20)).expect("a new code waits");
    assert_eq!(fresh.expires_at, now(20) + PAIRING_LOCK + CODE_TTL, "good for ten minutes after the lock");

    // Locked: not even the right code is looked at.
    r.bot.handle(&text(11, CHAT, &format!("/start {}", fresh.code)), now(300)).await;
    assert!(r.api.texts().last().unwrap().starts_with("Too many wrong pairing codes. Pairing is locked for 6 min"), "{:?}", r.api.texts().last());
    assert!(r.bot.paired().is_none());
    // The old code is gone even after the lock.
    if old.code != fresh.code {
        r.bot.handle(&text(12, CHAT, &format!("/start {}", old.code)), now(621)).await;
        assert!(r.bot.paired().is_none(), "the code before the lock pairs nothing");
    }
    // After the lock, the new code pairs.
    r.bot.handle(&text(13, CHAT, &format!("/start {}", fresh.code)), now(625)).await;
    assert_eq!(r.bot.paired().map(|p| p.chat_id), Some(CHAT), "{:?}", r.api.texts());
}

#[tokio::test]
async fn free_text_is_a_turn_of_the_chat_session_and_the_answer_replaces_the_placeholder() {
    let r = paired();
    r.agent.ends_with(TurnStatus::Completed, "**Disk**: 41% used\n\n::widget{id=\"s/1\"}");
    r.bot.handle(&text(1, CHAT, "  how is the disk? "), now(0)).await;
    assert!(r.api.until(|s| s.edit.is_some()).await, "{:?}", r.api.sent());
    let sent = r.api.sent();
    assert_eq!((sent[0].edit, sent[0].text.as_str(), sent[0].chat), (None, "Working on it...", CHAT), "at once");
    assert_eq!((sent[1].edit, sent[1].markup, sent[1].text.as_str()), (Some(100), Markup::Html, "<b>Disk</b>: 41% used"), "the final text replaces it: HTML, no directive line");
    assert_eq!(r.agent.sends(), [("s1".to_string(), "how is the disk?".to_string())]);
    assert_eq!(*r.agent.sessions.lock().unwrap(), [("s1".to_string(), "Telegram".to_string())]);

    // The session is reused until /new.
    r.agent.ends_with(TurnStatus::Completed, "Second.");
    r.bot.handle(&text(2, CHAT, "and now?"), now(1)).await;
    assert!(eventually(|| showing(&r.api, "Second.") == 1).await);
    r.bot.handle(&text(3, CHAT, "/new"), now(2)).await;
    assert_eq!(r.api.texts().last().unwrap(), "New session. The next message starts a fresh conversation.");
    r.agent.ends_with(TurnStatus::Completed, "Third.");
    r.bot.handle(&text(4, CHAT, "fresh?"), now(3)).await;
    assert!(eventually(|| showing(&r.api, "Third.") == 1).await);
    assert_eq!(r.agent.sends().iter().map(|(session, _)| session.as_str()).collect::<Vec<_>>(), ["s1", "s1", "s2"]);
}

#[tokio::test]
async fn while_a_turn_runs_a_message_queues_as_steering() {
    let r = paired();
    r.agent.running.store(true, Ordering::SeqCst);
    r.bot.handle(&text(1, CHAT, "first"), now(0)).await;
    assert!(r.api.until(|s| s.text == "Working on it...").await);
    r.bot.handle(&text(2, CHAT, "and also this"), now(1)).await;
    assert!(r.api.until(|s| s.text.ends_with("1 more message queued)")).await, "{:?}", r.api.sent());
    r.bot.handle(&text(3, CHAT, "and that"), now(2)).await;
    assert!(r.api.until(|s| s.text.ends_with("2 more messages queued)")).await);
    assert_eq!(r.agent.sends(), [("s1".to_string(), "first".to_string()), ("s1".to_string(), "and also this".to_string()), ("s1".to_string(), "and that".to_string())]);
    // The turn ends: one answer for all of it.
    r.agent.ends_with(TurnStatus::Completed, "All three, done.");
    r.agent.running.store(false, Ordering::SeqCst);
    r.agent.release.notify_waiters();
    assert!(r.api.until(|s| s.text == "All three, done.").await);
}

#[tokio::test]
async fn at_most_three_turns_a_minute_and_a_message_is_cut_at_4000_characters() {
    // Three turns in one minute; the fourth waits (the clock is the test's).
    let r = with(FakeAgent::default());
    r.bot.pair(Pairing { bot_username: "kagent_bot".into(), chat_id: CHAT, user_id: 7, paired_at: String::new() }).unwrap();
    r.agent.ends_with(TurnStatus::Completed, "ok");
    r.bot.handle(&text(1, CHAT, &"é".repeat(4500)), now(0)).await;
    assert!(eventually(|| showing(&r.api, "ok") == 1).await);
    assert_eq!(r.agent.sends()[0].1.chars().count(), 4000, "cut at 4000 characters, never inside one");
    for n in 2usize..=3 {
        r.agent.ends_with(TurnStatus::Completed, "ok");
        r.bot.handle(&text(n as i64, CHAT, "more"), now(10)).await;
        assert!(eventually(|| showing(&r.api, "ok") == n).await);
    }
    let (sent, sends) = (r.api.sent().len(), r.agent.sends().len());
    r.bot.handle(&text(4, CHAT, "one too many"), now(20)).await;
    assert_eq!(r.api.texts().last().unwrap(), "That is more than 3 messages in a minute. Wait 60 s, please.");
    assert_eq!((r.api.sent().len(), r.agent.sends().len()), (sent + 1, sends), "one message says it; a refused message costs no turn");
    // A minute later there is room again.
    r.clock().advance(Duration::from_secs(61));
    r.agent.ends_with(TurnStatus::Completed, "ok");
    r.bot.handle(&text(5, CHAT, "one more"), now(30)).await;
    assert!(eventually(|| showing(&r.api, "ok") == 4).await);
}

impl Rig {
    fn clock(&self) -> Arc<GateClock> {
        self._clock.clone()
    }
}

#[tokio::test]
async fn a_long_answer_comes_in_several_messages_and_text_telegram_cannot_parse_comes_plain() {
    let r = paired();
    let (a, b, c) = ("a".repeat(3000), "b".repeat(3000), "c".repeat(3000));
    r.agent.ends_with(TurnStatus::Completed, &format!("{a}\n\n{b}\n\n{c}"));
    r.bot.handle(&text(1, CHAT, "a long one"), now(0)).await;
    assert!(r.api.until(|s| s.text == c).await);
    assert_eq!(r.api.sent().iter().map(|s| (s.edit.is_some(), s.text.len())).collect::<Vec<_>>(), [(false, 16), (true, 3000), (false, 3000), (false, 3000)], "the first part replaces the placeholder");

    // Telegram would not parse the HTML: the same words, with no tag in them.
    r.api.refuse_html_once.store(true, Ordering::SeqCst);
    r.agent.ends_with(TurnStatus::Completed, "**Bold** and <answer>");
    r.bot.handle(&text(2, CHAT, "again"), now(1)).await;
    assert!(r.api.until(|s| s.text == "Bold and &lt;answer&gt;" && s.markup == Markup::None).await, "{:?}", r.api.sent());
}

#[tokio::test]
async fn a_voice_note_needs_a_gemini_model_and_is_text_once_transcribed() {
    let r = paired();
    r.bot.handle(&voice(1, CHAT, "f1"), now(0)).await;
    assert_eq!(r.api.texts(), ["Voice notes need a Gemini model. Enable one in Settings > Models, or type instead."]);

    let r = with(FakeAgent { transcriber: Some(Arc::new(FakeTranscriber("Switch the router off."))), ..Default::default() });
    r.bot.pair(Pairing { bot_username: "kagent_bot".into(), chat_id: CHAT, user_id: 7, paired_at: String::new() }).unwrap();
    *r.api.file.lock().unwrap() = ("voice/file_1.oga".into(), vec![1, 2, 3]);
    r.agent.ends_with(TurnStatus::Completed, "Done.");
    r.bot.handle(&voice(2, CHAT, "f1"), now(0)).await;
    assert!(eventually(|| showing(&r.api, "Done.") == 1).await);
    assert_eq!(r.agent.sends()[0].1, "Switch the router off.", "the transcript is handled as text");
    assert_eq!(*r.api.downloads.lock().unwrap(), ["voice/file_1.oga".to_string()]);
    // A path that is not a plain relative one is never asked for.
    *r.api.file.lock().unwrap() = ("https://evil.example/x".into(), vec![]);
    r.bot.handle(&voice(3, CHAT, "f2"), now(1)).await;
    assert_eq!(r.api.texts().last().unwrap(), "The voice note could not be transcribed. Try again, or type instead.");
    assert_eq!(r.api.downloads.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn attachments_and_updates_of_other_kinds_get_nothing_from_the_agent() {
    let r = paired();
    let photo: Update = serde_json::from_value(json!({"update_id": 5, "message": {"message_id": 9, "chat": {"id": CHAT, "type": "private"}, "photo": [{"file_id": "p", "width": 1, "height": 1}], "caption": "what is this?"}})).unwrap();
    r.bot.handle(&photo, now(0)).await;
    assert_eq!(r.api.texts(), ["I cannot read attachments here yet."]);
    let edited: Update = serde_json::from_value(json!({"update_id": 6, "edited_message": {"message_id": 9, "chat": {"id": CHAT}, "text": "typo"}})).unwrap();
    r.bot.handle(&edited, now(0)).await;
    r.bot.handled(&edited);
    assert_eq!(r.bot.offset(), Some(7), "the next poll starts after an update the bot drops");
    assert_eq!((r.api.sent().len(), r.agent.sends().len()), (1, 0));
}

#[tokio::test]
async fn the_commands_of_the_chat_start_no_turn() {
    let r = paired();
    *r.agent.status.lock().unwrap() = Status { doctor: None, open_alerts: 0, running: 2 };
    r.agent.stopped.store(2, Ordering::SeqCst);
    r.bot.handle(&text(1, CHAT, "/status"), now(0)).await;
    r.bot.handle(&text(2, CHAT, "/stop@kagent_bot"), now(0)).await;
    r.bot.handle(&text(3, CHAT, "/help"), now(0)).await;
    assert_eq!(r.api.texts(), ["Health: not checked now\nOpen alerts: 0\nRunning turns: 2", "Stopped 2 running turn(s).", HELP]);
    assert!(r.agent.sends().is_empty());
}

/// Scenario S29 item 4: Allow once resolves the card; "Already" for one that is gone.
#[tokio::test]
async fn allow_once_and_deny_answer_a_card_the_bot_sent_and_nothing_else() {
    let r = paired();
    let request = "6f1c2a0e-9b7d-4c3e-8a2f-0d1e2f3a4b5c";
    // A press that names a card the bot never sent: nobody waits for it.
    r.bot.handle(&press(1, CHAT, 500, &format!("a:{request}:allow")), now(0)).await;
    assert_eq!(*r.api.answered.lock().unwrap(), [("cb1".to_string(), Some("Already resolved".to_string()))]);
    assert!(r.agent.resolved().is_empty() && r.api.sent().is_empty());

    // A card the bot sent: Allow once resolves it and the message shows it.
    r.agent.pending.lock().unwrap().push(card(request, false, false));
    r.bot.card(CHAT, request, "KAgent needs your approval\n\nTool: shell", Keyboard::row([Button { text: "Allow once".into(), callback_data: cards::approval_data(request, true) }])).await.unwrap();
    let message = r.api.last_id();
    r.bot.handle(&press(2, CHAT, message, &cards::approval_data(request, true)), now(0)).await;
    assert_eq!(r.agent.resolved(), [(request.to_string(), ApprovalDecision::AllowOnce)]);
    assert_eq!(r.api.texts(), ["KAgent needs your approval\n\nTool: shell", "KAgent needs your approval\n\nTool: shell\n\nAllowed once from Telegram."]);
    assert_eq!(r.api.answered.lock().unwrap()[1], ("cb2".to_string(), Some("Allowed once".to_string())));
    // Pressed again: already resolved, and nothing more is edited.
    r.bot.handle(&press(3, CHAT, message, &cards::approval_data(request, true)), now(0)).await;
    assert_eq!((r.api.answered.lock().unwrap()[2].1.as_deref(), r.api.sent().len()), (Some("Already resolved"), 2));

    // Deny.
    r.agent.pending.lock().unwrap().push(card("r2", false, false));
    r.bot.card(CHAT, "r2", "card two", Keyboard::row([])).await.unwrap();
    r.bot.handle(&press(4, CHAT, r.api.last_id(), "a:r2:deny"), now(0)).await;
    assert_eq!(r.agent.resolved()[1], ("r2".to_string(), ApprovalDecision::Deny { reason: Some("denied from Telegram".into()) }));
    assert_eq!(r.api.texts()[3], "card two\n\nDenied from Telegram.");

    // Acknowledge an alert the bot pushed.
    r.bot.card(CHAT, &cards::ack_data("alr_1"), "KAgent alert (high): hot", Keyboard::row([])).await.unwrap();
    r.bot.handle(&press(5, CHAT, r.api.last_id(), "ack:alr_1"), now(0)).await;
    assert_eq!(*r.agent.acked.lock().unwrap(), ["alr_1".to_string()]);
    assert_eq!(r.api.texts()[5], "KAgent alert (high): hot\n\nAcknowledged from Telegram.");

    // A press from another chat is dropped before anything else.
    r.bot.handle(&press(6, 43, message, "a:r3:allow"), now(0)).await;
    assert_eq!(r.api.answered.lock().unwrap().len(), 5);
    // "Always" is no button of this bot.
    r.bot.handle(&press(7, CHAT, message, "a:r3:always"), now(0)).await;
    assert_eq!(r.api.answered.lock().unwrap()[5], ("cb7".to_string(), Some("This button is not one of mine.".to_string())));

    // A card answered on the desktop a moment ago.
    r.bot.card(CHAT, "r4", "card four", Keyboard::row([])).await.unwrap();
    r.bot.handle(&press(8, CHAT, r.api.last_id(), "a:r4:allow"), now(0)).await;
    assert_eq!((r.api.texts()[7].as_str(), r.api.answered.lock().unwrap()[6].1.as_deref()), ("card four\n\nAlready resolved.", Some("Already resolved")));
}
