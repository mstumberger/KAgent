//! Calendars and events as the rest of the module sees them (calendar.md section 3): read
//! from Google's JSON, and written for the model in the machine's time zone, with the
//! weekday. Pure: the zone is passed in, so a test needs no clock and no locale.

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Timelike, Utc};
use serde_json::Value;

/// How an instant is shown: the machine's offset at that instant.
pub type Zone = std::sync::Arc<dyn Fn(DateTime<Utc>) -> FixedOffset + Send + Sync>;

/// One calendar of the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalInfo {
    pub id: String,
    pub name: String,
    pub color: String,
    pub writable: bool,
    pub primary: bool,
}

impl CalInfo {
    pub fn from_json(v: &Value) -> Option<Self> {
        let id = v.get("id")?.as_str()?.to_string();
        let role = v.get("accessRole").and_then(Value::as_str).unwrap_or("reader");
        let name = v.get("summaryOverride").or_else(|| v.get("summary")).and_then(Value::as_str).unwrap_or(&id).to_string();
        Some(Self {
            color: v.get("backgroundColor").and_then(Value::as_str).unwrap_or("").to_string(),
            writable: matches!(role, "owner" | "writer"),
            primary: v.get("primary").and_then(Value::as_bool).unwrap_or(false),
            name,
            id,
        })
    }
}

/// The start or the end of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    At(DateTime<FixedOffset>),
    /// A whole day; as an end it is the day after the last one (Google's rule).
    Day(NaiveDate),
}

impl When {
    pub fn from_json(v: &Value) -> Option<Self> {
        if let Some(t) = v.get("dateTime").and_then(Value::as_str) {
            return DateTime::parse_from_rfc3339(t).ok().map(Self::At);
        }
        let day = v.get("date").and_then(Value::as_str)?;
        NaiveDate::parse_from_str(day, "%Y-%m-%d").ok().map(Self::Day)
    }

    /// The instant it stands for in `zone`: midnight for a day.
    pub fn instant(&self, zone: &Zone) -> DateTime<FixedOffset> {
        match self {
            Self::At(t) => t.with_timezone(&zone(t.to_utc())),
            Self::Day(d) => {
                let utc = d.and_hms_opt(0, 0, 0).unwrap().and_utc();
                let offset = zone(utc);
                (*d).and_hms_opt(0, 0, 0).unwrap().and_local_timezone(offset).single().unwrap_or_else(|| utc.with_timezone(&offset))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    /// The calendar's id as the account knows it (the address, for the primary one).
    pub calendar: String,
    pub title: String,
    pub start: When,
    pub end: When,
    pub location: String,
    pub description: String,
    pub meet: Option<String>,
    pub link: String,
    /// Names, or the address when there is no name.
    pub guests: Vec<String>,
    /// Part of a series (`recurringEventId`): one instance of it.
    pub recurring: bool,
    /// Google's own JSON, kept so a deleted event can be restored.
    pub raw: Value,
}

impl Event {
    pub fn from_json(calendar: &str, v: &Value) -> Option<Self> {
        let meet = v.get("hangoutLink").and_then(Value::as_str).map(String::from).or_else(|| {
            v.pointer("/conferenceData/entryPoints")?.as_array()?.iter().find(|e| e["entryPointType"] == "video").and_then(|e| e["uri"].as_str()).map(String::from)
        });
        let guests = v
            .get("attendees")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter(|a| !a.get("self").and_then(Value::as_bool).unwrap_or(false) && !a.get("resource").and_then(Value::as_bool).unwrap_or(false))
                    .filter_map(|a| a.get("displayName").and_then(Value::as_str).filter(|n| !n.is_empty()).or_else(|| a.get("email").and_then(Value::as_str)))
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(),
            calendar: calendar.to_string(),
            title: v.get("summary").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("(no title)").to_string(),
            start: When::from_json(v.get("start")?)?,
            end: When::from_json(v.get("end")?)?,
            location: text("location"),
            description: text("description"),
            meet,
            link: text("htmlLink"),
            guests,
            recurring: v.get("recurringEventId").is_some(),
            raw: v.clone(),
        })
    }

    pub fn all_day(&self) -> bool {
        matches!(self.start, When::Day(_))
    }

    pub fn starts(&self, zone: &Zone) -> DateTime<FixedOffset> {
        self.start.instant(zone)
    }

    pub fn ends(&self, zone: &Zone) -> DateTime<FixedOffset> {
        self.end.instant(zone)
    }
}

/// Two timed events that share some time. All-day events overlap nothing: they are a note on the day.
pub fn overlap(a: &Event, b: &Event, zone: &Zone) -> Option<(DateTime<FixedOffset>, DateTime<FixedOffset>)> {
    if a.all_day() || b.all_day() {
        return None;
    }
    let (from, to) = (a.starts(zone).max(b.starts(zone)), a.ends(zone).min(b.ends(zone)));
    (from < to).then_some((from, to))
}

const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `Fri 28 Nov`.
pub fn day_text(d: NaiveDate) -> String {
    format!("{} {} {}", DAYS[d.weekday().num_days_from_monday() as usize], d.day(), MONTHS[d.month0() as usize])
}

/// `Fri 28 Nov 12:00`.
pub fn at_text(t: DateTime<FixedOffset>) -> String {
    format!("{} {:02}:{:02}", day_text(t.date_naive()), t.hour(), t.minute())
}

/// The time of an event for the model: `Fri 28 Nov 12:00 to 13:00`, `all day Fri 28 Nov`, or both
/// ends in full when it crosses midnight.
pub fn span_text(e: &Event, zone: &Zone) -> String {
    match (e.start, e.end) {
        (When::Day(a), When::Day(b)) => {
            let last = b - Duration::days(1);
            if last <= a {
                format!("all day {}", day_text(a))
            } else {
                format!("all day {} to {}", day_text(a), day_text(last))
            }
        }
        _ => {
            let (s, t) = (e.starts(zone), e.ends(zone));
            if s.date_naive() == t.date_naive() {
                format!("{} to {:02}:{:02}", at_text(s), t.hour(), t.minute())
            } else {
                format!("{} to {}", at_text(s), at_text(t))
            }
        }
    }
}

/// `+01:00`, for the header of a list.
pub fn offset_text(o: FixedOffset) -> String {
    let secs = o.local_minus_utc();
    format!("UTC{}{:02}:{:02}", if secs < 0 { '-' } else { '+' }, secs.abs() / 3600, secs.abs() % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn zone(hours: i32) -> Zone {
        std::sync::Arc::new(move |_| FixedOffset::east_opt(hours * 3600).unwrap())
    }

    fn ev(v: Value) -> Event {
        Event::from_json("primary", &v).unwrap()
    }

    #[test]
    fn a_timed_event_is_shown_in_the_machines_zone_with_the_weekday() {
        let e = ev(json!({"id": "a", "summary": "Lunch", "start": {"dateTime": "2026-11-28T11:00:00Z"}, "end": {"dateTime": "2026-11-28T12:00:00Z"}}));
        assert_eq!(span_text(&e, &zone(1)), "Sat 28 Nov 12:00 to 13:00");
        assert_eq!(span_text(&e, &zone(-5)), "Sat 28 Nov 06:00 to 07:00");
        assert_eq!(span_text(&e, &zone(13)), "Sun 29 Nov 00:00 to 01:00", "another zone, another day");
        let night = ev(json!({"id": "b", "start": {"dateTime": "2026-11-28T22:30:00+01:00"}, "end": {"dateTime": "2026-11-29T01:00:00+01:00"}}));
        assert_eq!(span_text(&night, &zone(1)), "Sat 28 Nov 22:30 to Sun 29 Nov 01:00");
        assert_eq!(night.title, "(no title)");
    }

    #[test]
    fn an_all_day_event_ends_the_day_before_googles_exclusive_end() {
        let one = ev(json!({"id": "a", "start": {"date": "2026-11-28"}, "end": {"date": "2026-11-29"}}));
        assert!(one.all_day());
        assert_eq!(span_text(&one, &zone(1)), "all day Sat 28 Nov");
        let three = ev(json!({"id": "b", "start": {"date": "2026-11-28"}, "end": {"date": "2026-12-01"}}));
        assert_eq!(span_text(&three, &zone(1)), "all day Sat 28 Nov to Mon 30 Nov");
        assert_eq!(one.starts(&zone(2)).to_rfc3339(), "2026-11-28T00:00:00+02:00", "midnight of the day, in the machine's zone");
    }

    #[test]
    fn overlaps_are_for_timed_events_that_share_time() {
        let z = zone(1);
        let a = ev(json!({"id": "a", "start": {"dateTime": "2026-11-28T10:00:00+01:00"}, "end": {"dateTime": "2026-11-28T11:00:00+01:00"}}));
        let b = ev(json!({"id": "b", "start": {"dateTime": "2026-11-28T10:30:00+01:00"}, "end": {"dateTime": "2026-11-28T12:00:00+01:00"}}));
        let c = ev(json!({"id": "c", "start": {"dateTime": "2026-11-28T11:00:00+01:00"}, "end": {"dateTime": "2026-11-28T12:00:00+01:00"}}));
        let day = ev(json!({"id": "d", "start": {"date": "2026-11-28"}, "end": {"date": "2026-11-29"}}));
        let (from, to) = overlap(&a, &b, &z).unwrap();
        assert_eq!((at_text(from), at_text(to)), ("Sat 28 Nov 10:30".into(), "Sat 28 Nov 11:00".into()));
        assert!(overlap(&a, &c, &z).is_none(), "back to back is not an overlap");
        assert!(overlap(&a, &day, &z).is_none(), "an all-day event overlaps nothing");
    }

    #[test]
    fn guests_meet_and_series_are_read_from_googles_json() {
        let e = ev(json!({
            "id": "x", "summary": "Sync", "start": {"dateTime": "2026-11-28T10:00:00+01:00"}, "end": {"dateTime": "2026-11-28T11:00:00+01:00"},
            "attendees": [{"email": "me@example.com", "self": true}, {"email": "miha@example.com", "displayName": "Miha"}, {"email": "ana@example.com"}, {"email": "room@x", "resource": true}],
            "conferenceData": {"entryPoints": [{"entryPointType": "phone", "uri": "tel:1"}, {"entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij"}]},
            "recurringEventId": "series", "location": "Room 4", "htmlLink": "https://calendar.google.com/event?eid=x"
        }));
        assert_eq!(e.guests, ["Miha", "ana@example.com"]);
        assert_eq!(e.meet.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert!(e.recurring);
        assert_eq!((e.location.as_str(), e.link.as_str()), ("Room 4", "https://calendar.google.com/event?eid=x"));
        assert!(Event::from_json("p", &json!({"id": "y", "start": {"foo": 1}, "end": {}})).is_none(), "no start, no event");
    }

    #[test]
    fn a_calendar_is_writable_for_an_owner_or_a_writer() {
        let c = |role: &str| CalInfo::from_json(&json!({"id": "i", "summary": "S", "accessRole": role})).unwrap().writable;
        assert!(c("owner") && c("writer") && !c("reader") && !c("freeBusyReader"));
        let p = CalInfo::from_json(&json!({"id": "me@example.com", "summary": "me", "summaryOverride": "Personal", "accessRole": "owner", "primary": true, "backgroundColor": "#9fe1e7"})).unwrap();
        assert_eq!((p.name.as_str(), p.primary, p.color.as_str()), ("Personal", true, "#9fe1e7"));
        assert_eq!(offset_text(FixedOffset::east_opt(3600).unwrap()), "UTC+01:00");
        assert_eq!(offset_text(FixedOffset::west_opt(19800).unwrap()), "UTC-05:30");
    }
}
