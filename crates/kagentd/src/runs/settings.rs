//! `settings.workflows` (config.md section 17): how many workflow runs are active at once.

use serde::{Deserialize, Serialize};

use crate::sections::SectionError;
use crate::settings::Settings;

pub const DEFAULT_MAX_RUNS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WorkflowsSection {
    /// 1..=4.
    pub max_runs: u32,
}

impl Default for WorkflowsSection {
    fn default() -> Self {
        Self { max_runs: DEFAULT_MAX_RUNS }
    }
}

impl WorkflowsSection {
    pub fn validate(&self) -> Result<(), SectionError> {
        (1..=4).contains(&self.max_runs).then_some(()).ok_or_else(|| SectionError::Invalid { key: "workflows".into(), message: format!("maxRuns must be between 1 and 4, not {}", self.max_runs) })
    }
}

impl Settings {
    pub fn workflows_config(&self) -> WorkflowsSection {
        self.section::<WorkflowsSection>("workflows").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_has_a_default_and_a_range() {
        let mut s = Settings::defaults();
        assert_eq!(s.workflows_config().max_runs, 2);
        s.patch(serde_json::json!({"workflows": {"maxRuns": 4}})).unwrap();
        assert_eq!(s.workflows_config().max_runs, 4);
        for n in [0u32, 5] {
            assert_eq!(s.patch(serde_json::json!({"workflows": {"maxRuns": n}})).unwrap_err().to_string(), format!("invalid settings.workflows: maxRuns must be between 1 and 4, not {n}"));
        }
        assert!(s.patch(serde_json::json!({"workflows": {"maxRun": 2}})).unwrap_err().contains("did you mean \"maxRuns\"?"));
        assert!(s.validate_sections().is_ok());
    }
}
