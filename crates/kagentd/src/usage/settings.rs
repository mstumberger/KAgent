//! `settings.usage` (config.md section 15, usage.md section 3.3): the limits the owner types per provider, which KAgent
//! counts against ("free tier: 1500 requests a day"). Never prefilled: they can go stale.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sections::SectionError;
use crate::settings::Settings;

/// The window a known limit counts over: the minute, day or month of the machine's local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownWindow {
    Minute,
    Day,
    Month,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownUnit {
    Requests,
    Tokens,
}

/// One limit: at most `total` requests (or tokens) in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct KnownLimit {
    pub window: KnownWindow,
    pub unit: KnownUnit,
    pub total: u64,
}

/// `knownLimits`: provider id -> its limits.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageSection {
    pub known_limits: BTreeMap<String, Vec<KnownLimit>>,
}

impl UsageSection {
    /// A total is 1 to a trillion; a provider has one limit per window and unit.
    pub fn validate(&self) -> Result<(), SectionError> {
        let bad = |message: String| SectionError::Invalid { key: "usage".into(), message };
        for (provider, limits) in &self.known_limits {
            for (i, l) in limits.iter().enumerate() {
                if l.total == 0 || l.total > 1_000_000_000_000 {
                    return Err(bad(format!("knownLimits.{provider}: a total is 1 to 1000000000000, not {}", l.total)));
                }
                if limits[..i].iter().any(|o| o.window == l.window && o.unit == l.unit) {
                    return Err(bad(format!("knownLimits.{provider}: one limit for each window and unit")));
                }
            }
        }
        Ok(())
    }
}

impl Settings {
    /// settings.usage: the known limits (usage.md section 3.3).
    pub fn usage_config(&self) -> UsageSection {
        self.section::<UsageSection>("usage").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_known_limit_is_read_and_checked() {
        let mut s = Settings::defaults();
        assert!(s.usage_config().known_limits.is_empty(), "a missing section has the default");
        s.patch(json!({"usage": {"knownLimits": {"gemini": [{"window": "day", "unit": "requests", "total": 1500}]}}})).unwrap();
        assert_eq!(s.usage_config().known_limits["gemini"], [KnownLimit { window: KnownWindow::Day, unit: KnownUnit::Requests, total: 1500 }]);
        for (bad, why) in [
            (json!({"window": "day", "unit": "requests", "total": 0}), "total"),
            (json!({"window": "week", "unit": "requests", "total": 5}), "week"),
            (json!({"window": "day", "unit": "requests", "total": 5, "extra": 1}), "extra"),
            (json!({"window": "day", "unit": "usd", "total": 5}), "usd"),
        ] {
            let mut s = Settings::defaults();
            let err = s.patch(json!({"usage": {"knownLimits": {"gemini": [bad]}}})).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(err.contains("usage"), "{why}: {err:?}");
        }
        let mut s = Settings::defaults();
        assert!(s.patch(json!({"usage": {"knownLimits": {"gemini": [{"window": "day", "unit": "requests", "total": 5}, {"window": "day", "unit": "requests", "total": 6}]}}})).is_err(), "two for one window and unit");
        assert!(Settings::defaults().patch(json!({"usage": {"knownLimitz": {}}})).is_err(), "a typo in the section fails loudly");
    }
}
