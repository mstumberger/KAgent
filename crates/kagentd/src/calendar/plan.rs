//! What `calendar_change` is asked to do, checked and turned into Google's JSON and into the
//! sentence the owner confirms (calendar.md section 3). Pure apart from the clock and the
//! zone of the module: every refusal is decided here, before anything is sent.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate};
use serde_json::{json, Map, Value};

use super::model::{at_text, span_text, Event, When};
use super::tools::parse_instant;
use super::Calendar;

/// More than this many guests is refused.
pub const MAX_GUESTS: usize = 20;
/// A start further from now than this (past or future) is refused.
const MAX_DISTANCE: Duration = Duration::days(5 * 366);
const MAPS_LINKS: [&str; 3] = ["https://www.google.com/maps/", "https://maps.google.com/", "https://maps.app.goo.gl/"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Place {
    pub name: String,
    pub address: String,
    pub maps_url: String,
}

impl Place {
    /// `name, address`: what the event's location says.
    pub fn text(&self) -> String {
        match (self.name.is_empty(), self.address.is_empty()) {
            (_, true) => self.name.clone(),
            (true, false) => self.address.clone(),
            _ => format!("{}, {}", self.name, self.address),
        }
    }
}

/// The fields of one `calendar_change` call, read and shape-checked.
#[derive(Debug, Clone, Default)]
pub struct Params {
    pub title: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub duration_min: Option<u32>,
    pub all_day: Option<bool>,
    pub place: Option<Place>,
    pub notes: Option<String>,
    /// `Some(None)`: `reminderMin: null`, Google's default.
    pub reminder: Option<Option<u32>>,
    pub meet: bool,
    pub guests: Vec<String>,
}

fn text(input: &Value, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.chars().any(|c| c.is_control() && c != '\n') => Err(format!("{key} holds control characters")),
        Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
        Some(_) => Err(format!("{key} must be text")),
    }
}

pub fn read(input: &Value) -> Result<Params, String> {
    let mut p = Params { title: text(input, "title")?, start: text(input, "start")?, end: text(input, "end")?, notes: text(input, "notes")?, ..Default::default() };
    if p.title.as_ref().is_some_and(|t| t.is_empty() || t.chars().count() > 300) {
        return Err("title must be 1 to 300 characters".into());
    }
    if p.notes.as_ref().is_some_and(|n| n.chars().count() > 8000) {
        return Err("notes are longer than 8000 characters".into());
    }
    if let Some(d) = input.get("durationMin").filter(|v| !v.is_null()) {
        p.duration_min = Some(d.as_u64().filter(|d| (1..=10_080).contains(d)).ok_or("durationMin must be a whole number of minutes, 1 to 10080")? as u32);
    }
    p.all_day = input.get("allDay").and_then(Value::as_bool);
    p.meet = input.get("meet").and_then(Value::as_bool).unwrap_or(false);
    if let Some(r) = input.get("reminderMin") {
        p.reminder = Some(match r {
            Value::Null => None,
            v => Some(v.as_u64().filter(|m| *m <= 40_320).ok_or("reminderMin must be minutes before the start (0 to 40320), or null for Google's default")? as u32),
        });
    }
    if let Some(l) = input.get("location").filter(|v| !v.is_null()) {
        let part = |k: &str| l.get(k).and_then(Value::as_str).map(|s| s.trim().to_string()).unwrap_or_default();
        let place = Place { name: part("name"), address: part("address"), maps_url: part("mapsUrl") };
        if place.name.is_empty() && place.address.is_empty() {
            return Err("location needs a name or an address".into());
        }
        if !place.maps_url.is_empty() && !MAPS_LINKS.iter().any(|p| place.maps_url.starts_with(p)) {
            return Err("location.mapsUrl is not a Google Maps link: use the mapsUrl that place_search gave".into());
        }
        p.place = Some(place);
    }
    if let Some(g) = input.get("guests").filter(|v| !v.is_null()) {
        let list = g.as_array().ok_or("guests must be a list of email addresses")?;
        if list.len() > MAX_GUESTS {
            return Err(format!("{} guests is more than the {MAX_GUESTS} KAgent invites at once", list.len()));
        }
        for guest in list {
            let email = guest.as_str().map(str::trim).unwrap_or_default();
            let ok = email.len() <= 254 && email.split_once('@').is_some_and(|(a, b)| !a.is_empty() && b.contains('.') && !b.ends_with('.')) && !email.contains(char::is_whitespace);
            if !ok {
                return Err(format!("\"{email}\" is not an email address: ask the owner for it, never guess one"));
            }
            p.guests.push(email.to_string());
        }
    }
    Ok(p)
}

/// The two ends of an event: a new one (`old` is `None`) or the new times of a moved one.
pub fn times(cal: &Calendar, p: &Params, old: Option<&Event>) -> Result<Option<(When, When)>, String> {
    let touched = p.start.is_some() || p.end.is_some() || p.duration_min.is_some() || p.all_day.is_some();
    if !touched && old.is_some() {
        return Ok(None);
    }
    let all_day = p.all_day.unwrap_or_else(|| old.is_some_and(Event::all_day));
    let start_text = match (&p.start, old) {
        (Some(s), _) => Some(s.clone()),
        (None, Some(_)) => None,
        (None, None) => return Err("start is needed: the day and the time (2026-11-28T12:00), or the day with allDay".into()),
    };
    if all_day {
        let day = |t: &str| -> Result<NaiveDate, String> {
            let date = t.get(..10).unwrap_or(t);
            NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| format!("\"{t}\" is not a date: an all-day event takes a date such as 2026-11-28"))
        };
        let first = match (&start_text, old) {
            (Some(t), _) => day(t)?,
            (None, Some(e)) => e.starts(&cal.zone).date_naive(),
            _ => unreachable!(),
        };
        let last = match &p.end {
            Some(t) => day(t)?,
            None => match (old, &p.start) {
                (Some(e), None) => match e.end {
                    When::Day(d) => d - Duration::days(1),
                    _ => first,
                },
                _ => first,
            },
        };
        if last < first {
            return Err("the end is before the start".into());
        }
        far_check(cal, cal.day_start(first))?;
        return Ok(Some((When::Day(first), When::Day(last + Duration::days(1)))));
    }
    let (start, whole_day) = match &start_text {
        Some(t) => parse_instant(cal, t)?,
        None => (old.map(|e| e.starts(&cal.zone)).unwrap(), false),
    };
    if whole_day {
        return Err("a time is needed for the start (2026-11-28T12:00): or use allDay for an event without one".into());
    }
    let end = match (&p.end, p.duration_min) {
        (Some(t), _) => {
            let (end, whole) = parse_instant(cal, t)?;
            if whole {
                return Err("a time is needed for the end (2026-11-28T13:00)".into());
            }
            end
        }
        (None, Some(m)) => start + Duration::minutes(i64::from(m)),
        // Moving an event keeps its length; a new one gets the default.
        (None, None) => match old {
            Some(e) if !e.all_day() => start + (e.ends(&cal.zone) - e.starts(&cal.zone)),
            _ => start + Duration::minutes(i64::from(cal.config().default_duration_min)),
        },
    };
    if end <= start {
        return Err(format!("the end ({}) is not after the start ({}): check the times", at_text(end), at_text(start)));
    }
    far_check(cal, start)?;
    Ok(Some((When::At(start), When::At(end))))
}

fn far_check(cal: &Calendar, start: DateTime<FixedOffset>) -> Result<(), String> {
    if (start.to_utc() - cal.now()).abs() > MAX_DISTANCE {
        return Err(format!("{} is more than 5 years from now: check the year", at_text(start)));
    }
    Ok(())
}

fn when_json(w: &When) -> Value {
    match w {
        When::At(t) => json!({"dateTime": t.to_rfc3339()}),
        When::Day(d) => json!({"date": d.format("%Y-%m-%d").to_string()}),
    }
}

/// The description of an event: the Maps link on the first line, then the notes.
pub fn description(maps: &str, notes: &str) -> String {
    match (maps.is_empty(), notes.is_empty()) {
        (true, _) => notes.to_string(),
        (false, true) => maps.to_string(),
        (false, false) => format!("{maps}\n\n{notes}"),
    }
}

/// An old description as (the Maps link of its first line, the rest).
pub fn split_description(old: &str) -> (String, String) {
    let (first, rest) = old.split_once('\n').unwrap_or((old, ""));
    if MAPS_LINKS.iter().any(|p| first.trim().starts_with(p)) {
        (first.trim().to_string(), rest.trim().to_string())
    } else {
        (String::new(), old.trim().to_string())
    }
}

fn reminders(r: Option<u32>) -> Value {
    match r {
        Some(minutes) => json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": minutes}]}),
        None => json!({"useDefault": true}),
    }
}

/// The body of `events.insert`.
pub fn add_body(p: &Params, when: &(When, When), meet_id: &str) -> Value {
    let mut body = Map::new();
    body.insert("summary".into(), json!(p.title));
    body.insert("start".into(), when_json(&when.0));
    body.insert("end".into(), when_json(&when.1));
    if let Some(place) = &p.place {
        body.insert("location".into(), json!(place.text()));
    }
    let description = description(p.place.as_ref().map(|l| l.maps_url.as_str()).unwrap_or(""), p.notes.as_deref().unwrap_or(""));
    if !description.is_empty() {
        body.insert("description".into(), json!(description));
    }
    if let Some(r) = p.reminder {
        body.insert("reminders".into(), reminders(r));
    }
    if !p.guests.is_empty() {
        body.insert("attendees".into(), json!(p.guests.iter().map(|g| json!({"email": g})).collect::<Vec<_>>()));
    }
    if p.meet {
        body.insert("conferenceData".into(), super::google::meet_request(meet_id));
    }
    Value::Object(body)
}

/// What `update` writes, what writes the old values back, and whether guests are told.
pub struct Patch {
    pub body: Value,
    pub undo: Value,
    pub invite: bool,
    pub meet: bool,
}

pub fn patch(p: &Params, old: &Event, when: Option<(When, When)>, meet_id: &str) -> Patch {
    let (mut body, mut undo) = (Map::new(), Map::new());
    let mut set = |key: &str, new: Value| {
        undo.insert(key.to_string(), old.raw.get(key).cloned().unwrap_or(Value::Null));
        body.insert(key.to_string(), new);
    };
    if let Some(t) = &p.title {
        set("summary", json!(t));
    }
    if let Some((s, e)) = &when {
        set("start", when_json(s));
        set("end", when_json(e));
    }
    if let Some(place) = &p.place {
        set("location", json!(place.text()));
    }
    if p.place.is_some() || p.notes.is_some() {
        let (old_maps, old_notes) = split_description(&old.description);
        let maps = p.place.as_ref().map(|l| l.maps_url.clone()).unwrap_or(old_maps);
        let notes = p.notes.clone().unwrap_or(old_notes);
        let d = description(&maps, &notes);
        set("description", if d.is_empty() { Value::Null } else { json!(d) });
    }
    if let Some(r) = p.reminder {
        set("reminders", reminders(r));
    }
    let new_guests: Vec<&String> = p.guests.iter().filter(|g| !old.raw["attendees"].as_array().into_iter().flatten().any(|a| a["email"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(g)))).collect();
    if !new_guests.is_empty() {
        let mut list = old.raw["attendees"].as_array().cloned().unwrap_or_default();
        list.extend(new_guests.iter().map(|g| json!({"email": g})));
        set("attendees", Value::Array(list));
    }
    let meet = p.meet && old.meet.is_none();
    if meet {
        body.insert("conferenceData".into(), super::google::meet_request(meet_id));
    }
    Patch { invite: !new_guests.is_empty(), body: Value::Object(body), undo: Value::Object(undo), meet }
}

/// The copy that brings a deleted event back: what Google takes on insert, none of its own bookkeeping.
pub fn restore_body(raw: &Value) -> Value {
    let keep = ["summary", "description", "location", "start", "end", "reminders", "colorId", "transparency", "visibility"];
    let mut body: Map<String, Value> = keep.iter().filter_map(|k| raw.get(*k).map(|v| (k.to_string(), v.clone()))).collect();
    if let Some(list) = raw["attendees"].as_array() {
        let guests: Vec<Value> = list.iter().filter_map(|a| a.get("email").map(|e| json!({"email": e}))).collect();
        if !guests.is_empty() {
            body.insert("attendees".into(), Value::Array(guests));
        }
    }
    Value::Object(body)
}

fn reminder_text(r: &Option<Option<u32>>) -> String {
    match r {
        Some(Some(m)) => format!(", reminder {m} min"),
        Some(None) => ", Google's default reminder".into(),
        None => String::new(),
    }
}

/// `add "Lunch", Sat 28 Nov 12:00 to 13:00, at Thai Orchid, calendar Personal, reminder 30 min`.
pub fn summary_add(p: &Params, when: &(When, When), cal: &Calendar, calendar_name: &str) -> String {
    let e = Event { id: String::new(), calendar: String::new(), title: String::new(), start: when.0, end: when.1, location: String::new(), description: String::new(), meet: None, link: String::new(), guests: vec![], recurring: false, raw: Value::Null };
    let mut out = format!("add \"{}\", {}", p.title.as_deref().unwrap_or("?"), span_text(&e, &cal.zone));
    if let Some(place) = &p.place {
        out.push_str(&format!(", at {}", place.text()));
    }
    out.push_str(&format!(", calendar {calendar_name}"));
    out.push_str(&reminder_text(&p.reminder));
    if p.meet {
        out.push_str(", with a Meet link");
    }
    if !p.guests.is_empty() {
        out.push_str(&format!(", inviting {} by mail", p.guests.join(", ")));
    }
    out
}

/// `move "Lunch" from Sat 28 Nov 12:00 to 13:00 to Sun 29 Nov 12:00 to 13:00`, or the other changes in words.
pub fn summary_update(p: &Params, old: &Event, when: &Option<(When, When)>, cal: &Calendar) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some((s, e)) = when {
        let new = Event { start: *s, end: *e, ..old.clone() };
        parts.push(format!("move \"{}\" from {} to {}", old.title, span_text(old, &cal.zone), span_text(&new, &cal.zone)));
    }
    let mut others = Vec::new();
    if let Some(t) = &p.title {
        others.push(format!("title to \"{t}\""));
    }
    if let Some(l) = &p.place {
        others.push(format!("place to {}", l.text()));
    }
    if p.notes.is_some() {
        others.push("the notes".into());
    }
    if let Some(r) = &p.reminder {
        others.push(reminder_text(&Some(*r)).trim_start_matches(", ").to_string());
    }
    if p.meet && old.meet.is_none() {
        others.push("a Meet link".into());
    }
    if !p.guests.is_empty() {
        others.push(format!("guests {} (invited by mail)", p.guests.join(", ")));
    }
    if !others.is_empty() {
        let lead = if when.is_some() { "also change".to_string() } else { format!("change \"{}\" ({})", old.title, span_text(old, &cal.zone)) };
        parts.push(format!("{lead}: {}", others.join(", ")));
    }
    if parts.is_empty() {
        return format!("change \"{}\"", old.title);
    }
    parts.join("; ")
}

pub fn summary_delete(old: &Event, cal: &Calendar) -> String {
    format!("delete \"{}\", {}", old.title, span_text(old, &cal.zone))
}
