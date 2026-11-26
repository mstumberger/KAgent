//! Schedules for tasks (automations.md section 2): the three variants, parsed from short
//! human text or a 5-field cron expression, described in a few words, and `next_run`.
//! All times are local. Records from before S28 load through the aliases `At`, `Every`
//! and `Daily`.
//!
//! In words:
//!   once:   "in 20m", "14:30", "at 14:30", "tomorrow 07:00", "monday 09:00",
//!           "2026-11-25 09:00"
//!   repeat: "every 30m", "every 2 hours", "hourly", "daily 09:00", "every day at 09:00",
//!           "weekdays 08:30", "weekends 10:00", "mon,fri 18:00", "every monday 09:00",
//!           "every 2 days at 09:00", "every 2 weeks on monday 09:00", "every 3 months"
//!   cron:   "0 9 * * 1-5", "*/20 9-17 * * 1-5", "cron 30 8 1 * *"

use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, SubsecRound, TimeZone};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::cron;

mod parse;
pub use parse::parse;

#[cfg(test)]
mod tests;

/// The largest `n` of an interval.
const MAX_N: u32 = 200;

/// The clock time of a day-and-longer interval that names none.
const DEFAULT_AT: &str = "08:00";

/// Three examples, in the message about text that does not parse.
pub const EXAMPLES: &str = "'in 20m', 'weekdays 08:30', '0 9 * * 1-5'";

/// What an empty schedule means, in lists.
pub const ON_DEMAND: &str = "to-do (run on demand)";

const DAY_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    Minute,
    Hour,
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Schedule {
    /// Once, at this moment.
    Once { at: DateTime<Local> },
    /// Every `n` of `unit`. `at` (HH:MM) and `weekdays` (0 = Monday .. 6 = Sunday) are
    /// for days and longer; `from` is the first day of the periods of an interval of
    /// several days, of weeks (the weekday of the first run) or of months (its day).
    Interval {
        unit: Unit,
        n: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        at: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        weekdays: Option<Vec<u8>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<NaiveDate>,
    },
    /// 5 fields: minute, hour, day of month, month, day of week (cron.rs).
    Cron { expr: String },
}

/// `now`, to the second: what the daemon stores.
pub fn stamp(now: DateTime<Local>) -> DateTime<Local> {
    now.trunc_subsecs(0)
}

/// A wall-clock time of this zone. The earlier one when it comes twice (the clocks went
/// back); a time the clocks skipped (they went forward) is read with the offset of the
/// day before, so it lands an hour later.
pub(super) fn local(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => Some(at),
        LocalResult::None => {
            let offset = Local.offset_from_utc_datetime(&(naive - Duration::days(1)));
            Some(Local.from_utc_datetime(&(naive - offset)))
        }
    }
}

/// "HH:MM" as a schedule stores it.
fn hhmm(text: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text.trim(), "%H:%M").ok()
}

fn hhmm_string(time: NaiveTime) -> String {
    time.format("%H:%M").to_string()
}

/// The days of a week in a few words: nothing for every day, "weekdays", "weekends", or
/// "mon, fri".
fn day_list(days: Option<&[u8]>) -> Option<String> {
    let mut days: Vec<u8> = days?.to_vec();
    if days.is_empty() {
        return None;
    }
    days.sort_unstable();
    days.dedup();
    if days == [0, 1, 2, 3, 4] {
        return Some("weekdays".into());
    }
    if days == [5, 6] {
        return Some("weekends".into());
    }
    Some(days.iter().map(|d| DAY_NAMES[(*d as usize) % 7]).collect::<Vec<_>>().join(", "))
}

// What a stored or sent schedule holds, field by field: `deny_unknown_fields` cannot be
// combined with the type tag, so each variant has its own struct.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OnceAt {
    at: DateTime<Local>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IntervalFields {
    unit: Unit,
    n: u32,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    weekdays: Option<Vec<u8>>,
    #[serde(default)]
    from: Option<NaiveDate>,
}

impl From<IntervalFields> for Schedule {
    fn from(f: IntervalFields) -> Self {
        Self::Interval { unit: f.unit, n: f.n, at: f.at, weekdays: f.weekdays, from: f.from }
    }
}

/// The shape before S28.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OldEvery {
    minutes: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OldDaily {
    time: String,
    days: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CronFields {
    expr: String,
}

fn read<T: serde::de::DeserializeOwned>(kind: &str, fields: &Value) -> Result<T, String> {
    serde_json::from_value(fields.clone()).map_err(|e| format!("a {kind} schedule: {e}"))
}

impl Schedule {
    /// Every variant except `once`.
    pub fn repeats(&self) -> bool {
        !matches!(self, Self::Once { .. })
    }

    pub fn interval(unit: Unit, n: u32, at: Option<String>, weekdays: Option<Vec<u8>>, from: Option<NaiveDate>) -> Self {
        Self::Interval { unit, n, at, weekdays, from }
    }

    /// A schedule from what a file, a request or a tool holds. The type names the
    /// variant (`once`, `interval`, `cron`; before S28: `At`, `Every`, `Daily`); a field
    /// the variant does not have, or a value out of range, is an error.
    pub fn from_value(value: Value) -> Result<Self, String> {
        let Value::Object(mut fields) = value else {
            return Err("a schedule must be an object with a type".into());
        };
        let kind = fields.remove("type").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
        let rest = Value::Object(fields);
        // The tags before S28 are read in any case: files hold both `At` and `at`.
        let schedule = match kind.to_ascii_lowercase().as_str() {
            "once" | "at" => Self::Once { at: read::<OnceAt>(&kind, &rest)?.at },
            "interval" => read::<IntervalFields>(&kind, &rest)?.into(),
            "every" => Self::interval(Unit::Minute, read::<OldEvery>(&kind, &rest)?.minutes, None, None, None),
            "daily" => {
                let daily = read::<OldDaily>(&kind, &rest)?;
                Self::interval(Unit::Day, 1, Some(daily.time), (!daily.days.is_empty()).then_some(daily.days), None)
            }
            "cron" => Self::Cron { expr: read::<CronFields>(&kind, &rest)?.expr },
            _ => return Err(format!("unknown schedule type '{kind}'")),
        };
        schedule.validate()?;
        Ok(schedule)
    }

    /// As the spec allows a stored schedule to be (automations.md section 1).
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Once { .. } => Ok(()),
            Self::Cron { expr } => cron::parse(expr).map(|_| ()),
            Self::Interval { unit, n, at, weekdays, from } => {
                if !(1..=MAX_N).contains(n) {
                    return Err(format!("interval: n must be between 1 and {MAX_N}, not {n}"));
                }
                if at.as_deref().is_some_and(|t| hhmm(t).is_none()) {
                    return Err(format!("interval: at must be HH:MM, not '{}'", at.clone().unwrap_or_default()));
                }
                if weekdays.as_deref().is_some_and(|days| days.iter().any(|d| *d > 6)) {
                    return Err("interval: weekdays are 0 (Monday) to 6 (Sunday)".into());
                }
                let extra = match unit {
                    Unit::Minute | Unit::Hour if at.is_some() || weekdays.is_some() || from.is_some() => {
                        Some("a schedule in minutes or hours takes no at, weekdays or from".to_string())
                    }
                    Unit::Month if weekdays.is_some() => Some("a monthly interval takes no weekdays".to_string()),
                    _ => None,
                };
                extra.map_or(Ok(()), Err)
            }
        }
    }

    /// A few words for lists: "once Wed 25 Nov 09:00", "every 30 minutes", "weekdays at
    /// 08:30", "every 2 weeks on mon at 09:00", "cron 0 9 * * 1-5".
    pub fn describe(&self) -> String {
        match self {
            Self::Once { at } => format!("once {}", at.format("%a %d %b %H:%M")),
            Self::Cron { expr } => format!("cron {expr}"),
            Self::Interval { unit, n, at, weekdays, from } => {
                let time = hhmm(at.as_deref().unwrap_or(DEFAULT_AT)).map(hhmm_string).unwrap_or_default();
                let days = day_list(weekdays.as_deref());
                let on = days.as_deref().map(|d| format!(" on {d}")).unwrap_or_default();
                match unit {
                    Unit::Minute if *n == 1 => "every minute".into(),
                    Unit::Minute => format!("every {n} minutes"),
                    Unit::Hour if *n == 1 => "hourly".into(),
                    Unit::Hour => format!("every {n} hours"),
                    Unit::Day if *n == 1 => days.map_or_else(|| format!("daily at {time}"), |d| format!("{d} at {time}")),
                    Unit::Day => format!("every {n} days{on} at {time}"),
                    Unit::Week if *n == 1 => format!("every week{on} at {time}"),
                    Unit::Week => format!("every {n} weeks{on} at {time}"),
                    Unit::Month if *n == 1 => format!("monthly{} at {time}", from.map(|d| format!(" on day {}", d.day())).unwrap_or_default()),
                    Unit::Month => format!("every {n} months{} at {time}", from.map(|d| format!(" on day {}", d.day())).unwrap_or_default()),
                }
            }
        }
    }

    /// The next run strictly after `after`: None when a `once` is over, or when a cron
    /// expression matches nothing within 366 days. Computed from `after`, never from
    /// when a run ended.
    pub fn next_run(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        match self {
            Self::Once { at } => (*at > after).then_some(*at),
            Self::Cron { expr } => cron::parse(expr).ok().and_then(|c| c.next(after)),
            Self::Interval { unit: Unit::Minute, n, .. } => Some(after + Duration::minutes(i64::from(*n))),
            Self::Interval { unit: Unit::Hour, n, .. } => Some(after + Duration::hours(i64::from(*n))),
            Self::Interval { unit, n, at, weekdays, from } => next_day_like(*unit, *n, at.as_deref(), weekdays.as_deref(), *from, after),
        }
    }
}

impl<'de> Deserialize<'de> for Schedule {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(serde::de::Error::custom)
    }
}

/// Days, weeks and months: the first matching day after `after`. The periods count from
/// `from` (an epoch Monday for days and weeks, January 1st 1970 for months, when a
/// schedule names none); at most the period that holds `after` and the one after it are
/// looked at.
fn next_day_like(unit: Unit, n: u32, at: Option<&str>, days: Option<&[u8]>, from: Option<NaiveDate>, after: DateTime<Local>) -> Option<DateTime<Local>> {
    let time = hhmm(at.unwrap_or(DEFAULT_AT))?;
    let today = after.date_naive();
    // An interval that names no anchor counts from the epoch: a Monday for days and
    // weeks, the 1st of January for months.
    let anchor = from.unwrap_or(match unit {
        Unit::Month => NaiveDate::from_ymd_opt(1970, 1, 1).expect("a date"),
        _ => NaiveDate::from_ymd_opt(1970, 1, 5).expect("a date"),
    });
    let n = i64::from(n);
    // A day interval names its days of the week: the next one can be a week away.
    let passes = if unit == Unit::Day { 8 } else { 2 };
    for i in 0..passes {
        let start = match unit {
            Unit::Day => anchor.checked_add_signed(Duration::days((today - anchor).num_days().div_euclid(n) * n + i * n))?,
            Unit::Week => anchor.checked_add_signed(Duration::days((today - anchor).num_days().div_euclid(7 * n) * 7 * n + i * 7 * n))?,
            _ => {
                // Months: the day of the anchor, or the last day of a shorter month.
                let months = (i64::from(today.year()) - i64::from(anchor.year())) * 12 + i64::from(today.month0()) - i64::from(anchor.month0());
                let k = months.div_euclid(n) * n + i * n;
                let year = anchor.year() + i32::try_from((i64::from(anchor.month0()) + k).div_euclid(12)).ok()?;
                let month = u32::try_from((i64::from(anchor.month0()) + k).rem_euclid(12)).ok()? + 1;
                let last = last_day(year, month)?;
                NaiveDate::from_ymd_opt(year, month, anchor.day().min(last))?
            }
        };
        for j in 0..if unit == Unit::Week { 7 } else { 1 } {
            let date = start + Duration::days(j);
            if let Some(days) = days.filter(|d| !d.is_empty()) {
                if !days.contains(&(date.weekday().num_days_from_monday() as u8)) {
                    continue;
                }
            }
            if let Some(at) = local(date.and_time(time)) {
                if at > after {
                    return Some(at);
                }
            }
        }
    }
    None
}

fn last_day(year: i32, month: u32) -> Option<u32> {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31].get(month as usize - 1).copied()
}
