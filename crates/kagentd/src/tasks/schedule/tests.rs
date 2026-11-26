use super::*;
use serde_json::json;

/// Monday 2026-11-23 10:00 local.
fn now() -> DateTime<Local> {
    Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
}

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
    Local.with_ymd_and_hms(year, month, day, hour, minute, 0).unwrap()
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn daily(at: &str, days: Option<Vec<u8>>) -> Schedule {
    Schedule::interval(Unit::Day, 1, Some(at.into()), days, None)
}

#[test]
fn once_in_words() {
    let n = now();
    assert_eq!(parse("in 20m", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 10, 20) }));
    assert_eq!(parse("in 2 hours", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 12, 0) }));
    assert_eq!(parse("1m", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 10, 1) }), "a bare duration means in");
    assert_eq!(parse("30 min", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 10, 30) }));
    assert_eq!(parse("in 1d", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 24, 10, 0) }));
    assert_eq!(parse("at 14:30", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 14, 30) }));
    assert_eq!(parse("14:30", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 14, 30) }));
    assert_eq!(parse("09:00", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 24, 9, 0) }), "a time that is over is tomorrow");
    assert_eq!(parse("7pm", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 19, 0) }));
    assert_eq!(parse("12am", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 24, 0, 0) }), "twelve at night, not at noon");
    assert_eq!(parse("tomorrow 7am", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 24, 7, 0) }));
    assert_eq!(parse("tomorrow", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 24, 9, 0) }));
    assert_eq!(parse("monday 09:00", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 30, 9, 0) }), "said on a Monday after nine: next Monday");
    assert_eq!(parse("monday 12:00", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 12, 0) }), "said on a Monday before noon: today");
    assert_eq!(parse("Fri 18:00", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 27, 18, 0) }));
    assert_eq!(parse("2026-11-25 09:15", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 25, 9, 15) }));
    assert_eq!(parse("2026-11-25", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 25, 9, 0) }));
    assert_eq!(parse("2026-01-01 09:00", n).unwrap_err(), "2026-01-01 09:00 is in the past");
    assert_eq!(parse("", n).unwrap(), None, "no schedule = a to-do");
    assert_eq!(parse(" none ", n).unwrap(), None);
    assert_eq!(parse("Manual", n).unwrap(), None);
    // What a once comes to on the wire.
    assert_eq!(serde_json::to_value(Schedule::Once { at: n }).unwrap(), json!({"type": "once", "at": n.to_rfc3339()}));
}

#[test]
fn intervals_in_words() {
    let n = now();
    assert_eq!(parse("every 30m", n).unwrap(), Some(Schedule::interval(Unit::Minute, 30, None, None, None)));
    assert_eq!(parse("every 90 minutes", n).unwrap(), Some(Schedule::interval(Unit::Minute, 90, None, None, None)));
    assert_eq!(parse("every 2h", n).unwrap(), Some(Schedule::interval(Unit::Hour, 2, None, None, None)));
    assert_eq!(parse("every 2 hours", n).unwrap(), Some(Schedule::interval(Unit::Hour, 2, None, None, None)));
    assert_eq!(parse("hourly", n).unwrap(), Some(Schedule::interval(Unit::Hour, 1, None, None, None)));
    assert_eq!(parse("every day at 9:00", n).unwrap(), Some(daily("09:00", None)));
    assert_eq!(parse("every day", n).unwrap(), Some(Schedule::interval(Unit::Day, 1, None, None, None)), "no time named: the default (08:00) applies when it runs");
    assert_eq!(parse("every day", n).unwrap().unwrap().describe(), "daily at 08:00");
    assert_eq!(parse("daily 09:00", n).unwrap(), Some(daily("09:00", None)));
    assert_eq!(parse("weekdays 08:30", n).unwrap(), Some(daily("08:30", Some(vec![0, 1, 2, 3, 4]))));
    assert_eq!(parse("weekends 10:00", n).unwrap(), Some(daily("10:00", Some(vec![5, 6]))));
    assert_eq!(parse("every weekday 08:30", n).unwrap(), Some(daily("08:30", Some(vec![0, 1, 2, 3, 4]))));
    assert_eq!(parse("mon,fri 18:00", n).unwrap(), Some(daily("18:00", Some(vec![0, 4]))));
    assert_eq!(parse("every monday 9", n).unwrap(), Some(daily("09:00", Some(vec![0]))));
    assert_eq!(parse("every 2 days at 09:00", n).unwrap(), Some(Schedule::interval(Unit::Day, 2, Some("09:00".into()), None, Some(date(2026, 11, 23)))));
    assert_eq!(parse("every 3 months", n).unwrap(), Some(Schedule::interval(Unit::Month, 3, None, None, Some(date(2026, 11, 23)))), "from today, the day of its first run");
}

#[test]
fn scenario_table_with_cron_and_what_does_not_parse() {
    let n = now();
    // Scenario S28 item 1.
    assert_eq!(parse("every 2 weeks on monday 09:00", n).unwrap(), Some(Schedule::interval(Unit::Week, 2, Some("09:00".into()), Some(vec![0]), Some(date(2026, 11, 30)))));
    assert_eq!(parse("mon,fri 18:00", n).unwrap(), Some(daily("18:00", Some(vec![0, 4]))));
    assert_eq!(parse("*/20 9-17 * * 1-5", n).unwrap(), Some(Schedule::Cron { expr: "*/20 9-17 * * 1-5".into() }));
    assert_eq!(parse("in 20m", n).unwrap(), Some(Schedule::Once { at: at(2026, 11, 23, 10, 20) }));
    assert_eq!(parse("sometimes", n).unwrap_err(), format!("unknown schedule 'sometimes'. Use e.g. {EXAMPLES}"));
    // Cron: five fields with a star or a step, or the word "cron" before five fields.
    assert_eq!(parse("0 9 * * 1-5", n).unwrap(), Some(Schedule::Cron { expr: "0 9 * * 1-5".into() }));
    assert_eq!(parse("cron 30 8 1 * *", n).unwrap(), Some(Schedule::Cron { expr: "30 8 1 * *".into() }));
    assert_eq!(parse("cron   30  8  1  *  *", n).unwrap(), Some(Schedule::Cron { expr: "30 8 1 * *".into() }), "one space between the fields");
    // Text that does not parse says why, with three examples.
    for text in ["sometimes", "every", "every 2 fortnights", "in", "in 0m", "monday", "dailyx", "every monday at home", "cron * * *", "cron 60 8 * * *", "0 9 * * * extra", "30 9 1 1 monday"] {
        let error = parse(text, n).unwrap_err();
        assert!(error.contains(EXAMPLES), "{text}: {error}");
    }
}

#[test]
fn described_in_a_few_words() {
    let n = now();
    let text = |words: &str| parse(words, n).unwrap().unwrap().describe();
    assert_eq!(text("every 30m"), "every 30 minutes");
    assert_eq!(text("every 1 minutes"), "every minute");
    assert_eq!(text("hourly"), "hourly");
    assert_eq!(text("every 2h"), "every 2 hours");
    assert_eq!(text("daily 09:00"), "daily at 09:00");
    assert_eq!(text("every day at 9:00"), "daily at 09:00");
    assert_eq!(text("weekdays 08:30"), "weekdays at 08:30");
    assert_eq!(text("weekends 10:00"), "weekends at 10:00");
    assert_eq!(text("mon,fri 18:00"), "mon, fri at 18:00");
    assert_eq!(text("every monday 9"), "mon at 09:00");
    assert_eq!(text("every 2 days at 09:00"), "every 2 days at 09:00");
    assert_eq!(text("every 2 weeks on monday 09:00"), "every 2 weeks on mon at 09:00");
    assert_eq!(text("every 3 months"), "every 3 months on day 23 at 08:00");
    assert_eq!(text("0 9 * * 1-5"), "cron 0 9 * * 1-5");
    assert_eq!(text("in 20m"), "once Mon 23 Nov 10:20");
}

#[test]
fn next_run_of_each_type_from_a_fixed_clock() {
    let n = now();
    // Days: the next allowed weekday, at that time.
    let weekdays = parse("weekdays 08:30", n).unwrap().unwrap();
    assert_eq!(weekdays.next_run(n), Some(at(2026, 11, 24, 8, 30)), "Tuesday morning");
    assert_eq!(weekdays.next_run(at(2026, 11, 27, 9, 0)), Some(at(2026, 11, 30, 8, 30)), "over the weekend");
    assert_eq!(weekdays.next_run(at(2026, 11, 23, 8, 30)), Some(at(2026, 11, 24, 8, 30)), "strictly after");
    assert_eq!(parse("every day at 9:00", n).unwrap().unwrap().next_run(n), Some(at(2026, 11, 24, 9, 0)));
    assert_eq!(parse("mon,fri 18:00", n).unwrap().unwrap().next_run(n), Some(at(2026, 11, 23, 18, 0)), "Monday evening is still ahead");
    // Every second day counts from its first.
    let second = parse("every 2 days at 09:00", n).unwrap().unwrap();
    assert_eq!(second.next_run(n), Some(at(2026, 11, 25, 9, 0)));
    assert_eq!(second.next_run(at(2026, 11, 23, 8, 0)), Some(at(2026, 11, 23, 9, 0)));
    // Every second week on Monday counts from the first such Monday.
    let fortnight = parse("every 2 weeks on monday 09:00", n).unwrap().unwrap();
    assert_eq!(fortnight.next_run(n), Some(at(2026, 11, 30, 9, 0)));
    assert_eq!(fortnight.next_run(at(2026, 11, 30, 9, 0)), Some(at(2026, 12, 14, 9, 0)));
    assert_eq!(fortnight.next_run(at(2026, 12, 7, 12, 0)), Some(at(2026, 12, 14, 9, 0)), "the Monday in between is the other week");
    // Minutes and hours count from `after`.
    assert_eq!(parse("every 30m", n).unwrap().unwrap().next_run(n), Some(n + Duration::minutes(30)));
    assert_eq!(parse("every 2 hours", n).unwrap().unwrap().next_run(n), Some(n + Duration::hours(2)));
    // A once that is over has no next run.
    assert_eq!(Schedule::Once { at: n }.next_run(n), None);
}

#[test]
fn month_lengths_and_a_day_the_clocks_skipped() {
    // The 31st: the last day of a shorter month.
    let monthly = Schedule::interval(Unit::Month, 1, Some("09:00".into()), None, Some(date(2026, 1, 31)));
    assert_eq!(monthly.next_run(at(2026, 1, 31, 10, 0)), Some(at(2026, 2, 28, 9, 0)));
    assert_eq!(monthly.next_run(at(2026, 2, 28, 10, 0)), Some(at(2026, 3, 31, 9, 0)));
    assert_eq!(monthly.next_run(at(2026, 11, 30, 10, 0)), Some(at(2026, 12, 31, 9, 0)));
    let leap = Schedule::interval(Unit::Month, 1, Some("09:00".into()), None, Some(date(2027, 1, 30)));
    assert_eq!(leap.next_run(at(2028, 1, 30, 10, 0)), Some(at(2028, 2, 29, 9, 0)), "a leap year");
    assert_eq!(leap.next_run(at(2027, 1, 30, 10, 0)), Some(at(2027, 2, 28, 9, 0)));
    // Every third month keeps the day of its first run.
    let quarter = Schedule::interval(Unit::Month, 3, Some("09:00".into()), None, Some(date(2026, 11, 23)));
    assert_eq!(quarter.next_run(at(2026, 11, 23, 10, 0)), Some(at(2027, 2, 23, 9, 0)));
    assert_eq!(quarter.next_run(at(2027, 2, 23, 10, 0)), Some(at(2027, 5, 23, 9, 0)));
    // An interval with no anchor counts from the epoch: every 200 weeks still has a next run.
    assert!(Schedule::interval(Unit::Week, 200, None, None, None).next_run(at(2026, 11, 23, 10, 0)).is_some());

    // Spring forward in Europe/Ljubljana: 29 March 2026, 02:00 becomes 03:00. In a zone
    // without a gap there the time is kept as it is.
    let morning = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(2, 30, 0).unwrap();
    let gap = matches!(Local.from_local_datetime(&morning), LocalResult::None);
    let daily = parse("daily 02:30", at(2026, 3, 28, 12, 0)).unwrap().unwrap();
    let expected = if gap { at(2026, 3, 29, 3, 30) } else { at(2026, 3, 29, 2, 30) };
    assert_eq!(daily.next_run(at(2026, 3, 28, 12, 0)), Some(expected), "a time the clocks skipped runs an hour later");
    assert_eq!(daily.next_run(expected), Some(at(2026, 3, 30, 2, 30)), "and the next day is a normal one");
}

#[test]
fn stored_shapes_the_old_names_and_an_unknown_field() {
    let noon = DateTime::parse_from_rfc3339("2026-12-01T12:00:00+01:00").unwrap().with_timezone(&Local);
    let once = json!({"type": "once", "at": "2026-12-01T12:00:00+01:00"});
    assert_eq!(Schedule::from_value(once.clone()).unwrap(), Schedule::Once { at: noon });
    let interval = json!({"type": "interval", "unit": "day", "n": 1, "at": "09:00", "weekdays": [0, 4]});
    assert_eq!(Schedule::from_value(interval.clone()).unwrap(), daily("09:00", Some(vec![0, 4])));
    assert_eq!(serde_json::to_value(&daily("09:00", Some(vec![0, 4]))).unwrap(), interval);
    // A field a schedule does not have is left out of the file too.
    assert_eq!(serde_json::to_value(&daily("09:00", None)).unwrap(), json!({"type": "interval", "unit": "day", "n": 1, "at": "09:00"}));
    assert_eq!(serde_json::to_value(&Schedule::interval(Unit::Minute, 20, None, None, None)).unwrap(), json!({"type": "interval", "unit": "minute", "n": 20}));
    // Before S28.
    assert_eq!(Schedule::from_value(json!({"type": "At", "at": "2026-12-01T12:00:00+01:00"})).unwrap(), Schedule::Once { at: noon });
    assert_eq!(Schedule::from_value(json!({"type": "Every", "minutes": 20})).unwrap(), Schedule::interval(Unit::Minute, 20, None, None, None));
    assert_eq!(Schedule::from_value(json!({"type": "Daily", "time": "09:00", "days": [0, 4]})).unwrap(), daily("09:00", Some(vec![0, 4])));
    assert_eq!(Schedule::from_value(json!({"type": "Daily", "time": "09:00", "days": []})).unwrap(), daily("09:00", None), "no day named is every day");
    assert_eq!(Schedule::from_value(json!({"type": "cron", "expr": "0 9 * * 1-5"})).unwrap(), parse("0 9 * * 1-5", now()).unwrap().unwrap());
    // An unknown field, in or around a variant, is rejected; so is a value out of range.
    for bad in [
        json!({"type": "once", "at": "2026-12-01T12:00:00+01:00", "repeat": true}),
        json!({"type": "interval", "unit": "minute", "n": 5, "at": "09:00"}),
        json!({"type": "interval", "unit": "minute", "n": 0}),
        json!({"type": "interval", "unit": "day", "n": 201}),
        json!({"type": "interval", "unit": "day", "n": 1, "at": "9 in the morning"}),
        json!({"type": "interval", "unit": "day", "n": 1, "weekdays": [7]}),
        json!({"type": "interval", "unit": "month", "n": 1, "weekdays": [1]}),
        json!({"type": "cron", "expr": "0 9 * *"}),
        json!({"type": "weekly"}),
        json!({"unit": "day", "n": 1}),
        json!("daily"),
    ] {
        assert!(Schedule::from_value(bad.clone()).is_err(), "{bad} must be rejected");
    }
    let unknown = json!({"type": "once", "at": "2026-12-01T12:00:00+01:00", "repeat": true});
    assert!(Schedule::from_value(unknown.clone()).unwrap_err().contains("unknown field `repeat`, expected `at`"));
    assert!(serde_json::from_value::<Schedule>(unknown).is_err(), "a record or a request with it does not read");
    assert_eq!(Schedule::from_value(json!({"type": "weekly"})).unwrap_err(), "unknown schedule type 'weekly'");
}
