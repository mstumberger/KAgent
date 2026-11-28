//! `calendar_change` (calendar.md sections 3 and 9): each refusal, the three summaries built
//! from the stored event, the permission each call asks, the journal and every undo.

use kagent_core::permission::{Decision, Origin, PermissionEngine, PermissionMode};
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};

use super::super::change::CalendarChange;
use super::super::fake_api::{FAMILY, HOLIDAYS, WORK};
use super::{rig, Rig};

fn timed(title: &str, from: &str, to: &str) -> Value {
    json!({"summary": title, "start": {"dateTime": from}, "end": {"dateTime": to}})
}

async fn call(r: &Rig, ctx: &ToolContext, input: Value) -> (bool, String) {
    let tool = CalendarChange::new(r.cell());
    tool.prime(&input).await;
    let out = tool.call(input, ctx.clone()).await.unwrap();
    (out.is_error, out.content)
}

fn change_id(text: &str) -> String {
    text.lines().find_map(|l| l.strip_prefix("change id: ")).unwrap().split(' ').next().unwrap().to_string()
}

fn inserts(r: &Rig) -> Vec<super::super::fake::Seen> {
    r.fake.seen().into_iter().filter(|s| s.method != "GET" && s.path.starts_with("/calendar/")).collect()
}

async fn undo(ctx: &ToolContext, id: &str) -> (bool, String) {
    let out = kagent_core::tools::undo::run_undo(ctx, id).await.unwrap();
    (!out.is_error, out.content)
}

#[tokio::test]
async fn an_event_is_added_with_its_place_link_reminder_meet_and_guests() {
    let r = rig().await;
    r.ready(&["primary", FAMILY]).await;
    let ctx = r.ctx();
    let input = json!({
        "action": "add", "title": "Lunch with Miha", "start": "2026-11-28T12:00", "durationMin": 60,
        "location": {"name": "Thai Orchid", "address": "Glavni trg 5, Maribor", "mapsUrl": "https://www.google.com/maps/search/?api=1&query=Thai+Orchid"},
        "notes": "Bring the contract", "reminderMin": 30, "meet": true, "guests": ["miha@example.com"]
    });
    let (err, text) = call(&r, &ctx, input).await;
    assert!(!err, "{text}");
    let post = &inserts(&r)[0];
    assert_eq!((post.method.as_str(), post.path.as_str()), ("POST", "/calendar/v3/calendars/owner%40example.com/events"), "the default calendar");
    let body = post.json();
    assert_eq!(body["summary"], "Lunch with Miha");
    assert_eq!(body["start"]["dateTime"], "2026-11-28T12:00:00+01:00");
    assert_eq!(body["end"]["dateTime"], "2026-11-28T13:00:00+01:00");
    assert_eq!(body["location"], "Thai Orchid, Glavni trg 5, Maribor");
    assert_eq!(body["description"], "https://www.google.com/maps/search/?api=1&query=Thai+Orchid\n\nBring the contract", "the Maps link is the first line");
    assert_eq!(body["reminders"], json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": 30}]}));
    assert_eq!(body["attendees"], json!([{"email": "miha@example.com"}]));
    assert_eq!(body["conferenceData"]["createRequest"]["conferenceSolutionKey"]["type"], "hangoutsMeet");
    assert!(post.query.contains("sendUpdates=all") && post.query.contains("conferenceDataVersion=1"), "guests are mailed, Meet is asked for: {}", post.query);
    assert!(text.contains("stored event: Sat 28 Nov 12:00 to 13:00 \"Lunch with Miha\" at Thai Orchid") && text.contains("Meet https://meet.google.com/") && text.contains("link: https://calendar.google.com/calendar/event?eid=evt1"), "{text}");
    assert!(text.contains("change id: chg_") && text.contains("undo available"), "{text}");
}

#[tokio::test]
async fn without_guests_nobody_is_mailed_and_the_defaults_apply() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let (err, text) = call(&r, &r.ctx(), json!({"action": "add", "title": "Gym", "start": "2026-11-28T07:00"})).await;
    assert!(!err, "{text}");
    let post = &inserts(&r)[0];
    assert!(post.query.contains("sendUpdates=none") && !post.query.contains("conferenceDataVersion"), "{}", post.query);
    let body = post.json();
    assert_eq!(body["end"]["dateTime"], "2026-11-28T08:00:00+01:00", "an hour unless said otherwise");
    assert!(body.get("attendees").is_none() && body.get("reminders").is_none() && body.get("location").is_none() && body.get("conferenceData").is_none(), "{body}");
    r.settings.write().unwrap().patch(json!({"calendar": {"defaultDurationMin": 30}})).unwrap();
    call(&r, &r.ctx(), json!({"action": "add", "title": "Short", "start": "2026-11-28T09:00", "reminderMin": null})).await;
    let body = inserts(&r)[1].json();
    assert_eq!(body["end"]["dateTime"], "2026-11-28T09:30:00+01:00", "the owner's default length");
    assert_eq!(body["reminders"], json!({"useDefault": true}), "null is Google's default");
    let (_, day) = call(&r, &r.ctx(), json!({"action": "add", "title": "Holiday", "start": "2026-12-24", "end": "2026-12-26", "allDay": true})).await;
    assert!(day.contains("all day Thu 24 Dec to Sat 26 Dec"), "{day}");
    assert_eq!(inserts(&r)[2].json()["end"], json!({"date": "2026-12-27"}), "Google's end is the day after the last");
}

#[tokio::test]
async fn each_refusal_says_why_and_sends_nothing() {
    let r = rig().await;
    r.ready(&["primary", HOLIDAYS]).await;
    r.settings.write().unwrap().patch(json!({"calendar": {"defaultCalendar": "primary"}})).unwrap();
    let hidden = r.fake.add_event(WORK, timed("Hidden", "2026-11-27T11:00:00+01:00", "2026-11-27T12:00:00+01:00"));
    let mut series = timed("Weekly", "2026-11-27T13:00:00+01:00", "2026-11-27T14:00:00+01:00");
    series["recurrence"] = json!(["RRULE:FREQ=WEEKLY"]);
    let master = r.fake.add_event("primary", series);
    let ctx = r.ctx();
    let many: Vec<String> = (0..21).map(|n| format!("g{n}@example.com")).collect();
    let base = json!({"action": "add", "title": "T", "start": "2026-11-28T12:00"});
    let with = |k: &str, v: Value| {
        let mut b = base.clone();
        b[k] = v;
        b
    };
    let cases = [
        (with("calendar", json!("Work")), "is not switched on for KAgent"),
        (with("calendar", json!(HOLIDAYS)), "is read-only"),
        (with("end", json!("2026-11-28T11:00")), "is not after the start"),
        (with("start", json!("2036-11-28T11:00")), "more than 5 years from now"),
        (with("start", json!("2016-11-28T11:00")), "more than 5 years from now"),
        (with("guests", json!(many)), "21 guests is more than the 20"),
        (with("guests", json!(["miha"])), "is not an email address: ask the owner"),
        (with("durationMin", json!(0)), "durationMin must be"),
        (with("start", json!("2026-11-28")), "a time is needed for the start"),
        (json!({"action": "add", "start": "2026-11-28T12:00"}), "add needs a title"),
        (json!({"action": "add", "title": "T"}), "start is needed"),
        (with("location", json!({"name": "X", "mapsUrl": "https://evil.example/"})), "is not a Google Maps link"),
        (json!({"action": "update", "id": hidden, "title": "x"}), "no event with id"),
        (json!({"action": "delete", "id": hidden}), "no event with id"),
        (json!({"action": "update", "id": master, "title": "x"}), "change the series in Google Calendar"),
        (json!({"action": "delete", "id": master}), "change the series in Google Calendar"),
        (json!({"action": "update", "id": master}), "change the series in Google Calendar"),
        (json!({"action": "delete"}), "delete needs the id"),
        (json!({"action": "update"}), "update needs the id"),
        (json!({"action": "rename"}), "action must be add, update or delete"),
    ];
    for (input, why) in cases {
        let (err, text) = call(&r, &ctx, input.clone()).await;
        assert!(err && text.contains(why), "{input}: {text}");
    }
    assert!(inserts(&r).is_empty(), "nothing was sent to Google: {:?}", inserts(&r).iter().map(|s| format!("{} {}", s.method, s.path)).collect::<Vec<_>>());
    // A plain event, with nothing to change: refused too.
    let plain = r.fake.add_event("primary", timed("Plain", "2026-11-27T15:00:00+01:00", "2026-11-27T16:00:00+01:00"));
    let (err, text) = call(&r, &ctx, json!({"action": "update", "id": plain})).await;
    assert!(err && text.contains("nothing to change"), "{text}");
    // The calendar of an event that is off does not exist for update and delete either.
    let (err, text) = call(&r, &ctx, json!({"action": "delete", "id": hidden, "calendar": "Work"})).await;
    assert!(err && text.contains("not switched on"), "{text}");
}

#[tokio::test]
async fn the_sentence_of_the_card_is_built_from_the_stored_event_not_the_models_words() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let id = r.fake.add_event("primary", timed("Dentist", "2026-11-28T09:00:00+01:00", "2026-11-28T09:30:00+01:00"));
    let tool = CalendarChange::new(r.cell());

    let add = json!({"action": "add", "title": "Lunch with Miha", "start": "2026-11-28T12:00", "location": {"name": "Thai Orchid", "address": "Glavni trg 5, Maribor"}, "reminderMin": 30});
    tool.prime(&add).await;
    assert_eq!(tool.assess(&add).summary, "add \"Lunch with Miha\", Sat 28 Nov 12:00 to 13:00, at Thai Orchid, Glavni trg 5, Maribor, calendar Personal, reminder 30 min");

    // The model says the title is "Teeth": the card still names the event Google holds.
    let mv = json!({"action": "update", "id": id, "title": "Teeth", "start": "2026-11-29T10:00"});
    tool.prime(&mv).await;
    let a = tool.assess(&mv);
    assert_eq!(a.summary, "move \"Dentist\" from Sat 28 Nov 09:00 to 09:30 to Sun 29 Nov 10:00 to 10:30; also change: title to \"Teeth\"");
    let mv_only = json!({"action": "update", "id": id, "start": "2026-11-29T10:00"});
    assert_eq!(tool.assess(&mv_only).summary, "move \"Dentist\" from Sat 28 Nov 09:00 to 09:30 to Sun 29 Nov 10:00 to 10:30", "the length is kept");
    assert!(!a.destructive && a.always_ask && !a.read_only && a.needs_approval);

    let del = json!({"action": "delete", "id": id, "title": "Something else"});
    tool.prime(&del).await;
    let d = tool.assess(&del);
    assert_eq!(d.summary, "delete \"Dentist\", Sat 28 Nov 09:00 to 09:30");
    assert!(d.destructive && !d.read_only, "delete is destructive");

    // Not read yet: the sentence says so rather than inventing a title.
    let unknown = json!({"action": "delete", "id": "never-read"});
    assert_eq!(tool.assess(&unknown).summary, "delete event never-read");
}

#[tokio::test]
async fn permission_add_and_update_ask_from_voice_in_full_and_delete_asks_everywhere() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let id = r.fake.add_event("primary", timed("Dentist", "2026-11-28T09:00:00+01:00", "2026-11-28T09:30:00+01:00"));
    let tool = CalendarChange::new(r.cell());
    let add = tool.assess(&json!({"action": "add", "title": "T", "start": "2026-11-28T12:00"}));
    let upd = tool.assess(&json!({"action": "update", "id": id, "title": "X"}));
    tool.prime(&json!({"action": "delete", "id": id})).await;
    let del = tool.assess(&json!({"action": "delete", "id": id}));
    let ask = |mode, a: &_, origin| matches!(PermissionEngine::new(mode, vec![]).evaluate_from("s", "calendar_change", a, origin), Decision::Ask { .. });
    for a in [&add, &upd] {
        assert!(ask(PermissionMode::Ask, a, Origin::Chat), "a card in ask");
        assert!(ask(PermissionMode::Trusted, a, Origin::Chat), "and in trusted: always_ask");
        assert!(!ask(PermissionMode::Full, a, Origin::Chat), "none in chat under full");
        assert!(ask(PermissionMode::Full, a, Origin::Voice), "but from voice it asks in full");
        assert!(a.always_ask && !a.read_only, "so the voice gate asks out loud: {a:?}");
    }
    for origin in [Origin::Chat, Origin::Voice] {
        for mode in [PermissionMode::Ask, PermissionMode::Trusted, PermissionMode::Full] {
            assert!(ask(mode, &del, origin), "delete asks in {mode:?} from {origin:?}");
        }
    }
    assert!(del.destructive, "and by voice only 'confirm' answers a destructive call");
}

#[tokio::test]
async fn update_changes_one_event_and_mails_only_new_guests() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let mut e = timed("Review", "2026-11-28T14:00:00+01:00", "2026-11-28T15:30:00+01:00");
    e["description"] = json!("https://maps.google.com/?cid=1\n\nOld notes");
    e["location"] = json!("Old room");
    e["attendees"] = json!([{"email": "ana@example.com"}]);
    let id = r.fake.add_event("primary", e);
    let ctx = r.ctx();
    let (err, text) = call(&r, &ctx, json!({"action": "update", "id": id, "start": "2026-11-30T09:00", "notes": "New notes"})).await;
    assert!(!err, "{text}");
    let patch = &inserts(&r)[0];
    assert_eq!((patch.method.as_str(), patch.query.as_str()), ("PATCH", "sendUpdates=none"), "no guest is mailed for a move");
    let body = patch.json();
    assert_eq!(body["end"]["dateTime"], "2026-11-30T10:30:00+01:00", "the length of 90 minutes is kept");
    assert_eq!(body["description"], "https://maps.google.com/?cid=1\n\nNew notes", "the Maps link line is kept");
    assert!(body.get("attendees").is_none() && body.get("location").is_none() && body.get("summary").is_none(), "only what was named: {body}");
    assert!(text.contains("stored event: Mon 30 Nov 09:00 to 10:30 \"Review\""), "{text}");
    // A new guest is mailed, an old one is not asked twice.
    call(&r, &ctx, json!({"action": "update", "id": id, "guests": ["ana@example.com", "miha@example.com"]})).await;
    let patch = &inserts(&r)[1];
    assert!(patch.query.contains("sendUpdates=all"), "{}", patch.query);
    assert_eq!(patch.json()["attendees"], json!([{"email": "ana@example.com"}, {"email": "miha@example.com"}]));
    call(&r, &ctx, json!({"action": "update", "id": id, "guests": ["ana@example.com"]})).await;
    let third = inserts(&r);
    assert_eq!(third.len(), 2, "a guest who is there already: nothing to change, nothing sent");
}

#[tokio::test]
async fn every_change_is_journaled_and_every_undo_puts_it_back() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let ctx = r.ctx();
    let journal = &ctx.services.as_ref().unwrap().journal;

    // add -> undo deletes the event.
    let (_, text) = call(&r, &ctx, json!({"action": "add", "title": "Lunch", "start": "2026-11-28T12:00", "location": {"name": "Thai Orchid", "address": "Maribor"}})).await;
    let added = change_id(&text);
    let record = journal.get(&added).unwrap();
    assert_eq!((record.tool.as_str(), record.session_id.as_str(), record.privileged), ("calendar_change", "s1", false));
    assert!(record.summary.starts_with("add \"Lunch\", Sat 28 Nov 12:00 to 13:00, at Thai Orchid, Maribor, calendar Personal"), "{}", record.summary);
    assert_eq!(r.fake.events_of("primary").len(), 1);
    let (ok, said) = undo(&ctx, &added).await;
    assert!(ok, "{said}");
    assert!(r.fake.events_of("primary").is_empty(), "the event is gone");
    assert_eq!(format!("{:?}", journal.get(&added).unwrap().status), "Undone");
    assert!(!undo(&ctx, &added).await.0, "an undone change is not undone twice");

    // update -> undo writes the old fields back.
    let id = r.fake.add_event("primary", timed("Dentist", "2026-11-28T09:00:00+01:00", "2026-11-28T09:30:00+01:00"));
    let (_, text) = call(&r, &ctx, json!({"action": "update", "id": id, "title": "Teeth", "start": "2026-11-29T10:00", "location": {"name": "Clinic"}})).await;
    let moved = change_id(&text);
    let now = &r.fake.events_of("primary")[0];
    assert_eq!((now["summary"].as_str(), now["start"]["dateTime"].as_str(), now["location"].as_str()), (Some("Teeth"), Some("2026-11-29T10:00:00+01:00"), Some("Clinic")));
    let (ok, said) = undo(&ctx, &moved).await;
    assert!(ok && said.contains("Dentist"), "{said}");
    let back = &r.fake.events_of("primary")[0];
    assert_eq!((back["summary"].as_str(), back["start"]["dateTime"].as_str(), back["end"]["dateTime"].as_str()), (Some("Dentist"), Some("2026-11-28T09:00:00+01:00"), Some("2026-11-28T09:30:00+01:00")));
    assert!(back.get("location").is_none(), "a field the event did not have is cleared again: {back}");

    // delete -> undo restores it from the stored copy (as a new event: Google keeps the old id).
    let mut e = timed("Standup", "2026-11-30T10:00:00+01:00", "2026-11-30T10:15:00+01:00");
    e["description"] = json!("Daily");
    e["attendees"] = json!([{"email": "ana@example.com", "responseStatus": "accepted"}]);
    let gone = r.fake.add_event("primary", e);
    let (err, text) = call(&r, &ctx, json!({"action": "delete", "id": gone})).await;
    assert!(!err && text.contains("done: delete \"Standup\", Mon 30 Nov 10:00 to 10:15"), "{text}");
    assert!(r.fake.events_of("primary").iter().all(|e| e["summary"] != "Standup"));
    let (ok, said) = undo(&ctx, &change_id(&text)).await;
    assert!(ok && said.contains("brought the event back as a new one"), "{said}");
    let restored = r.fake.events_of("primary").into_iter().find(|e| e["summary"] == "Standup").unwrap();
    assert_eq!((restored["start"]["dateTime"].as_str(), restored["description"].as_str()), (Some("2026-11-30T10:00:00+01:00"), Some("Daily")));
    assert_eq!(restored["attendees"], json!([{"email": "ana@example.com"}]));
    let last = inserts(&r).into_iter().last().unwrap();
    assert!(last.query.contains("sendUpdates=none"), "bringing it back mails nobody: {}", last.query);
}

#[tokio::test]
async fn an_undo_of_a_calendar_that_is_switched_off_does_not_reach_it() {
    let r = rig().await;
    r.ready(&["primary", WORK]).await;
    let ctx = r.ctx();
    let (_, text) = call(&r, &ctx, json!({"action": "add", "title": "Secret", "calendar": "Work", "start": "2026-11-28T12:00"})).await;
    r.settings.write().unwrap().patch(json!({"calendar": {"calendars": ["primary"], "defaultCalendar": "primary"}})).unwrap();
    let (ok, said) = undo(&ctx, &change_id(&text)).await;
    assert!(!ok && said.contains("not switched on"), "{said}");
    assert_eq!(r.fake.events_of(WORK).len(), 1);
}

/// v0.0.3 A7: a read-only calendar is refused as the default when the setting is saved.
#[tokio::test]
async fn a_read_only_default_is_refused_when_it_is_saved() {
    let r = rig().await;
    // Not connected: nothing can be told yet, the check of `add` stays.
    assert!(r.cal.check_default(&json!({"calendar": {"defaultCalendar": HOLIDAYS}})).await.is_ok());
    r.ready(&["primary"]).await;
    let refused = r.cal.check_default(&json!({"calendar": {"defaultCalendar": HOLIDAYS}})).await.unwrap_err();
    assert!(refused.contains("Holidays in Slovenia") && refused.contains("read-only"), "{refused}");
    for ok in [json!({"calendar": {"defaultCalendar": WORK}}), json!({"calendar": {"defaultCalendar": FAMILY}}), json!({"calendar": {"defaultCalendar": "primary"}}), json!({"calendar": {"defaultCalendar": "unknown"}}), json!({"calendar": {"home": "x"}}), json!({"maxIterations": 5})] {
        assert!(r.cal.check_default(&ok).await.is_ok(), "{ok}");
    }
    // By its name, as the owner's page and the agent say it.
    assert!(r.cal.check_default(&json!({"calendar": {"defaultCalendar": "holidays in slovenia"}})).await.is_err());
}
