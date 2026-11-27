//! The morning report (calendar.md sections 6 and 9): the text for a day, the arithmetic of
//! leaving, the timer with its days and its late start, the three places it goes to, and the
//! notice that comes once.

use std::sync::atomic::Ordering;

use chrono::Duration;
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};

use super::super::fake::MAPS_KEY;
use super::super::fake_api::FAMILY;
use super::super::report::{Day, TaskLine};
use super::super::report_tool::CalendarReport;
use super::{rig, Rig};

fn timed(title: &str, from: &str, to: &str) -> Value {
    json!({"summary": title, "start": {"dateTime": from}, "end": {"dateTime": to}})
}

/// Friday 27 Nov 2026, 09:00 on the machine's clock (UTC+01:00), a day with events.
async fn busy() -> Rig {
    let r = rig().await;
    r.ready(&["primary", FAMILY]).await;
    r.settings.write().unwrap().patch(json!({"persona": {"address": "Marko", "language": "en"}})).unwrap();
    r.fake.add_event("primary", json!({"summary": "Day off", "start": {"date": "2026-11-27"}, "end": {"date": "2026-11-28"}}));
    let mut dentist = timed("Dentist", "2026-11-27T10:00:00+01:00", "2026-11-27T11:00:00+01:00");
    dentist["location"] = json!("Kardeljeva ploščad 1, Ljubljana");
    r.fake.add_event("primary", dentist);
    r.fake.add_event(FAMILY, timed("School play", "2026-11-27T10:30:00+01:00", "2026-11-27T11:30:00+01:00"));
    r.fake.add_event("primary", timed("Gym", "2026-11-28T07:00:00+01:00", "2026-11-28T08:00:00+01:00"));
    r.fake.add_event("primary", timed("Later", "2026-11-28T15:00:00+01:00", "2026-11-28T16:00:00+01:00"));
    r
}

#[tokio::test]
async fn the_text_of_a_day_with_an_overlap_an_all_day_event_tomorrow_and_the_tasks() {
    let r = busy().await;
    *r.tasks.lock().unwrap() = vec![TaskLine { at: Some("14:00".into()), title: "Call the plumber".into() }];
    let text = r.cal.compose(Day::Today).await.unwrap();
    assert_eq!(
        text,
        "Good morning, Marko. Today, Friday, 27 November:\n- all day Day off\n- 10:00 Dentist, at Kardeljeva ploščad 1, Ljubljana\n- 10:30 School play\nDentist and School play overlap from 10:30 to 11:00.\nTomorrow starts early: Gym at 07:00.\nReminders today: 14:00 Call the plumber"
    );
    r.settings.write().unwrap().patch(json!({"calendar": {"report": {"tomorrow": false, "tasks": false}}})).unwrap();
    let plain = r.cal.compose(Day::Today).await.unwrap();
    assert!(!plain.contains("Tomorrow") && !plain.contains("Reminders"), "the content switches work: {plain}");
    let next = r.cal.compose(Day::Tomorrow).await.unwrap();
    assert!(next.starts_with("Tomorrow, Saturday, 28 November:\n- 07:00 Gym\n- 15:00 Later"), "{next}");
    r.settings.write().unwrap().patch(json!({"persona": {"language": "sl"}})).unwrap();
    let sl = r.cal.compose(Day::Today).await.unwrap();
    assert!(sl.starts_with("Dobro jutro, Marko. Danes, petek, 27. november:\n- ves dan Day off") && sl.contains("Dentist in School play se prekrivata od 10:30 do 11:00."), "{sl}");
}

#[tokio::test]
async fn an_empty_day_and_the_early_rule_for_tomorrow() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.settings.write().unwrap().patch(json!({"persona": {"address": "", "language": "en"}})).unwrap();
    assert_eq!(r.cal.compose(Day::Today).await.unwrap(), "Good morning. Today, Friday, 27 November:\nNothing in your calendar today.");
    // Tomorrow's first event is told when it is early or the only one.
    r.fake.add_event("primary", timed("Review", "2026-11-28T11:00:00+01:00", "2026-11-28T12:00:00+01:00"));
    assert!(r.cal.compose(Day::Today).await.unwrap().ends_with("Tomorrow: only Review at 11:00."));
    r.fake.add_event("primary", timed("Standup", "2026-11-28T11:30:00+01:00", "2026-11-28T11:45:00+01:00"));
    assert!(!r.cal.compose(Day::Today).await.unwrap().contains("Tomorrow"), "two events, the first at 11:00: nothing to say");
}

#[tokio::test]
async fn the_language_auto_follows_the_last_thing_the_owner_wrote() {
    let r = busy().await;
    r.settings.write().unwrap().patch(json!({"persona": {"language": "auto"}})).unwrap();
    assert!(r.cal.compose(Day::Today).await.unwrap().starts_with("Good morning"), "English until the owner writes Slovenian");
    r.cal.hear("Kaj imam danes na koledarju?");
    assert!(r.cal.compose(Day::Today).await.unwrap().starts_with("Dobro jutro"));
    r.cal.hear("ok");
    r.cal.hear("What is on my calendar tomorrow please");
    assert!(r.cal.compose(Day::Today).await.unwrap().starts_with("Good morning"));
}

#[tokio::test]
async fn leave_by_is_the_drive_with_traffic_plus_the_margin_before_the_first_event_with_a_place() {
    let r = busy().await;
    // Without a key and a home there is no such line.
    assert!(!r.cal.compose(Day::Today).await.unwrap().contains("Leave by"));
    r.cal.set_maps_key(Some(MAPS_KEY)).await.unwrap();
    r.fake.add_place("Kardeljeva", "Kardeljeva ploščad 1, Ljubljana", 46.0569, 14.5058);
    r.fake.add_place("Home", "Glavni trg 5, 2000 Maribor", 46.5547, 15.6459);
    assert!(!r.cal.compose(Day::Today).await.unwrap().contains("Leave by"), "no home yet");
    r.settings.write().unwrap().patch(json!({"calendar": {"home": "Glavni trg 5, 2000 Maribor", "travelMarginMin": 10}})).unwrap();
    // 30 minutes with traffic, plus 10: 10:00 - 40 min.
    let text = r.cal.compose(Day::Today).await.unwrap();
    assert!(text.contains("Leave by 09:20 for Dentist: 30 minutes by car with traffic, plus 10.\n"), "{text}");
    r.settings.write().unwrap().patch(json!({"calendar": {"travelMarginMin": 0, "report": {"leaveBy": false}}})).unwrap();
    assert!(!r.cal.compose(Day::Today).await.unwrap().contains("Leave by"), "the switch");
    // A failing map is no report without the line, not no report.
    r.settings.write().unwrap().patch(json!({"calendar": {"report": {"leaveBy": true}}})).unwrap();
    r.fake.state.lock().unwrap().maps_down = true;
    let text = r.cal.compose(Day::Today).await.unwrap();
    assert!(text.contains("Dentist") && !text.contains("Leave by"), "{text}");
}

#[tokio::test]
async fn the_timer_delivers_once_a_day_to_telegram_and_keeps_the_spoken_one_for_a_session() {
    let r = busy().await;
    r.later(Duration::minutes(-76)); // 07:44 local
    r.tick_check_none().await;
    r.later(Duration::minutes(1));
    r.cal.tick().await;
    let sent = r.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "once at the time");
    assert_eq!((sent[0].title.as_str(), sent[0].channels.clone()), ("Morning report", vec![crate::notify::Channel::Telegram]));
    assert!(sent[0].body.starts_with("Good morning, Marko. Today, Friday, 27 November:"), "{}", sent[0].body);
    r.cal.tick().await;
    r.later(Duration::minutes(3));
    r.cal.tick().await;
    assert_eq!(r.sent.lock().unwrap().len(), 1, "never twice the same day");
    // The page's book: when, what and where it went.
    let state: Value = serde_json::from_slice(&std::fs::read(r.dir.path().join("data/calendar-state.json")).unwrap()).unwrap();
    assert_eq!(state["lastReport"]["sent"], json!(["telegram", "spoken"]));
    assert!(state["lastReport"]["text"].as_str().unwrap().contains("Dentist"));
    // The spoken one waits, and is given once.
    let spoken = r.cal.take_spoken().expect("it waits for a voice session");
    assert_eq!(spoken, sent[0].body, "the same text");
    assert!(r.cal.take_spoken().is_none(), "spoken once");
}

impl Rig {
    async fn tick_check_none(&self) {
        self.cal.tick().await;
        assert!(self.sent.lock().unwrap().is_empty() && self.cal.take_spoken().is_none(), "nothing before its time");
    }
}

#[tokio::test]
async fn a_spoken_report_is_for_the_day_it_was_made() {
    let r = busy().await;
    r.cal.tick().await; // 09:00: a late start, before noon
    r.later(Duration::days(1));
    assert!(r.cal.take_spoken().is_none(), "yesterday's report is not read out today");
}

#[tokio::test]
async fn the_late_start_rule_and_the_weekend_time() {
    let r = busy().await;
    r.later(Duration::minutes(210)); // 12:30, a daemon that started after noon
    r.cal.tick().await;
    assert!(r.sent.lock().unwrap().is_empty() && r.cal.take_spoken().is_none(), "after 12:00 today has none");
    // Saturday 28 Nov at 09:00 is the weekend time, not 07:45.
    r.later(Duration::hours(20) + Duration::minutes(30)); // 09:00 Saturday
    r.cal.tick().await;
    assert_eq!(r.sent.lock().unwrap().len(), 1, "Saturday at 09:00");
    assert!(r.sent.lock().unwrap()[0].body.contains("Saturday, 28 November"));
}

#[tokio::test]
async fn telegram_is_left_out_when_no_chat_is_paired_and_each_channel_has_its_switch() {
    let r = busy().await;
    r.paired.store(false, Ordering::SeqCst);
    let (text, sent) = r.cal.report_now(true).await.unwrap();
    assert!(r.sent.lock().unwrap().is_empty() && sent == ["spoken"], "unpaired: {sent:?}");
    r.paired.store(true, Ordering::SeqCst);
    let (_, sent) = r.cal.report_now(true).await.unwrap();
    assert_eq!(sent, ["telegram", "spoken"]);
    r.settings.write().unwrap().patch(json!({"calendar": {"report": {"telegram": false}}})).unwrap();
    assert_eq!(r.cal.report_now(true).await.unwrap().1, ["spoken"]);
    r.settings.write().unwrap().patch(json!({"calendar": {"report": {"spoken": false}}})).unwrap();
    let (again, sent) = r.cal.report_now(true).await.unwrap();
    assert!(sent.is_empty() && again == text, "no channel is on: the text is made and shown, and goes nowhere");
    let before = r.sent.lock().unwrap().len();
    r.settings.write().unwrap().patch(json!({"calendar": {"report": {"telegram": true}}})).unwrap();
    let (_, sent) = r.cal.report_now(false).await.unwrap();
    assert!(sent.is_empty() && r.sent.lock().unwrap().len() == before, "without deliver nothing is sent");
}

#[tokio::test]
async fn not_connected_no_report_and_one_notice_not_one_a_day() {
    let mut r = rig().await;
    r.settings.write().unwrap().patch(json!({"calendar": {"enabled": true}})).unwrap();
    r.drain();
    r.cal.tick().await;
    r.later(Duration::days(1));
    r.cal.tick().await;
    r.later(Duration::days(1));
    r.cal.tick().await;
    let told: Vec<String> = r.drain().into_iter().filter(|e| e.starts_with("notice:")).collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("No morning report") && told[0].contains("not connected"), "{told:?}");
    assert!(r.sent.lock().unwrap().is_empty());
    // A report that cannot be made (Google fails) is told, and the day is not retried.
    let r = busy().await;
    r.fake.state.lock().unwrap().token_error = Some("server_error".into());
    r.later(Duration::hours(2));
    let mut r = r;
    r.drain();
    r.cal.tick().await;
    assert!(r.drain().iter().any(|e| e.contains("The morning report could not be made")));
    assert!(r.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_tool_says_the_report_for_the_model_and_refuses_when_not_connected() {
    let r = busy().await;
    let tool = CalendarReport::new(r.cell());
    let ctx = || ToolContext::standalone(std::time::Duration::from_secs(5), 24_000);
    let out = tool.call(json!({}), ctx()).await.unwrap();
    assert!(!out.is_error && out.content.starts_with("[untrusted: calendar data") && out.content.contains("Good morning, Marko.") && out.content.contains("Dentist"), "{}", out.content);
    let tomorrow = tool.call(json!({"day": "tomorrow"}), ctx()).await.unwrap();
    assert!(tomorrow.content.contains("Tomorrow, Saturday, 28 November") && tomorrow.content.contains("Gym"), "{}", tomorrow.content);
    assert!(r.sent.lock().unwrap().is_empty(), "the tool changes nothing and sends nothing");
    r.cal.disconnect().await.unwrap();
    let off = tool.call(json!({}), ctx()).await.unwrap();
    assert!(off.is_error && off.content.contains("not connected"), "{}", off.content);
}
