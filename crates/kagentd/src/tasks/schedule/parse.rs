//! The words a schedule is written in (automations.md section 2): what `parse` reads.
//! Once: "in 20m", "14:30", "tomorrow 07:00", "monday 09:00", a date. Repeating:
//! "every 30m", "every 2 hours", "hourly", "daily 09:00", "weekdays 08:30",
//! "mon,fri 18:00", "every monday 09:00", "every 2 days at 09:00", "every 2 weeks on
//! monday 09:00", "every 3 months". Cron: five fields, or the word "cron" before them.

use super::*;
use chrono::Weekday;

/// Parse a schedule from words or cron. "", "none", "manual" and "todo" mean none (a
/// to-do). Text that does not parse is an error with three examples.
pub fn parse(text: &str, now: DateTime<Local>) -> Result<Option<Schedule>, String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() || ["none", "manual", "todo"].contains(&text.as_str()) {
        return Ok(None);
    }
    parse_some(&text, now).map(Some)
}

fn parse_some(t: &str, now: DateTime<Local>) -> Result<Schedule, String> {
    let bad = || format!("unknown schedule '{t}'. Use e.g. {EXAMPLES}");
    // Cron: after the word "cron", or five fields with a `*` or a `/`.
    let after_cron = t.strip_prefix("cron ");
    let fields: Vec<&str> = after_cron.unwrap_or(t).split_whitespace().collect();
    if after_cron.is_some() || (fields.len() == 5 && fields.iter().any(|f| f.contains('*') || f.contains('/'))) {
        let expr = fields.join(" ");
        return cron::parse(&expr).map(|_| Schedule::Cron { expr: expr.clone() }).map_err(|e| format!("cron '{expr}': {e}. Use e.g. {EXAMPLES}"));
    }
    // Once: a duration.
    if let Some((n, unit)) = duration_token(t) {
        return Ok(after(now, n, unit));
    }
    if let Some(rest) = t.strip_prefix("in ") {
        let (n, unit) = duration_token(rest.trim()).ok_or_else(bad)?;
        return Ok(after(now, n, unit));
    }
    if t == "hourly" {
        return Ok(Schedule::interval(Unit::Hour, 1, None, None, None));
    }
    if let Some(rest) = t.strip_prefix("tomorrow") {
        let time = time_arg(rest).unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).expect("a valid time"));
        let at = local((now.date_naive() + Duration::days(1)).and_time(time)).ok_or_else(bad)?;
        return Ok(Schedule::Once { at: stamp(at) });
    }
    if let Some(rest) = t.strip_prefix("every ") {
        return every(rest.trim(), now).ok_or_else(bad);
    }
    for (word, days) in [("daily", None), ("weekdays", Some(vec![0, 1, 2, 3, 4])), ("weekends", Some(vec![5, 6]))] {
        if let Some(rest) = t.strip_prefix(word).filter(|rest| rest.is_empty() || rest.starts_with(' ')) {
            let time = time_arg(rest).unwrap_or_else(default_time);
            return Ok(Schedule::interval(Unit::Day, 1, Some(hhmm_string(time)), days, None));
        }
    }
    // A list of days: repeating, on those days.
    if let Some((list, time)) = t.split_once(' ') {
        let parts: Vec<&str> = list.split(',').collect();
        if parts.len() >= 2 {
            let days: Option<Vec<u8>> = parts.iter().map(|p| weekday_num(p)).collect();
            if let (Some(mut days), Some(time)) = (days, time_arg(time)) {
                days.sort_unstable();
                days.dedup();
                return Ok(Schedule::interval(Unit::Day, 1, Some(hhmm_string(time)), Some(days), None));
            }
        }
    }
    // Absolute. A time that is over is refused.
    for format in ["%Y-%m-%d %H:%M", "%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%d.%m.%Y %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(t, format) {
            let at = stamp(local(naive).ok_or_else(bad)?);
            return if at > now { Ok(Schedule::Once { at }) } else { Err(format!("{t} is in the past")) };
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(t, "%Y-%m-%d") {
        let at = local(date.and_hms_opt(9, 0, 0).ok_or_else(bad)?).ok_or_else(bad)?;
        return if at > now { Ok(Schedule::Once { at: stamp(at) }) } else { Err(format!("{t} is in the past")) };
    }
    // One weekday: once, the next such day.
    if let Some((day, time)) = t.split_once(' ') {
        if let (Some(day), Some(time)) = (weekday_num(day), time_arg(time)) {
            let date = first_weekday(now.date_naive(), day);
            let week = local((date + Duration::days(7)).and_time(time)).ok_or_else(bad)?;
            let at = local(date.and_time(time)).filter(|at| *at > now).unwrap_or(week);
            return Ok(Schedule::Once { at: stamp(at) });
        }
    }
    // A clock time: today while that is ahead, else tomorrow.
    if let Some(time) = time_arg(t) {
        let today = local(now.date_naive().and_time(time)).ok_or_else(bad)?;
        let at = if today > now { today } else { local((now.date_naive() + Duration::days(1)).and_time(time)).ok_or_else(bad)? };
        return Ok(Schedule::Once { at: stamp(at) });
    }
    Err(bad())
}

fn after(now: DateTime<Local>, n: u32, unit: Unit) -> Schedule {
    let step = match unit {
        Unit::Minute => Duration::minutes(i64::from(n)),
        Unit::Hour => Duration::hours(i64::from(n)),
        Unit::Day => Duration::days(i64::from(n)),
        _ => Duration::weeks(i64::from(n)),
    };
    Schedule::Once { at: stamp(now + step) }
}

/// "20m", "2 hours", "30min": a count and a unit, nothing else.
fn duration_token(text: &str) -> Option<(u32, Unit)> {
    let text = text.replace(' ', "");
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let n: u32 = text[..split].parse().ok()?;
    let unit = unit_word(&text[split..])?;
    (n > 0 && n <= MAX_N).then_some((n, unit))
}

fn unit_word(word: &str) -> Option<Unit> {
    match word {
        "m" | "min" | "mins" | "minute" | "minutes" => Some(Unit::Minute),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(Unit::Hour),
        "d" | "day" | "days" => Some(Unit::Day),
        "w" | "wk" | "week" | "weeks" => Some(Unit::Week),
        "mo" | "month" | "months" => Some(Unit::Month),
        _ => None,
    }
}

/// What follows "every".
fn every(rest: &str, now: DateTime<Local>) -> Option<Schedule> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let first = *words.first()?;
    if let Ok(n) = first.parse::<u32>() {
        return every_n(n, unit_word(*words.get(1)?)?, &words[2..], now);
    }
    if let Some((n, unit)) = duration_token(first) {
        return every_n(n, unit, &words[1..], now);
    }
    if first == "day" || first == "days" {
        return every_n(1, Unit::Day, &words[1..], now);
    }
    // "every monday 09:00", "every weekday 08:30", "every weekend 10:00"
    let days: Vec<u8> = match first {
        "weekday" | "weekdays" => vec![0, 1, 2, 3, 4],
        "weekend" | "weekends" => vec![5, 6],
        _ => vec![weekday_num(first)?],
    };
    let time = tail_time(&words[1..])?;
    Some(Schedule::interval(Unit::Day, 1, time.map(hhmm_string), Some(days), None))
}

/// `n` of `unit`, then the words that are left.
fn every_n(n: u32, unit: Unit, tail: &[&str], now: DateTime<Local>) -> Option<Schedule> {
    if n == 0 || n > MAX_N {
        return None;
    }
    if matches!(unit, Unit::Minute | Unit::Hour) {
        return tail.is_empty().then(|| Schedule::interval(unit, n, None, None, None));
    }
    let mut rest = tail;
    let mut day = now.weekday().num_days_from_monday() as u8;
    if unit == Unit::Week {
        // "on monday"; without a day, the weekday of today.
        if rest.first() == Some(&"on") {
            day = weekday_num(*rest.get(1)?)?;
            rest = &rest[2..];
        }
    }
    let time = tail_time(rest)?;
    let at = time.map(hhmm_string);
    let today = now.date_naive();
    match unit {
        Unit::Day if n == 1 => Some(Schedule::interval(Unit::Day, 1, at, None, None)),
        Unit::Day => Some(Schedule::interval(Unit::Day, n, at, None, Some(today))),
        // The periods count from the first run: the first such day whose time is still ahead.
        Unit::Week => Some(Schedule::interval(Unit::Week, n, at, Some(vec![day]), Some(first_ahead(today, day, time.unwrap_or_else(default_time), now)))),
        _ => Some(Schedule::interval(Unit::Month, n, at, None, Some(today))),
    }
}

/// The clock time of the words after a unit: none (the default), a time, or "at" and a
/// time; None when the words are something else.
fn tail_time(words: &[&str]) -> Option<Option<NaiveTime>> {
    match words {
        [] => Some(None),
        [time] | ["at", time] => time_arg(time).map(Some),
        _ => None,
    }
}

fn default_time() -> NaiveTime {
    NaiveTime::from_hms_opt(8, 0, 0).expect("a valid time")
}

/// The first day from `today` on that is `day`.
fn first_weekday(today: NaiveDate, day: u8) -> NaiveDate {
    let ahead = (i64::from(day) - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
    today + Duration::days(ahead)
}

/// The day the periods of a weekly schedule count from: the first `day` from `today` on
/// whose `time` is still ahead of `now`.
fn first_ahead(today: NaiveDate, day: u8, time: NaiveTime, now: DateTime<Local>) -> NaiveDate {
    let date = first_weekday(today, day);
    if local(date.and_time(time)).is_some_and(|at| at > now) {
        date
    } else {
        date + Duration::days(7)
    }
}

fn weekday_num(word: &str) -> Option<u8> {
    let weekday: Weekday = word.trim().trim_end_matches('s').parse().ok()?;
    Some(weekday.num_days_from_monday() as u8)
}

/// A clock time as people write it: "14:30", "at 14:30", "9", "9am", "7pm".
fn time_arg(text: &str) -> Option<NaiveTime> {
    let text = text.trim().trim_start_matches("at ").trim();
    if let Ok(time) = NaiveTime::parse_from_str(text, "%H:%M") {
        return Some(time);
    }
    let (number, pm) = if let Some(n) = text.strip_suffix("pm") {
        (n.trim(), true)
    } else if let Some(n) = text.strip_suffix("am") {
        (n.trim(), false)
    } else {
        (text, false)
    };
    let hour: u32 = number.parse().ok()?;
    let hour = match (pm, hour) {
        (true, 1..=11) => hour + 12,
        (false, 12) => 0,
        _ => hour,
    };
    NaiveTime::from_hms_opt(hour, 0, 0)
}
