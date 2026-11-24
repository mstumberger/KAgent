//! Schedules for tasks, parsed from short human text (docs/specs/tasks.md):
//! once:   "in 20m", "in 2h", "in 1d", "14:30", "at 14:30", "tomorrow 07:00",
//!         "2026-11-25 09:00"
//! repeat: "every 30m", "every 2h", "hourly", "daily 09:00", "every day at 9:00",
//!         "weekdays 08:30", "weekends 10:00", "mon,wed,fri 18:00", "every monday 9:00"
//! All times are local.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Schedule {
    /// Once, at this moment.
    At { at: DateTime<Local> },
    /// Repeats every N minutes.
    Every { minutes: u32 },
    /// At a time of day on some weekdays (0 = Monday .. 6 = Sunday; empty = every day).
    Daily { time: String, days: Vec<u8> },
}

impl Schedule {
    pub fn repeats(&self) -> bool {
        !matches!(self, Schedule::At { .. })
    }

    /// Short description for lists ("daily 09:00", "weekdays 08:30", "once 25 Nov 09:00").
    pub fn describe(&self) -> String {
        match self {
            Schedule::At { at } => format!("once {}", at.format("%a %d %b %H:%M")),
            Schedule::Every { minutes } if minutes % 1440 == 0 => format!("every {} d", minutes / 1440),
            Schedule::Every { minutes } if minutes % 60 == 0 => format!("every {} h", minutes / 60),
            Schedule::Every { minutes } => format!("every {minutes} min"),
            Schedule::Daily { time, days } => {
                let d: &[u8] = days;
                let which = match d {
                    [] => "daily".to_string(),
                    [0, 1, 2, 3, 4] => "weekdays".into(),
                    [5, 6] => "weekends".into(),
                    _ => d.iter().map(|x| ["mon", "tue", "wed", "thu", "fri", "sat", "sun"][*x as usize % 7]).collect::<Vec<_>>().join(","),
                };
                format!("{which} {time}")
            }
        }
    }

    /// First run strictly after `from` (None: a one-off already in the past).
    pub fn next_after(&self, from: DateTime<Local>) -> Option<DateTime<Local>> {
        match self {
            Schedule::At { at } => (*at > from).then_some(*at),
            Schedule::Every { minutes } => Some(from + Duration::minutes((*minutes).max(1) as i64)),
            Schedule::Daily { time, days } => {
                let t = NaiveTime::parse_from_str(time, "%H:%M").ok()?;
                (0..8).find_map(|offset| {
                    let date = from.date_naive() + Duration::days(offset);
                    let wd = date.weekday().num_days_from_monday() as u8;
                    if !days.is_empty() && !days.contains(&wd) {
                        return None;
                    }
                    let at = local(date.and_time(t))?;
                    (at > from).then_some(at)
                })
            }
        }
    }
}

fn local(n: NaiveDateTime) -> Option<DateTime<Local>> {
    Local.from_local_datetime(&n).earliest()
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim().trim_start_matches("at ").trim();
    if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M") {
        return Some(t);
    }
    // "9", "9am", "7pm"
    let (num, pm) = if let Some(n) = s.strip_suffix("pm") { (n.trim(), true) } else if let Some(n) = s.strip_suffix("am") { (n.trim(), false) } else { (s, false) };
    let h: u32 = num.parse().ok()?;
    let h = if pm && h < 12 { h + 12 } else { h };
    NaiveTime::from_hms_opt(h, 0, 0)
}

fn parse_duration(s: &str) -> Option<u32> {
    let s = s.trim().replace(' ', "");
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u32 = num.parse().ok()?;
    let mins = match unit {
        "m" | "min" | "mins" | "minute" | "minutes" => n,
        "h" | "hr" | "hrs" | "hour" | "hours" => n * 60,
        "d" | "day" | "days" => n * 1440,
        _ => return None,
    };
    (mins > 0).then_some(mins)
}

fn weekday(s: &str) -> Option<u8> {
    let w: Weekday = s.trim().trim_end_matches('s').parse().ok()?;
    Some(w.num_days_from_monday() as u8)
}

/// Parse a schedule; `None` input text or "" means no schedule (a to-do).
pub fn parse(text: &str, now: DateTime<Local>) -> Result<Option<Schedule>, String> {
    let t = text.trim().to_lowercase();
    if t.is_empty() || t == "none" || t == "manual" || t == "todo" {
        return Ok(None);
    }
    let bad = || format!("unknown schedule '{text}'. Use e.g. 'in 20m', '14:30', 'tomorrow 07:00', '2026-11-25 09:00', 'every 30m', 'daily 09:00', 'weekdays 08:30', 'mon,fri 18:00'");
    // A bare duration ("1m", "30 min") means "in".
    if let Some(m) = parse_duration(&t) {
        return Ok(Some(Schedule::At { at: now + Duration::minutes(m as i64) }));
    }
    if let Some(rest) = t.strip_prefix("in ") {
        let m = parse_duration(rest).ok_or_else(bad)?;
        return Ok(Some(Schedule::At { at: now + Duration::minutes(m as i64) }));
    }
    if t == "hourly" {
        return Ok(Some(Schedule::Every { minutes: 60 }));
    }
    if let Some(rest) = t.strip_prefix("every ") {
        if let Some(m) = parse_duration(rest) {
            return Ok(Some(Schedule::Every { minutes: m }));
        }
        if let Some(rest) = rest.strip_prefix("day") {
            let time = parse_time(rest).ok_or_else(bad)?;
            return Ok(Some(Schedule::Daily { time: time.format("%H:%M").to_string(), days: vec![] }));
        }
        // "every monday 9:00"
        let (day, time) = rest.split_once(' ').ok_or_else(bad)?;
        let d = weekday(day).ok_or_else(bad)?;
        let time = parse_time(time).ok_or_else(bad)?;
        return Ok(Some(Schedule::Daily { time: time.format("%H:%M").to_string(), days: vec![d] }));
    }
    if let Some(rest) = t.strip_prefix("tomorrow") {
        let time = parse_time(rest).unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).expect("valid"));
        let at = local((now.date_naive() + Duration::days(1)).and_time(time)).ok_or_else(bad)?;
        return Ok(Some(Schedule::At { at }));
    }
    for (word, days) in [("daily", vec![]), ("weekdays", vec![0, 1, 2, 3, 4]), ("weekends", vec![5, 6])] {
        if let Some(rest) = t.strip_prefix(word) {
            let time = parse_time(rest).ok_or_else(bad)?;
            return Ok(Some(Schedule::Daily { time: time.format("%H:%M").to_string(), days }));
        }
    }
    // "mon,wed,fri 18:00"
    if let Some((list, time)) = t.split_once(' ') {
        let days: Option<Vec<u8>> = list.split(',').map(weekday).collect();
        if let (Some(mut days), Some(time)) = (days, parse_time(time)) {
            days.sort_unstable();
            days.dedup();
            return Ok(Some(Schedule::Daily { time: time.format("%H:%M").to_string(), days }));
        }
    }
    // Absolute date-time.
    for f in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%d.%m.%Y %H:%M"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(&t, f) {
            let at = local(n).ok_or_else(bad)?;
            return if at > now { Ok(Some(Schedule::At { at })) } else { Err(format!("{text} is in the past")) };
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(&t, "%Y-%m-%d") {
        let at = local(d.and_hms_opt(9, 0, 0).expect("valid")).ok_or_else(bad)?;
        return Ok(Some(Schedule::At { at }));
    }
    // A bare time: today if still ahead, else tomorrow.
    if let Some(time) = parse_time(&t) {
        let today = local(now.date_naive().and_time(time)).ok_or_else(bad)?;
        let at = if today > now { today } else { today + Duration::days(1) };
        return Ok(Some(Schedule::At { at }));
    }
    Err(bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Local> {
        // Monday 2026-11-23 10:00 local.
        Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
    }

    #[test]
    fn parses_once() {
        let n = now();
        assert_eq!(parse("in 20m", n).unwrap(), Some(Schedule::At { at: n + Duration::minutes(20) }));
        assert_eq!(parse("in 2 hours", n).unwrap(), Some(Schedule::At { at: n + Duration::hours(2) }));
        assert_eq!(parse("1m", n).unwrap(), Some(Schedule::At { at: n + Duration::minutes(1) }));
        assert_eq!(parse("30 min", n).unwrap(), Some(Schedule::At { at: n + Duration::minutes(30) }));
        assert_eq!(parse("14:30", n).unwrap(), Some(Schedule::At { at: Local.with_ymd_and_hms(2026, 11, 23, 14, 30, 0).unwrap() }));
        assert_eq!(parse("09:00", n).unwrap(), Some(Schedule::At { at: Local.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap() }), "past time -> tomorrow");
        assert_eq!(parse("tomorrow 7am", n).unwrap(), Some(Schedule::At { at: Local.with_ymd_and_hms(2026, 11, 24, 7, 0, 0).unwrap() }));
        assert_eq!(parse("2026-11-25 09:15", n).unwrap(), Some(Schedule::At { at: Local.with_ymd_and_hms(2026, 11, 25, 9, 15, 0).unwrap() }));
        assert!(parse("2026-01-01 09:00", n).unwrap_err().contains("past"));
        assert_eq!(parse("", n).unwrap(), None, "no schedule = to-do");
        assert!(parse("whenever", n).is_err());
    }

    #[test]
    fn parses_repeats_and_next_runs() {
        let n = now();
        assert_eq!(parse("every 30m", n).unwrap(), Some(Schedule::Every { minutes: 30 }));
        assert_eq!(parse("hourly", n).unwrap(), Some(Schedule::Every { minutes: 60 }));
        let wd = parse("weekdays 08:30", n).unwrap().unwrap();
        assert_eq!(wd, Schedule::Daily { time: "08:30".into(), days: vec![0, 1, 2, 3, 4] });
        assert_eq!(wd.describe(), "weekdays 08:30");
        assert_eq!(wd.next_after(n).unwrap(), Local.with_ymd_and_hms(2026, 11, 24, 8, 30, 0).unwrap(), "Tuesday morning");
        let fri = Local.with_ymd_and_hms(2026, 11, 27, 9, 0, 0).unwrap();
        assert_eq!(wd.next_after(fri).unwrap(), Local.with_ymd_and_hms(2026, 11, 30, 8, 30, 0).unwrap(), "skips the weekend");
        let d = parse("every day at 9:00", n).unwrap().unwrap();
        assert_eq!(d.next_after(n).unwrap(), Local.with_ymd_and_hms(2026, 11, 24, 9, 0, 0).unwrap());
        assert_eq!(parse("mon,fri 18:00", n).unwrap(), Some(Schedule::Daily { time: "18:00".into(), days: vec![0, 4] }));
        assert_eq!(parse("every monday 9", n).unwrap(), Some(Schedule::Daily { time: "09:00".into(), days: vec![0] }));
        assert_eq!(Schedule::At { at: n }.next_after(n), None, "one-off in the past");
        assert_eq!(Schedule::Every { minutes: 120 }.describe(), "every 2 h");
    }
}
