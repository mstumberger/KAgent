//! The morning report, pure part (calendar.md section 6): when it is due, the language, and
//! the text, built by code in Slovenian or English. No model writes it, so it costs nothing
//! and says only what the calendar says.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use super::settings::{parse_hhmm, ReportSection};

/// A report that is later than this many minutes after its time is a late start.
const ON_TIME_MIN: i64 = 5;
/// A late start still delivers before this hour.
const LATE_UNTIL_HOUR: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Sl,
    En,
}

impl Lang {
    pub fn of(setting: &str, last_used: &str) -> Self {
        match (setting, last_used) {
            ("sl", _) => Self::Sl,
            ("en", _) => Self::En,
            (_, "sl") => Self::Sl,
            _ => Self::En,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sl => "sl",
            Self::En => "en",
        }
    }
}

/// The language of a text the owner wrote: Slovenian when it has its letters or its common
/// words, else English; `None` when there is nothing to judge by.
pub fn language_of(text: &str) -> Option<Lang> {
    let lower = text.to_lowercase();
    if lower.chars().filter(|c| c.is_alphabetic()).count() < 3 {
        return None;
    }
    let padded = format!(" {lower} ");
    let words = [" je ", " za ", " na ", " ne ", " ali ", " prosim", " kaj ", " kdaj", " dodaj", " jutri", " danes", " sem ", " imam", " hvala", " dobro", " koledar"];
    if lower.contains(['č', 'š', 'ž']) || words.iter().any(|w| padded.contains(w)) {
        Some(Lang::Sl)
    } else if lower.split_whitespace().count() >= 3 {
        Some(Lang::En)
    } else {
        // A word or two with no Slovenian in it says nothing: "yes", "ok", a name.
        None
    }
}

/// What a tick of the timer does about today's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    No,
    /// Deliver it now.
    Yes,
    /// Its time is long past (a daemon that started after noon): today has none.
    Skip,
}

/// Mon-Fri at `weekdays`, Sat and Sun at `weekends`, once per day; a daemon started after
/// the time delivers once if it is before 12:00.
pub fn due(cfg: &ReportSection, now: NaiveDateTime, done_day: &str) -> Due {
    if !cfg.enabled || done_day == now.date().to_string() {
        return Due::No;
    }
    let at = if now.weekday().number_from_monday() >= 6 { &cfg.weekends } else { &cfg.weekdays };
    let Some(minute) = parse_hhmm(at) else { return Due::No };
    let since = i64::from(now.hour() * 60 + now.minute()) - i64::from(minute);
    match since {
        s if s < 0 => Due::No,
        s if s <= ON_TIME_MIN || now.hour() < LATE_UNTIL_HOUR => Due::Yes,
        _ => Due::Skip,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Day {
    Today,
    Tomorrow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// `None`: all day.
    pub at: Option<String>,
    pub title: String,
    pub place: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    pub a: String,
    pub b: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leave {
    pub title: String,
    pub at: String,
    pub drive_min: i64,
    pub margin_min: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Next {
    pub title: String,
    pub at: Option<String>,
    pub only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLine {
    pub at: Option<String>,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct Data {
    pub lang: Lang,
    pub address: String,
    pub date: NaiveDate,
    pub day: Day,
    pub items: Vec<Item>,
    pub clashes: Vec<Clash>,
    pub leave: Option<Leave>,
    pub tomorrow: Option<Next>,
    pub tasks: Vec<TaskLine>,
}

const SL_DAYS: [&str; 7] = ["ponedeljek", "torek", "sreda", "četrtek", "petek", "sobota", "nedelja"];
const SL_MONTHS: [&str; 12] = ["januar", "februar", "marec", "april", "maj", "junij", "julij", "avgust", "september", "oktober", "november", "december"];
const EN_DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const EN_MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn date_text(lang: Lang, d: NaiveDate) -> String {
    let w = d.weekday().num_days_from_monday() as usize;
    match lang {
        Lang::Sl => format!("{}, {}. {}", SL_DAYS[w], d.day(), SL_MONTHS[d.month0() as usize]),
        Lang::En => format!("{}, {} {}", EN_DAYS[w], d.day(), EN_MONTHS[d.month0() as usize]),
    }
}

/// The report for the model, the page, Telegram and the voice: short lines.
pub fn text(d: &Data) -> String {
    let (en, sl) = (d.lang == Lang::En, d.lang == Lang::Sl);
    let when = date_text(d.lang, d.date);
    let mut out = String::new();
    let name = d.address.trim();
    if d.day == Day::Today {
        let hello = if sl { "Dobro jutro" } else { "Good morning" };
        out.push_str(&if name.is_empty() { format!("{hello}. ") } else { format!("{hello}, {name}. ") });
        out.push_str(&if sl { format!("Danes, {when}:\n") } else { format!("Today, {when}:\n") });
    } else {
        out.push_str(&if sl { format!("Jutri, {when}:\n") } else { format!("Tomorrow, {when}:\n") });
    }
    if d.items.is_empty() {
        out.push_str(match (d.lang, d.day) {
            (Lang::Sl, Day::Today) => "Danes nimaš ničesar v koledarju.\n",
            (Lang::Sl, Day::Tomorrow) => "Jutri nimaš ničesar v koledarju.\n",
            (Lang::En, Day::Today) => "Nothing in your calendar today.\n",
            (Lang::En, Day::Tomorrow) => "Nothing in your calendar tomorrow.\n",
        });
    }
    for i in &d.items {
        let at = i.at.clone().unwrap_or_else(|| if sl { "ves dan".into() } else { "all day".into() });
        let place = match (i.place.is_empty(), en) {
            (true, _) => String::new(),
            (false, true) => format!(", at {}", i.place),
            (false, false) => format!(", {}", i.place),
        };
        out.push_str(&format!("- {at} {}{place}\n", i.title));
    }
    for c in &d.clashes {
        out.push_str(&if sl { format!("{} in {} se prekrivata od {} do {}.\n", c.a, c.b, c.from, c.to) } else { format!("{} and {} overlap from {} to {}.\n", c.a, c.b, c.from, c.to) });
    }
    if let Some(l) = &d.leave {
        out.push_str(&if sl {
            format!("Odidi do {} za {}: {} minut z avtom s prometom, plus {}.\n", l.at, l.title, l.drive_min, l.margin_min)
        } else {
            format!("Leave by {} for {}: {} minutes by car with traffic, plus {}.\n", l.at, l.title, l.drive_min, l.margin_min)
        });
    }
    if let Some(n) = &d.tomorrow {
        let line = match (&n.at, n.only, d.lang) {
            (Some(at), false, Lang::Sl) => format!("Jutri se začne zgodaj: {} ob {at}.", n.title),
            (Some(at), true, Lang::Sl) => format!("Jutri: samo {} ob {at}.", n.title),
            (None, _, Lang::Sl) => format!("Jutri: ves dan {}.", n.title),
            (Some(at), false, Lang::En) => format!("Tomorrow starts early: {} at {at}.", n.title),
            (Some(at), true, Lang::En) => format!("Tomorrow: only {} at {at}.", n.title),
            (None, _, Lang::En) => format!("Tomorrow: all day {}.", n.title),
        };
        out.push_str(&line);
        out.push('\n');
    }
    if !d.tasks.is_empty() {
        let list: Vec<String> = d.tasks.iter().map(|t| t.at.as_ref().map(|a| format!("{a} {}", t.title)).unwrap_or_else(|| t.title.clone())).collect();
        out.push_str(&format!("{} {}\n", if sl { "Opomniki danes:" } else { "Reminders today:" }, list.join("; ")));
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(lang: Lang) -> Data {
        Data {
            lang,
            address: "Marko".into(),
            date: NaiveDate::from_ymd_opt(2026, 11, 27).unwrap(),
            day: Day::Today,
            items: vec![
                Item { at: None, title: "Day off".into(), place: String::new() },
                Item { at: Some("10:00".into()), title: "Dentist".into(), place: "Clinic, Glavni trg 5".into() },
                Item { at: Some("10:30".into()), title: "Sync".into(), place: String::new() },
            ],
            clashes: vec![Clash { a: "Dentist".into(), b: "Sync".into(), from: "10:30".into(), to: "11:00".into() }],
            leave: Some(Leave { title: "Dentist".into(), at: "09:20".into(), drive_min: 25, margin_min: 10 }),
            tomorrow: Some(Next { title: "Gym".into(), at: Some("07:00".into()), only: false }),
            tasks: vec![TaskLine { at: Some("14:00".into()), title: "Call the plumber".into() }, TaskLine { at: None, title: "Water the plants".into() }],
        }
    }

    #[test]
    fn a_full_day_in_english() {
        assert_eq!(
            text(&data(Lang::En)),
            "Good morning, Marko. Today, Friday, 27 November:\n- all day Day off\n- 10:00 Dentist, at Clinic, Glavni trg 5\n- 10:30 Sync\nDentist and Sync overlap from 10:30 to 11:00.\nLeave by 09:20 for Dentist: 25 minutes by car with traffic, plus 10.\nTomorrow starts early: Gym at 07:00.\nReminders today: 14:00 Call the plumber; Water the plants"
        );
    }

    #[test]
    fn a_full_day_in_slovenian() {
        assert_eq!(
            text(&data(Lang::Sl)),
            "Dobro jutro, Marko. Danes, petek, 27. november:\n- ves dan Day off\n- 10:00 Dentist, Clinic, Glavni trg 5\n- 10:30 Sync\nDentist in Sync se prekrivata od 10:30 do 11:00.\nOdidi do 09:20 za Dentist: 25 minut z avtom s prometom, plus 10.\nJutri se začne zgodaj: Gym ob 07:00.\nOpomniki danes: 14:00 Call the plumber; Water the plants"
        );
    }

    #[test]
    fn an_empty_day_and_the_other_shapes() {
        let mut d = data(Lang::En);
        d.items.clear();
        d.clashes.clear();
        d.leave = None;
        d.tomorrow = Some(Next { title: "Review".into(), at: Some("09:00".into()), only: true });
        d.tasks.clear();
        d.address.clear();
        assert_eq!(text(&d), "Good morning. Today, Friday, 27 November:\nNothing in your calendar today.\nTomorrow: only Review at 09:00.");
        d.lang = Lang::Sl;
        d.tomorrow = Some(Next { title: "Dopust".into(), at: None, only: true });
        assert_eq!(text(&d), "Dobro jutro. Danes, petek, 27. november:\nDanes nimaš ničesar v koledarju.\nJutri: ves dan Dopust.");
        d.day = Day::Tomorrow;
        d.tomorrow = None;
        assert_eq!(text(&d), "Jutri, petek, 27. november:\nJutri nimaš ničesar v koledarju.");
        d.lang = Lang::En;
        assert_eq!(text(&d), "Tomorrow, Friday, 27 November:\nNothing in your calendar tomorrow.");
    }

    fn at(day: u32, h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap().and_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn the_days_and_times_the_report_is_due() {
        let cfg = ReportSection::default();
        // Friday 27 Nov: 07:45 on weekdays.
        assert_eq!(due(&cfg, at(27, 7, 44), ""), Due::No, "not yet");
        assert_eq!(due(&cfg, at(27, 7, 45), ""), Due::Yes, "at the time");
        assert_eq!(due(&cfg, at(27, 7, 50), ""), Due::Yes, "while the timer is still on time");
        assert_eq!(due(&cfg, at(27, 7, 45), "2026-11-27"), Due::No, "once per day");
        assert_eq!(due(&cfg, at(27, 7, 45), "2026-11-26"), Due::Yes, "yesterday's does not count");
        // Saturday 28 Nov: 09:00 on weekends.
        assert_eq!(due(&cfg, at(28, 7, 45), ""), Due::No, "the weekday time is not the weekend's");
        assert_eq!(due(&cfg, at(28, 9, 0), ""), Due::Yes);
        assert_eq!(due(&cfg, at(29, 9, 0), ""), Due::Yes, "Sunday too");
        assert_eq!(due(&cfg, at(30, 7, 45), ""), Due::Yes, "Monday is a weekday");
    }

    #[test]
    fn a_daemon_that_starts_late_delivers_before_noon_and_skips_after() {
        let cfg = ReportSection::default();
        assert_eq!(due(&cfg, at(27, 8, 30), ""), Due::Yes, "started after the time, before 12:00: once");
        assert_eq!(due(&cfg, at(27, 11, 59), ""), Due::Yes);
        assert_eq!(due(&cfg, at(27, 12, 0), ""), Due::Skip, "from noon today has none");
        assert_eq!(due(&cfg, at(27, 8, 30), "2026-11-27"), Due::No, "and not again");
        // A report the owner set for the afternoon is on time at its time.
        let late = ReportSection { weekdays: "14:00".into(), ..cfg.clone() };
        assert_eq!(due(&late, at(27, 14, 2), ""), Due::Yes);
        assert_eq!(due(&late, at(27, 15, 0), ""), Due::Skip);
    }

    #[test]
    fn off_or_no_time_means_no_report() {
        let off = ReportSection { enabled: false, ..ReportSection::default() };
        assert_eq!(due(&off, at(27, 8, 0), ""), Due::No);
        let none = ReportSection { weekdays: String::new(), ..ReportSection::default() };
        assert_eq!(due(&none, at(27, 8, 0), ""), Due::No, "no report on weekdays");
        assert_eq!(due(&none, at(28, 9, 0), ""), Due::Yes, "weekends still");
    }

    #[test]
    fn the_language_follows_the_setting_or_the_last_the_owner_used() {
        assert_eq!(Lang::of("sl", "en"), Lang::Sl);
        assert_eq!(Lang::of("en", "sl"), Lang::En);
        assert_eq!(Lang::of("auto", "sl"), Lang::Sl);
        assert_eq!(Lang::of("auto", ""), Lang::En);
        assert_eq!(language_of("Kaj imam danes na koledarju?"), Some(Lang::Sl));
        assert_eq!(language_of("dodaj dogodek v petek"), Some(Lang::Sl));
        assert_eq!(language_of("What is on my calendar today?"), Some(Lang::En));
        assert_eq!(language_of("ok"), None);
        assert_eq!(language_of("Pošlji"), Some(Lang::Sl));
    }
}
