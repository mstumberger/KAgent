//! 5-field cron expressions for task schedules (automations.md section 2): minute, hour,
//! day of month, month and day of week (0 and 7 are Sunday), each `*`, a number, a list,
//! a range or a step. A day matches when its month matches and its day of month or its
//! day of week does; a day field that is `*` matches every day.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};

use super::schedule::local;

/// How far ahead the next matching minute is looked for.
const DAYS_AHEAD: i64 = 366;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    /// Minutes past midnight of the matching clock times, sorted.
    moments: Vec<u32>,
    days: Vec<u32>,
    months: Vec<u32>,
    /// 0 is Sunday, as in the expression.
    weekdays: Vec<u32>,
    any_day: bool,
    any_weekday: bool,
}

struct Field {
    values: Vec<u32>,
    any: bool,
}

pub fn parse(expr: &str) -> Result<Cron, String> {
    let specs: Vec<&str> = expr.split_whitespace().collect();
    if specs.len() != 5 {
        return Err(format!("expected 5 fields (minute, hour, day of month, month, day of week), found {}", specs.len()));
    }
    let minutes = field(specs[0], 0, 59, "minute")?;
    let hours = field(specs[1], 0, 23, "hour")?;
    let days = field(specs[2], 1, 31, "day of month")?;
    let months = field(specs[3], 1, 12, "month")?;
    let weekdays = field(specs[4], 0, 7, "day of week")?;
    let mut moments: Vec<u32> = hours.values.iter().flat_map(|h| minutes.values.iter().map(move |m| h * 60 + m)).collect();
    moments.sort_unstable();
    let mut weekday_values: Vec<u32> = weekdays.values.iter().map(|d| d % 7).collect();
    weekday_values.sort_unstable();
    weekday_values.dedup();
    Ok(Cron { moments, days: days.values, months: months.values, weekdays: weekday_values, any_day: days.any, any_weekday: weekdays.any })
}

/// One field: comma-separated parts, each a number, a range or `*`, with an optional step.
fn field(spec: &str, min: u32, max: u32, name: &str) -> Result<Field, String> {
    if spec.is_empty() {
        return Err(format!("{name}: an empty field"));
    }
    let mut values = Vec::new();
    for part in spec.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().ok().filter(|s| *s > 0).ok_or_else(|| format!("{name}: a bad step in '{part}'"))?),
            None => (part, 1),
        };
        let (from, to) = match range {
            "*" => (min, max),
            range if range.contains('-') => {
                let (a, b) = range.split_once('-').ok_or_else(|| format!("{name}: a bad range in '{part}'"))?;
                (number(a, min, max, name)?, number(b, min, max, name)?)
            }
            single => {
                let value = number(single, min, max, name)?;
                values.extend((value..=value).step_by(step as usize));
                continue;
            }
        };
        if from <= to {
            values.extend((from..=to).step_by(step as usize));
        } else {
            // Over the end of the range: 22-2 is 22, 23, 0, 1, 2.
            values.extend((from..=max).step_by(step as usize));
            values.extend((min..=to).step_by(step as usize));
        }
    }
    values.sort_unstable();
    values.dedup();
    if values.is_empty() {
        return Err(format!("{name}: nothing matches '{spec}'"));
    }
    Ok(Field { values, any: spec == "*" })
}

fn number(text: &str, min: u32, max: u32, name: &str) -> Result<u32, String> {
    let value: u32 = text.trim().parse().map_err(|_| format!("{name}: '{text}' is not a number"))?;
    (min..=max).contains(&value).then_some(value).ok_or_else(|| format!("{name}: {value} is not between {min} and {max}"))
}

impl Cron {
    /// The first matching minute strictly after `after`, looked for 366 days ahead.
    pub fn next(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        let first = after.date_naive();
        for offset in 0..=DAYS_AHEAD {
            let date = first + Duration::days(offset);
            if !self.months.contains(&date.month()) || !self.is_day(date) {
                continue;
            }
            let midnight = date.and_hms_opt(0, 0, 0)?;
            for &moment in &self.moments {
                if let Some(at) = local(midnight + Duration::minutes(i64::from(moment))) {
                    if at > after {
                        return Some(at);
                    }
                }
            }
        }
        None
    }

    fn is_day(&self, date: NaiveDate) -> bool {
        let day = self.days.contains(&date.day());
        let weekday = self.weekdays.contains(&date.weekday().num_days_from_sunday());
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => weekday,
            (false, true) => day,
            _ => day || weekday,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(year, month, day, hour, minute, 0).unwrap()
    }

    #[test]
    fn five_fields_with_lists_ranges_and_steps() {
        let cron = parse("*/20 9-17 * * 1-5").unwrap();
        assert_eq!(cron.moments.first(), Some(&540));
        assert_eq!(cron.moments.len(), 9 * 3);
        assert!(cron.any_day && !cron.any_weekday, "the day of month is `*`, the day of week is not");
        assert_eq!(cron.weekdays, [1, 2, 3, 4, 5]);
        // Lists, a wrap-around range, and Sunday written 7 or 0.
        assert_eq!(parse("0,30 22-2 1,15 1-6 0,7").unwrap().moments.len(), 2 * 5);
        assert_eq!(parse("0 12 * * 7").unwrap().weekdays, [0]);
        assert_eq!(parse("0 12 * * 1-7/2").unwrap().weekdays, [0, 1, 3, 5], "sorted, with Sunday written 7");
        for bad in ["* * * *", "* * * * * *", "60 * * * *", "* 24 * * *", "* * 0 * *", "* * 32 * *", "* * * 13 *", "* * * * 8", "a * * * *", "*/0 * * * *", "1-60 * * * *", "", "5-2/0 * * * *"] {
            assert!(parse(bad).is_err(), "{bad:?} must not parse");
        }
    }

    #[test]
    fn next_run_from_a_fixed_clock() {
        // Monday 23 November 2026, 10:05 local.
        let cron = parse("*/20 9-17 * * 1-5").unwrap();
        let now = at(2026, 11, 23, 10, 5);
        assert_eq!(cron.next(now), Some(at(2026, 11, 23, 10, 20)));
        // The last slot of a weekday is 17:59: the next one is the next working day.
        assert_eq!(cron.next(at(2026, 11, 20, 17, 40)), Some(at(2026, 11, 23, 9, 0)), "Friday evening to Monday morning");
        assert_eq!(cron.next(at(2026, 11, 21, 12, 0)), Some(at(2026, 11, 23, 9, 0)), "Saturday to Monday morning");
        assert_eq!(parse("30 8 * * *").unwrap().next(at(2026, 11, 20, 8, 30)), Some(at(2026, 11, 21, 8, 30)));
        assert_eq!(parse("0 9 1 * *").unwrap().next(at(2026, 11, 2, 0, 0)), Some(at(2026, 12, 1, 9, 0)));
        // Day of month and day of week both set: a day matches either one (the 1st, and every Monday).
        let either = parse("0 12 1 * 1").unwrap();
        assert_eq!(either.next(at(2026, 11, 2, 13, 0)), Some(at(2026, 11, 9, 12, 0)), "after the 1st (a Sunday) and a Monday: the next Monday");
        assert_eq!(either.next(at(2026, 11, 30, 13, 0)), Some(at(2026, 12, 1, 12, 0)), "after the Monday noon: the 1st of December (a Tuesday) comes before the next Monday");
        // The same minute is not returned twice, and a time that never comes gives None.
        assert_eq!(cron.next(at(2026, 11, 23, 10, 20)), Some(at(2026, 11, 23, 10, 40)));
        assert_eq!(parse("0 0 30 2 *").unwrap().next(now), None, "there is no 30th of February");
    }
}
