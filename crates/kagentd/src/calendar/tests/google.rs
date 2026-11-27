//! The Google client and `calendar_events` (calendar.md sections 3 and 9): only the switched-on
//! calendars, paging, the time zone and all-day events, overlaps, a single instance of a series.

use chrono::Duration;
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};

use super::super::fake_api::{FAMILY, HOLIDAYS, OWNER, WORK};
use super::super::tools::CalendarEvents;
use super::super::{CalError, ConnState};
use super::{rig, Rig};

fn timed(title: &str, from: &str, to: &str) -> Value {
    json!({"summary": title, "start": {"dateTime": from}, "end": {"dateTime": to}})
}

async fn run(r: &Rig, input: Value) -> (bool, String) {
    let tool = CalendarEvents::new(r.cell());
    let out = tool.call(input, ToolContext::standalone(std::time::Duration::from_secs(5), 24_000)).await.unwrap();
    (out.is_error, out.content)
}

#[tokio::test]
async fn only_the_switched_on_calendars_are_asked_and_the_others_do_not_exist() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.fake.add_event("primary", timed("Standup", "2026-11-27T10:00:00+01:00", "2026-11-27T10:15:00+01:00"));
    r.fake.add_event(FAMILY, timed("Family dinner", "2026-11-27T18:00:00+01:00", "2026-11-27T19:00:00+01:00"));
    r.fake.add_event(WORK, timed("Secret work thing", "2026-11-27T11:00:00+01:00", "2026-11-27T12:00:00+01:00"));
    let (err, text) = run(&r, json!({})).await;
    assert!(!err, "{text}");
    assert!(text.contains("\"Standup\"") && !text.contains("Family dinner") && !text.contains("Secret work thing"), "{text}");
    let asked: Vec<String> = r.fake.seen_at("/calendar/v3/calendars").iter().map(|s| s.path.clone()).collect();
    assert!(asked.iter().all(|p| p.contains("owner%40example.com") || p.contains("owner@example.com")), "only the primary calendar was asked: {asked:?}");

    // One that is not switched on is refused, by name or by id, and nothing is sent for it.
    let before = r.fake.seen().len();
    for wanted in ["Work", WORK, "work"] {
        let (err, text) = run(&r, json!({"calendar": wanted})).await;
        assert!(err && text.contains("is not switched on for KAgent, so it does not exist for me") && text.contains("Personal"), "{wanted}: {text}");
    }
    let new: Vec<String> = r.fake.seen().iter().skip(before).map(|s| s.path.clone()).collect();
    assert!(new.iter().all(|p| p == "/calendar/v3/users/me/calendarList" || p == "/token"), "no events call for a calendar that is off: {new:?}");

    // The client itself refuses, whoever calls: a `CalInfo` for a calendar that is off.
    let all = r.cal.account_calendars().await.unwrap();
    assert_eq!(all.len(), 4, "the page may list every calendar of the account");
    assert_eq!(r.cal.account().as_deref(), Some(OWNER), "the account is the primary calendar's address");
    let work = all.iter().find(|c| c.id == WORK).unwrap();
    let from = r.cal.day_start(r.cal.today());
    let e = r.cal.list_events(work, from, from + Duration::days(1), None).await.unwrap_err();
    assert!(matches!(e, CalError::Refused(m) if m.contains("not switched on")));
    assert!(r.cal.get_event(work, "x").await.is_err());

    // Switching it on makes it exist, by the owner's settings only.
    r.settings.write().unwrap().patch(json!({"calendar": {"calendars": ["primary", WORK]}})).unwrap();
    let (_, text) = run(&r, json!({"calendar": "Work"})).await;
    assert!(text.contains("Secret work thing"), "{text}");
    let (_, text) = run(&r, json!({})).await;
    assert!(text.contains("Standup") && text.contains("Secret work thing") && !text.contains("Family dinner"), "{text}");
}

#[tokio::test]
async fn an_event_is_shown_in_the_machines_zone_with_its_id_place_meet_and_guests() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let mut e = timed("Lunch with Miha", "2026-11-28T11:00:00Z", "2026-11-28T12:00:00Z");
    e["location"] = json!("Thai Orchid, Glavni trg 5, Maribor");
    e["hangoutLink"] = json!("https://meet.google.com/abc-defg-hij");
    e["attendees"] = json!([{"email": "miha@example.com", "displayName": "Miha"}, {"email": "ana@example.com"}]);
    let id = r.fake.add_event("primary", e);
    let (err, text) = run(&r, json!({"from": "2026-11-28", "to": "2026-11-28"})).await;
    assert!(!err, "{text}");
    assert!(text.starts_with("[untrusted: calendar data"), "{text}");
    assert!(text.contains("Sat 28 Nov 12:00 to 13:00 \"Lunch with Miha\" at Thai Orchid, Glavni trg 5, Maribor"), "{text}");
    assert!(text.contains(&format!("calendar \"Personal\" | id {id}")) && text.contains("Meet https://meet.google.com/abc-defg-hij") && text.contains("guests: Miha, ana@example.com"), "{text}");
    assert!(text.contains("(UTC+01:00)"), "the header names the zone: {text}");
}

#[tokio::test]
async fn an_all_day_event_a_series_instance_and_an_overlap() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.fake.add_event("primary", json!({"summary": "Day off", "start": {"date": "2026-11-27"}, "end": {"date": "2026-11-28"}}));
    r.fake.add_event("primary", timed("Dentist", "2026-11-27T10:00:00+01:00", "2026-11-27T11:00:00+01:00"));
    let mut series = timed("Sync", "2026-11-27T10:30:00+01:00", "2026-11-27T11:30:00+01:00");
    series["recurringEventId"] = json!("series1");
    let sync = r.fake.add_event("primary", series);
    let (_, text) = run(&r, json!({})).await;
    assert!(text.contains("1. all day Fri 27 Nov \"Day off\""), "{text}");
    assert!(text.contains("2. Fri 27 Nov 10:00 to 11:00 \"Dentist\"") && text.contains(&format!("overlaps: {sync} \"Sync\" (10:30 to 11:00)")), "{text}");
    assert!(text.contains("3. Fri 27 Nov 10:30 to 11:30 \"Sync\"") && text.contains("one event of a series"), "{text}");
    assert!(!text.lines().find(|l| l.contains("Day off")).unwrap().contains("overlaps"), "an all-day event overlaps nothing");
}

#[tokio::test]
async fn every_page_is_read_and_max_cuts_the_list_and_says_so() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.fake.state.lock().unwrap().page_size = 2;
    for n in 0..5 {
        r.fake.add_event("primary", timed(&format!("Event {n}"), &format!("2026-11-27T1{n}:00:00+01:00"), &format!("2026-11-27T1{n}:30:00+01:00")));
    }
    let (_, text) = run(&r, json!({})).await;
    for n in 0..5 {
        assert!(text.contains(&format!("\"Event {n}\"")), "page {n}: {text}");
    }
    assert!(r.fake.seen().iter().any(|s| s.query.contains("pageToken=2")) && r.fake.seen().iter().any(|s| s.query.contains("pageToken=4")), "the next pages were asked");
    let (_, cut) = run(&r, json!({"max": 2})).await;
    assert!(cut.contains("2 of 5") && cut.contains("3 more not shown"), "{cut}");
    assert_eq!(cut.matches("\n1. ").count() + cut.matches("\n2. ").count(), 2);
    let (_, q) = run(&r, json!({"query": "event 3"})).await;
    assert!(q.contains("Event 3") && !q.contains("Event 1"), "{q}");
}

#[tokio::test]
async fn the_range_is_today_a_whole_day_or_what_was_asked_and_a_bad_one_is_an_error() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.fake.add_event("primary", timed("Today", "2026-11-27T23:30:00+01:00", "2026-11-28T00:30:00+01:00"));
    r.fake.add_event("primary", timed("Tomorrow", "2026-11-28T09:00:00+01:00", "2026-11-28T10:00:00+01:00"));
    r.fake.add_event("primary", timed("Monday", "2026-11-30T09:00:00+01:00", "2026-11-30T10:00:00+01:00"));
    let (_, today) = run(&r, json!({})).await;
    assert!(today.contains("\"Today\"") && !today.contains("Tomorrow") && today.contains("Fri 27 Nov 00:00 to Sat 28 Nov 00:00"), "the default is today: {today}");
    let (_, week) = run(&r, json!({"from": "2026-11-27", "to": "2026-11-30"})).await;
    assert!(week.contains("Today") && week.contains("Tomorrow") && week.contains("Monday"), "a date as the end includes that day: {week}");
    let (_, one) = run(&r, json!({"from": "2026-11-28T09:30"})).await;
    assert!(one.contains("Tomorrow"), "a time inside an event, to the end of its day: {one}");
    let (_, none) = run(&r, json!({"from": "2026-12-05"})).await;
    assert!(none.contains("No events from Sat 05 Dec") || none.contains("No events from Sat 5 Dec"), "{none}");
    for bad in [json!({"from": "next friday"}), json!({"from": "2026-11-28", "to": "2026-11-20"}), json!({"from": "2025-01-01", "to": "2026-11-27"})] {
        let (err, text) = run(&r, bad.clone()).await;
        assert!(err && !text.contains("No events"), "{bad}: {text}");
    }
}

#[tokio::test]
async fn a_401_is_refreshed_once_and_a_revoked_grant_is_reconnect() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.fake.add_event("primary", timed("Standup", "2026-11-27T10:00:00+01:00", "2026-11-27T10:15:00+01:00"));
    run(&r, json!({})).await;
    let tokens = r.fake.seen_at("/token").len();
    r.fake.expire_access_tokens();
    let (err, text) = run(&r, json!({})).await;
    assert!(!err && text.contains("Standup"), "the call went through after one refresh: {text}");
    assert_eq!(r.fake.seen_at("/token").len(), tokens + 1);
    r.fake.expire_access_tokens();
    r.fake.revoke_grant();
    let (err, text) = run(&r, json!({})).await;
    assert!(err && text.contains("Reconnect"), "{text}");
    assert_eq!(r.cal.state(), ConnState::Reconnect);
    let (err, text) = run(&r, json!({})).await;
    assert!(err && text.contains("connected again"), "later calls say so without asking Google: {text}");
}

#[tokio::test]
async fn off_not_connected_and_a_failure_of_google_are_errors_never_an_empty_list() {
    let r = rig().await;
    let (err, text) = run(&r, json!({})).await;
    assert!(err && text.contains("switched off"), "{text}");
    r.settings.write().unwrap().patch(json!({"calendar": {"enabled": true}})).unwrap();
    let (err, text) = run(&r, json!({})).await;
    assert!(err && text.contains("not connected") && text.contains("Settings -> Calendar"), "{text}");
    r.ready(&["primary"]).await;
    // A calendar that Google does not know (the owner switched on an id that is gone).
    r.settings.write().unwrap().patch(json!({"calendar": {"calendars": ["gone@example.com"], "defaultCalendar": "gone@example.com"}})).unwrap();
    let (err, text) = run(&r, json!({})).await;
    assert!(err && text.contains("no calendar is switched on that exists"), "{text}");
    // The module itself missing (the daemon is not up): an error too.
    let empty: super::super::tools::CalendarCell = Default::default();
    let out = CalendarEvents::new(empty).call(json!({}), ToolContext::standalone(std::time::Duration::from_secs(1), 1000)).await.unwrap();
    assert!(out.is_error);
    // The holidays calendar can be read once switched on, and is read-only for writes later.
    r.settings.write().unwrap().patch(json!({"calendar": {"calendars": [HOLIDAYS], "defaultCalendar": HOLIDAYS}})).unwrap();
    r.fake.add_event(HOLIDAYS, json!({"summary": "Statehood Day", "start": {"date": "2026-11-27"}, "end": {"date": "2026-11-28"}}));
    let (_, text) = run(&r, json!({})).await;
    assert!(text.contains("Statehood Day"), "{text}");
}
