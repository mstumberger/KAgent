//! `settings.subagents` (config.md section 11): how many subagents run at once. A typed
//! section like those of `sections.rs`, here to keep that file near 400 lines.

use serde::{Deserialize, Serialize};

use kagent_core::agent::children::DEFAULT_MAX_CHILDREN;

use crate::sections::SectionError;
use crate::settings::Settings;

/// `maxConcurrent`: 1..=8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SubagentsSection {
    pub max_concurrent: u32,
}

impl Default for SubagentsSection {
    fn default() -> Self {
        Self { max_concurrent: DEFAULT_MAX_CHILDREN as u32 }
    }
}

impl SubagentsSection {
    /// A value outside 1..=8 makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        (1..=8).contains(&self.max_concurrent).then_some(()).ok_or_else(|| {
            SectionError::Invalid { key: "subagents".into(), message: format!("maxConcurrent must be between 1 and 8, not {}", self.max_concurrent) }
        })
    }
}

impl Settings {
    /// settings.subagents: how many subagents run at once (agents.md section 2).
    pub fn subagents_config(&self) -> SubagentsSection {
        self.section::<SubagentsSection>("subagents").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_has_its_default_and_a_range() {
        let mut s = Settings::defaults();
        assert_eq!(s.subagents_config(), SubagentsSection { max_concurrent: 4 }, "a missing section has the default");
        s.patch(serde_json::json!({ "subagents": { "maxConcurrent": 6 } })).unwrap();
        assert_eq!(s.subagents_config().max_concurrent, 6);
        for n in [0u32, 9, 100] {
            assert_eq!(
                s.patch(serde_json::json!({ "subagents": { "maxConcurrent": n } })).unwrap_err().to_string(),
                format!("invalid settings.subagents: maxConcurrent must be between 1 and 8, not {n}")
            );
        }
        assert_eq!(s.subagents_config().max_concurrent, 6, "a rejected patch changes nothing");
        // The ends of the range, a typo with its suggestion, and a wrong type.
        for n in [1u32, 8] {
            s.patch(serde_json::json!({ "subagents": { "maxConcurrent": n } })).unwrap();
            assert_eq!(s.subagents_config().max_concurrent, n);
        }
        assert!(s.patch(serde_json::json!({ "subagents": { "maxConcurents": 2 } })).unwrap_err().contains("did you mean \"maxConcurrent\"?"));
        assert!(s.patch(serde_json::json!({ "subagents": { "maxConcurrent": "four" } })).is_err());
        assert!(s.patch(serde_json::json!({ "subagents": 4 })).unwrap_err().contains("settings.subagents must be an object"));
        assert!(s.validate_sections().is_ok(), "the section is among those load checks");
    }

    #[test]
    fn a_value_outside_the_range_is_refused_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, serde_json::json!({ "subagents": { "maxConcurrent": 12 } }).to_string()).unwrap();
        let err = Settings::load(&path).unwrap_err().to_string();
        assert!(err.ends_with("invalid settings.subagents: maxConcurrent must be between 1 and 8, not 12"), "{err}");
        std::fs::write(&path, serde_json::json!({ "subagents": { "maxConcurrent": 2 } }).to_string()).unwrap();
        assert_eq!(Settings::load(&path).unwrap().subagents_config().max_concurrent, 2);
    }
}
