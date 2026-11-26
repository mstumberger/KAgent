//! The rest of the whole road: alerts and the Telegram channel, the state on every
//! change and the token deleted, and what the settings route takes.

use super::*;

/// Scenario S29 item 5: a `review` alert inside the quiet hours is not pushed; `high` is, with an Ack button.
#[tokio::test]
async fn alerts_and_the_telegram_channel_reach_the_paired_chat_only_while_the_bot_serves_it() {
    let r = rig().await.paired(json!({})).await;
    r.alerts.hit_at(hit("info", Severity::Info), Instant::now());
    *r.quiet.lock().unwrap() = true;
    r.alerts.hit_at(hit("review", Severity::Review), Instant::now());
    let high = r.alerts.hit_at(hit("high", Severity::High), Instant::now()).unwrap();
    assert!(r.api.until(|s| s.keyboard.is_some()).await, "{:?}", r.api.sent());
    assert_eq!(r.cards(), 1, "only the high alert: info is for the UI, review waits for the quiet hours to end");
    let pushed = r.api.sent().last().cloned().unwrap();
    assert_eq!((pushed.chat, pushed.text.as_str()), (42, "KAgent alert (high): t\n\nc\nSuggested: p"));
    assert_eq!(forward::button_data(pushed.keyboard.as_ref().unwrap(), "Ack"), Some(format!("ack:{}", high.id)));

    // Ack from the phone.
    r.api.update(press(2, 42, r.api.last_id(), &cards::ack_data(&high.id)));
    assert!(eventually(|| r.agent.acked.lock().unwrap().len() == 1).await);
    assert_eq!(*r.agent.acked.lock().unwrap(), [high.id.clone()]);
    assert!(r.api.until(|s| s.text.ends_with("Acknowledged from Telegram.")).await);

    // The Telegram channel of the notifier: a reminder, or the result of an automation.
    let note = Notification { title: "Reminder: Stand up".into(), body: "It is 10:00".into(), urgency: Urgency::Normal, channels: vec![Channel::Telegram] };
    r.notifier.send(note.clone()).await.unwrap();
    assert!(r.api.until(|s| s.text == "Reminder: Stand up\nIt is 10:00").await);

    // notifyAlerts off: no alert is pushed. The bot off: no channel sends and no card goes.
    ok(r.bots.patch(json!({"notifyAlerts": false, "forwardApprovals": true})));
    *r.quiet.lock().unwrap() = false;
    r.alerts.hit_at(hit("high2", Severity::High), Instant::now());
    ok(r.bots.patch(json!({"enabled": false})));
    assert!(eventually(|| r.bots.known().status == PollState::Disabled).await);
    let (sent, answered) = (r.sent(), r.api.answered.lock().unwrap().len());
    r.alerts.hit_at(hit("high3", Severity::High), Instant::now());
    r.notifier.send(note).await.unwrap();
    r.agent.pending.lock().unwrap().push(card("off", false, false));
    r.events.send(AgentEvent::ApprovalRequested(card("off", false, false))).unwrap();
    r.settle().await;
    assert_eq!((r.sent(), r.api.answered.lock().unwrap().len()), (sent, answered), "nothing was pushed, sent or forwarded");
}

#[tokio::test]
async fn the_state_is_told_on_every_change_and_deleting_the_token_forgets_the_chat() {
    let r = rig().await;
    let mut seen = r.events.subscribe();
    ok(r.bots.set_token(TOKEN).await);
    ok(r.bots.patch(json!({"enabled": true})));
    r.start();
    assert!(eventually(|| r.bots.known().status == PollState::Unpaired).await);
    // The poll fails: error, with what Telegram said; the next poll is tried 5 s later.
    r.api.push(Err(ApiError::Api { status: 401, description: "Unauthorized".into() }));
    assert!(eventually(|| r.bots.known().status == PollState::Error).await);
    assert_eq!(r.bots.known().detail, "Telegram answered 401: Unauthorized");
    assert!(eventually(|| r.clock.sleeps() == [Duration::from_secs(5)]).await, "the retry delay, on the clock the test holds");
    assert_eq!(r.bots.view()["lastError"], "Telegram answered 401: Unauthorized");
    // A good poll: the bot waits for a chat again; the last error is kept.
    r.api.push(Ok(vec![]));
    assert!(eventually(|| r.bots.known().status == PollState::Unpaired).await);
    assert_eq!(r.bots.view()["lastError"], "Telegram answered 401: Unauthorized");
    let code = ok(r.bots.pairing_code());
    assert_eq!(r.view()["pendingCode"]["code"], code["code"], "the code still waiting is in the answer");
    r.api.update(text(1, 42, &format!("/start {}", code["code"].as_str().unwrap())));
    assert!(eventually(|| r.bots.known().status == PollState::Polling).await);
    let sent = r.sent();

    // Deleting the token: the bot is off, the chat is forgotten, and nothing polls.
    r.bots.clear_token().await.unwrap();
    assert!(eventually(|| r.bots.known().status == PollState::Off).await);
    let view = r.view();
    assert_eq!((view["configured"].as_bool(), view["paired"].as_bool(), view["botUsername"].as_null(), view["lastError"].as_null()), (Some(false), Some(false), Some(()), Some(())));
    assert!(!Pairing::path(r.dir.path()).exists() && !r.dir.path().join("telegram.secret").exists());
    r.settle().await;
    let polls = r.api.offsets.lock().unwrap().len();
    r.api.update(text(2, 42, "anyone there?"));
    r.settle().await;
    assert_eq!((r.api.offsets.lock().unwrap().len(), r.sent()), (polls, sent), "no poll, no answer");
    assert_eq!(r.bots.test_message().await["ok"], false);
    assert_eq!(
        statuses(&mut seen),
        ["disabled:", "unpaired:", "error:Telegram answered 401: Unauthorized", "unpaired:", "polling:", "off:"],
        "an event for every change of the state (bots.md section 6)"
    );
}

#[tokio::test]
async fn the_settings_route_takes_the_five_keys_and_nothing_else() {
    let r = rig().await;
    let mut seen = r.events.subscribe();
    let err = refusal(r.bots.patch(json!({"enabled": true, "other": 1})));
    assert_eq!((err.0, err.1, err.2.as_str()), (StatusCode::BAD_REQUEST, "invalid_input", "unknown key 'other': one of enabled, notifyAlerts, forwardApprovals, approvalForwardSec, allowPrivileged"));
    assert!(r.bots.patch(json!("yes")).is_err(), "the patch must be an object");
    assert_eq!(refusal(r.bots.patch(json!({"approvalForwardSec": 601}))).2, "invalid settings.telegram: approvalForwardSec must be between 5 and 600, not 601");
    assert!(!r.bots.section().enabled, "a refused patch changes nothing");
    ok(r.bots.patch(json!({"enabled": true, "notifyAlerts": false, "forwardApprovals": true, "approvalForwardSec": 45, "allowPrivileged": true})));
    let section = r.bots.section();
    assert_eq!((section.enabled, section.notify_alerts, section.forward_approvals, section.approval_forward_sec, section.allow_privileged), (true, false, true, 45, true));
    // Saved as PUT /api/settings saves, and told to every UI.
    assert_eq!(Settings::load(&r.dir.path().join("settings.json")).unwrap().telegram_config(), section);
    assert!(matches!(seen.try_recv(), Ok(AgentEvent::SettingsChanged { .. })));
}
