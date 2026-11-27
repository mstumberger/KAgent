//! settings.calendar (calendar.md section 2, config.md section 11). The section type lives
//! in the module that reads it, as Telegram's does.

use serde::{Deserialize, Serialize};

use crate::sections::SectionError;
use crate::settings::Settings;

/// settings.calendar: off until the owner connects an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CalendarSection {
    pub enabled: bool,
    /// The OAuth client id of the owner's own Google Cloud project (not a secret by itself,
    /// but protected: it decides whose account KAgent talks to).
    pub client_id: String,
    /// The calendars the agent and the report may see; every other one does not exist for them.
    pub calendars: Vec<String>,
    /// Where `add` puts an event: one of `calendars`, and writable.
    pub default_calendar: String,
    /// 5..=1440.
    pub default_duration_min: u32,
    /// A free address; leave-by needs it and a Maps key.
    pub home: String,
    /// 0..=120: added to the drive when leaving.
    pub travel_margin_min: u32,
    pub report: ReportSection,
}

impl Default for CalendarSection {
    fn default() -> Self {
        Self {
            enabled: false,
            client_id: String::new(),
            calendars: vec!["primary".into()],
            default_calendar: "primary".into(),
            default_duration_min: 60,
            home: String::new(),
            travel_margin_min: 10,
            report: ReportSection::default(),
        }
    }
}

/// settings.calendar.report: the morning report (calendar.md section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportSection {
    pub enabled: bool,
    /// `HH:MM` on Monday to Friday, or empty: no report on those days.
    pub weekdays: String,
    /// `HH:MM` on Saturday and Sunday, or empty.
    pub weekends: String,
    /// Waits for the first live voice session after the time.
    pub spoken: bool,
    pub telegram: bool,
    /// The first event of tomorrow, when it is early.
    pub tomorrow: bool,
    /// KAgent's own reminders and tasks due today.
    pub tasks: bool,
    /// "Leave by 08:05": needs a Maps key and `home`.
    pub leave_by: bool,
}

impl Default for ReportSection {
    fn default() -> Self {
        Self { enabled: true, weekdays: "07:45".into(), weekends: "09:00".into(), spoken: true, telegram: true, tomorrow: true, tasks: true, leave_by: true }
    }
}

/// `HH:MM` as minutes after midnight.
pub fn parse_hhmm(text: &str) -> Option<u32> {
    let (h, m) = text.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl CalendarSection {
    /// A value outside its range makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        let bad = |message: String| SectionError::Invalid { key: "calendar".into(), message };
        if !(5..=1440).contains(&self.default_duration_min) {
            return Err(bad(format!("defaultDurationMin must be between 5 and 1440, not {}", self.default_duration_min)));
        }
        if self.travel_margin_min > 120 {
            return Err(bad(format!("travelMarginMin must be between 0 and 120, not {}", self.travel_margin_min)));
        }
        for (name, value) in [("weekdays", &self.report.weekdays), ("weekends", &self.report.weekends)] {
            if !value.is_empty() && parse_hhmm(value).is_none() {
                return Err(bad(format!("report.{name} must be HH:MM (such as 07:45) or empty, not \"{value}\"")));
            }
        }
        if self.calendars.iter().any(|c| c.trim().is_empty()) {
            return Err(bad("calendars must name calendars by their id".into()));
        }
        if !self.calendars.contains(&self.default_calendar) {
            return Err(bad(format!("defaultCalendar \"{}\" must be one of calendars ({})", self.default_calendar, self.calendars.join(", "))));
        }
        Ok(())
    }
}

impl Settings {
    /// settings.calendar.
    pub fn calendar_config(&self) -> CalendarSection {
        self.section::<CalendarSection>("calendar").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_section_has_the_defaults_of_the_spec() {
        assert_eq!(
            serde_json::to_value(CalendarSection::default()).unwrap(),
            json!({
                "enabled": false, "clientId": "", "calendars": ["primary"], "defaultCalendar": "primary", "defaultDurationMin": 60, "home": "", "travelMarginMin": 10,
                "report": {"enabled": true, "weekdays": "07:45", "weekends": "09:00", "spoken": true, "telegram": true, "tomorrow": true, "tasks": true, "leaveBy": true}
            }),
            "calendar.md section 2"
        );
        assert_eq!(Settings::defaults().calendar_config(), CalendarSection::default(), "a missing section has the defaults: off");
    }

    #[test]
    fn a_patch_is_checked_and_a_refused_one_changes_nothing() {
        let mut s = Settings::defaults();
        s.patch(json!({"calendar": {"calendars": ["primary", "family@group.calendar.google.com"], "report": {"weekdays": "", "weekends": "10:30"}}})).unwrap();
        let c = s.calendar_config();
        assert_eq!(c.calendars.len(), 2);
        assert!(c.report.weekdays.is_empty() && c.report.weekends == "10:30" && c.report.spoken, "a missing field takes its value");
        for (patch, why) in [
            (json!({"calendar": {"defaultDurationMin": 4}}), "defaultDurationMin must be between 5 and 1440, not 4"),
            (json!({"calendar": {"defaultDurationMin": 1441}}), "between 5 and 1440"),
            (json!({"calendar": {"travelMarginMin": 121}}), "travelMarginMin must be between 0 and 120"),
            (json!({"calendar": {"report": {"weekdays": "7:45"}}}), "report.weekdays must be HH:MM"),
            (json!({"calendar": {"report": {"weekends": "24:00"}}}), "report.weekends must be HH:MM"),
            (json!({"calendar": {"defaultCalendar": "work"}}), "defaultCalendar \"work\" must be one of calendars"),
            (json!({"calendar": {"calendars": [""]}}), "name calendars by their id"),
            (json!({"calendar": {"report": {"weekdayz": "07:45"}}}), "unknown field"),
            (json!({"calendar": {"cliendId": "x"}}), "did you mean \"clientId\""),
            (json!({"calendar": {"somethingElse": 1}}), "unknown field"),
        ] {
            let e = s.patch(patch.clone()).unwrap_err();
            assert!(e.contains(why), "{patch}: {e}");
        }
        assert_eq!(s.calendar_config(), c, "a refused patch changes nothing");
        // Moving the default and the list in one patch is one change.
        s.patch(json!({"calendar": {"calendars": ["work"], "defaultCalendar": "work"}})).unwrap();
        assert_eq!(s.calendar_config().default_calendar, "work");
        assert!(s.patch(json!({"calendar": 3})).is_err(), "the section must be an object");
    }

    #[test]
    fn times_are_read_as_hh_mm() {
        assert_eq!(parse_hhmm("07:45"), Some(465));
        assert_eq!(parse_hhmm("23:59"), Some(1439));
        for bad in ["", "7:45", "24:00", "07:60", "0745", "07:45:00", "aa:bb"] {
            assert_eq!(parse_hhmm(bad), None, "{bad}");
        }
    }
}
