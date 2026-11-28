//! settings.screenWatch (screen-watch.md section 4, config.md). The section type lives in the
//! module that reads it, as the calendar's does.

use serde::{Deserialize, Serialize};

use crate::sections::SectionError;
use crate::settings::Settings;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ScreenWatchSection {
    /// Off: `screen_watch start` is refused (a watch that runs goes on until it ends).
    pub enabled: bool,
    /// A hit is confirmed by the cloud vision model with that one frame. Protected: it lets a frame leave the machine.
    pub cloud_confirm: bool,
    /// A hit and the end of a watch also go to the paired Telegram chat.
    pub telegram: bool,
    /// 1..=120.
    pub default_minutes: u32,
    /// 5..=60.
    pub every_sec: u32,
}

impl Default for ScreenWatchSection {
    fn default() -> Self {
        Self { enabled: true, cloud_confirm: false, telegram: false, default_minutes: 20, every_sec: 10 }
    }
}

impl ScreenWatchSection {
    /// A value outside its range makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        let bad = |message: String| SectionError::Invalid { key: "screenWatch".into(), message };
        if !(1..=120).contains(&self.default_minutes) {
            return Err(bad(format!("defaultMinutes must be between 1 and 120, not {}", self.default_minutes)));
        }
        if !(5..=60).contains(&self.every_sec) {
            return Err(bad(format!("everySec must be between 5 and 60, not {}", self.every_sec)));
        }
        Ok(())
    }
}

impl Settings {
    /// The section as the watch reads it (the defaults when it is missing or invalid).
    pub fn screen_watch_config(&self) -> ScreenWatchSection {
        self.section::<ScreenWatchSection>("screenWatch").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_defaults_are_those_of_the_spec() {
        let s = Settings::defaults();
        assert_eq!(serde_json::to_value(s.screen_watch_config()).unwrap(), json!({"enabled": true, "cloudConfirm": false, "telegram": false, "defaultMinutes": 20, "everySec": 10}));
        assert!(!s.screen_watch_config().cloud_confirm, "nothing leaves the machine until the owner says so");
    }

    #[test]
    fn a_value_outside_its_range_or_a_typo_is_refused() {
        let mut s = Settings::defaults();
        for patch in [
            json!({"screenWatch": {"defaultMinutes": 0}}),
            json!({"screenWatch": {"defaultMinutes": 121}}),
            json!({"screenWatch": {"everySec": 4}}),
            json!({"screenWatch": {"everySec": 61}}),
            json!({"screenWatch": {"cloudConfirmm": true}}),
            json!({"screenWatch": {"enabled": "yes"}}),
            json!({"screenWatch": 5}),
        ] {
            assert!(s.patch(patch.clone()).is_err(), "{patch}");
        }
        assert_eq!(s, Settings::defaults());
        s.patch(json!({"screenWatch": {"defaultMinutes": 120, "everySec": 5, "enabled": false}})).unwrap();
        let c = s.screen_watch_config();
        assert_eq!((c.default_minutes, c.every_sec, c.enabled), (120, 5, false));
    }
}
