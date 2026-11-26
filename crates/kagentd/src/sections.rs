//! Typed settings sections (foundations.md section 3): the section types, the error a
//! bad one gives, and the readers the daemon uses for the existing sections. A stage
//! that adds a section (config.md section 11) adds its type here and a line in
//! `Settings::validate_sections`.
//!
//! A section is `#[serde(default)]` as a whole, so a missing field takes its value
//! from the type's `Default`: the same values a missing section gives.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::Settings;

/// How far (edit distance, case ignored) a key may be from a field and still count as
/// that field's typo.
const TYPO_DISTANCE: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionError {
    /// A key within edit distance 2 of a field of the section.
    Typo { key: String, suggestion: String },
    /// The settings key holds something other than an object.
    NotObject { key: String },
    /// A field has a wrong type or value.
    Invalid { key: String, message: String },
}

impl std::fmt::Display for SectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Typo { key, suggestion } => write!(f, "unknown key \"{key}\" (did you mean \"{suggestion}\"?)"),
            Self::NotObject { key } => write!(f, "settings.{key} must be an object"),
            Self::Invalid { key, message } => write!(f, "invalid settings.{key}: {message}"),
        }
    }
}

mod browser;
mod voice;
pub use browser::{AttachTrust, BrowserBackend, BrowserSection};
pub use voice::{VoiceBrain, VoiceSection};

/// settings.tasks: how unattended task runs behave (read from S28 on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TasksSection {
    pub on_approval: TaskApproval,
    pub deliver: Vec<String>,
    pub timeout_min: u32,
    pub max_concurrent: u32,
}

impl Default for TasksSection {
    fn default() -> Self {
        Self {
            on_approval: TaskApproval::Deny,
            deliver: ["desktop", "session"].iter().map(|c| c.to_string()).collect(),
            timeout_min: 30,
            max_concurrent: 2,
        }
    }
}

/// What an unattended run does when a tool needs approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskApproval {
    #[default]
    Deny,
    Wait,
}

impl TasksSection {
    /// A value outside its range makes the settings file invalid: `timeoutMin` 1..=1440,
    /// `maxConcurrent` 1..=8, `deliver` only known channels.
    pub fn validate(&self) -> Result<(), SectionError> {
        check_deliver(&self.deliver).and_then(|_| check_timeout(self.timeout_min)).and_then(|_| check_concurrent(self.max_concurrent)).map_err(|message| SectionError::Invalid { key: "tasks".into(), message })
    }
}

/// The channels a task may deliver to (automations.md section 1): a desktop
/// notification, the session of the run, Telegram (nothing until S29).
pub const DELIVER_CHANNELS: [&str; 3] = ["desktop", "session", "telegram"];

/// `deliver` names known channels only. The same rule for settings.tasks and for a task.
pub fn check_deliver(channels: &[String]) -> Result<(), String> {
    for channel in channels {
        if !DELIVER_CHANNELS.contains(&channel.as_str()) {
            return Err(format!("deliver: unknown channel '{channel}' ({})", DELIVER_CHANNELS.join(", ")));
        }
    }
    Ok(())
}

/// `timeoutMin`: 1..=1440.
pub fn check_timeout(minutes: u32) -> Result<(), String> {
    (1..=1440).contains(&minutes).then_some(()).ok_or_else(|| format!("timeoutMin must be between 1 and 1440, not {minutes}"))
}

/// `maxConcurrent`: 1..=8.
pub fn check_concurrent(runs: u32) -> Result<(), String> {
    (1..=8).contains(&runs).then_some(()).ok_or_else(|| format!("tasks.maxConcurrent must be between 1 and 8, not {runs}"))
}

impl Settings {
    /// settings.tasks: how unattended task runs behave (automations.md).
    pub fn tasks_config(&self) -> TasksSection {
        self.section::<TasksSection>("tasks").unwrap_or_default()
    }
}

/// settings.skills (config.md section 11): the skills hidden by name and the budget
/// of the prompt index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SkillsSection {
    /// Names (not paths) hidden from the prompt and from `load_skill`.
    pub disabled: Vec<String>,
    /// 500..=20000.
    pub index_budget_chars: u32,
}

impl SkillsSection {
    /// A budget outside 500..=20000 makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        if (500..=20000).contains(&self.index_budget_chars) {
            return Ok(());
        }
        Err(SectionError::Invalid { key: "skills".into(), message: "indexBudgetChars must be between 500 and 20000".into() })
    }
}

impl Default for SkillsSection {
    fn default() -> Self {
        Self { disabled: Vec::new(), index_budget_chars: 3000 }
    }
}

/// settings.compaction (config.md section 11): when long conversations are summarized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompactionSection {
    /// false: the oldest turns are dropped with a note, as before S22.
    pub enabled: bool,
    /// 50..=95: the share of the context window at which the oldest turns are summarized.
    pub threshold_pct: u8,
}

impl CompactionSection {
    /// A threshold outside 50..=95 makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        if (50..=95).contains(&self.threshold_pct) {
            return Ok(());
        }
        Err(SectionError::Invalid { key: "compaction".into(), message: "thresholdPct must be between 50 and 95".into() })
    }
}

impl Default for CompactionSection {
    fn default() -> Self {
        Self { enabled: true, threshold_pct: 80 }
    }
}

/// The names of `T`'s fields as they appear in the file (from its default value).
pub(super) fn fields_of<T: Serialize + Default>() -> Vec<String> {
    match serde_json::to_value(T::default()) {
        Ok(Value::Object(o)) => o.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The field `name` is closest to, when that is within `TYPO_DISTANCE` (else `None`:
/// the key is simply unknown and is left alone).
pub(super) fn closest<'a>(name: &str, fields: &'a [String]) -> Option<&'a String> {
    let lower = name.to_lowercase();
    let mut best: Option<(&'a String, usize)> = None;
    for f in fields {
        let d = edit_distance(&lower, &f.to_lowercase());
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((f, d));
        }
    }
    best.filter(|(_, d)| *d <= TYPO_DISTANCE).map(|(f, _)| f)
}

/// Levenshtein distance: insertions, deletions and substitutions.
pub(super) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitute = prev[j] + usize::from(ca != *cb);
            cur.push(substitute.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

impl Settings {
    /// settings.desktop: `{"editor": "vscode", "projectRoots": ["~/SystemInterface"]}`.
    pub fn desktop_prefs(&self) -> kagent_core::tool::DesktopPrefs {
        let d = self.extra.get("desktop");
        let editor = d.and_then(|d| d.get("editor")).and_then(Value::as_str).unwrap_or("").trim().to_string();
        kagent_core::tool::DesktopPrefs {
            editor: if editor == "auto" { String::new() } else { editor },
            project_roots: d
                .and_then(|d| d.get("projectRoots"))
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(String::from).filter(|s| !s.trim().is_empty()).collect())
                .unwrap_or_default(),
        }
    }

    /// The agent browser's port and binary. A port outside 1024..=65535 falls back to
    /// the default.
    pub fn browser_config(&self) -> (u16, String) {
        let b = self.section::<BrowserSection>("browser").unwrap_or_default();
        (if (1024..=65535).contains(&b.port) { b.port } else { 9333 }, b.binary)
    }

    /// settings.voice.pauseMedia (default true): pause players during a voice session.
    pub fn voice_pauses_media(&self) -> bool {
        self.section::<VoiceSection>("voice").map(|v| v.pause_media).unwrap_or(true)
    }

    /// settings.skills: hidden skill names and the prompt index budget.
    pub fn skills_config(&self) -> SkillsSection {
        self.section::<SkillsSection>("skills").unwrap_or_default()
    }

    /// settings.compaction: whether, and from what share of the window, long
    /// conversations are summarized.
    pub fn compaction_config(&self) -> CompactionSection {
        self.section::<CompactionSection>("compaction").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_TOOL_GROUPS;

    #[test]
    fn a_typo_is_rejected_with_the_suggestion() {
        let mut s = Settings::defaults();
        // Scenario S17 item 5.
        let err = s.patch(serde_json::json!({"voice": {"bran": "routed"}})).unwrap_err();
        assert_eq!(err, "unknown key \"voice.bran\" (did you mean \"brain\"?)");
        assert!(s.patch(serde_json::json!({"voice": {"pausemedia": false}})).unwrap_err().contains("pauseMedia"), "case does not hide a typo");
        assert!(s.extra.get("voice").is_none(), "a rejected patch leaves nothing behind");
    }

    #[test]
    fn a_known_key_is_not_a_typo_and_a_recased_one_is() {
        let mut s = Settings::defaults();
        s.patch(serde_json::json!({"voice": {"brain": "live", "pauseMedia": false}})).unwrap();
        assert_eq!(s.section::<VoiceSection>("voice").unwrap().brain, VoiceBrain::Live);
        let err = s.patch(serde_json::json!({"voice": {"PauseMedia": true}})).unwrap_err();
        assert_eq!(err, "unknown key \"voice.PauseMedia\" (did you mean \"pauseMedia\"?)", "keys are case-sensitive");
    }

    #[test]
    fn a_far_unknown_key_is_kept_and_its_siblings_still_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut s = Settings::defaults();
        s.patch(serde_json::json!({"voice": {"orbTheme": "kitt", "pauseMedia": false}, "browser": {"port": 9222}})).unwrap();
        s.save(&path).unwrap();
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.extra["voice"]["orbTheme"], "kitt", "written back unchanged");
        assert!(!s.voice_pauses_media(), "the known sibling is read");
        assert_eq!(s.browser_config(), (9222, String::new()));
    }

    #[test]
    fn sections_have_defaults_for_missing_fields() {
        let s = Settings::defaults();
        assert_eq!(s.section::<BrowserSection>("browser").unwrap(), BrowserSection::default());
        assert_eq!(s.section::<VoiceSection>("voice").unwrap(), VoiceSection::default());
        let mut partial = Settings::defaults();
        partial.patch(serde_json::json!({"tasks": {"timeoutMin": 5}, "browser": {"port": 9222}})).unwrap();
        let t = partial.section::<TasksSection>("tasks").unwrap();
        assert_eq!((t.on_approval, t.deliver, t.timeout_min, t.max_concurrent), (TaskApproval::Deny, vec!["desktop".to_string(), "session".to_string()], 5, 2), "a missing field takes the default");
        assert_eq!(partial.browser_config(), (9222, String::new()));
        // A port below 1024 is not used; the default is.
        partial.patch(serde_json::json!({"browser": {"port": 80}})).unwrap();
        assert_eq!(partial.browser_config().0, 9333);
    }

    #[test]
    fn invalid_sections_are_rejected_on_patch_and_on_load() {
        let mut s = Settings::defaults();
        for patch in [
            serde_json::json!({"voice": {"brain": "thinking"}}),
            serde_json::json!({"browser": 9333}),
            serde_json::json!({"tasks": {"timeoutMin": "soon"}}),
            serde_json::json!({"tasks": {"onApproval": "ignore"}}),
        ] {
            assert!(s.patch(patch.clone()).is_err(), "{patch} must be rejected");
        }
        assert_eq!(s, Settings::defaults(), "nothing of the rejected patches was applied");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, serde_json::json!({"browser": {"binray": "brave"}}).to_string()).unwrap();
        let err = Settings::load(&path).unwrap_err().to_string();
        assert!(err.contains("unknown key \"browser.binray\" (did you mean \"binary\"?)"), "{err}");
    }

    #[test]
    fn skills_section_has_defaults_and_a_budget_range() {
        let mut s = Settings::defaults();
        let section = s.section::<SkillsSection>("skills").unwrap();
        assert!(section.disabled.is_empty() && section.index_budget_chars == 3000, "a missing section has the defaults");
        s.patch(serde_json::json!({"skills": {"disabled": ["slow-machine"]}})).unwrap();
        assert_eq!(s.skills_config(), SkillsSection { disabled: vec!["slow-machine".into()], index_budget_chars: 3000 }, "a missing field takes the default");
        for budget in [499u32, 20001, 0] {
            assert_eq!(
                s.patch(serde_json::json!({"skills": {"indexBudgetChars": budget}})).unwrap_err().to_string(),
                "invalid settings.skills: indexBudgetChars must be between 500 and 20000"
            );
        }
        assert_eq!(s.skills_config().index_budget_chars, 3000, "a rejected patch changes nothing");
        s.patch(serde_json::json!({"skills": {"indexBudgetChars": 20000}})).unwrap();
        assert_eq!(s.skills_config().index_budget_chars, 20000);
        assert!(s.patch(serde_json::json!({"skills": {"disable": ["x"]}})).unwrap_err().contains("did you mean \"disabled\"?"));
        // An invalid range is refused on load as well, like every typed section.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, serde_json::json!({"skills": {"disabled": ["slow-machine"], "indexBudgetChars": 100}}).to_string()).unwrap();
        let err = Settings::load(&path).unwrap_err().to_string();
        assert!(err.contains("indexBudgetChars must be between 500 and 20000"), "{err}");
    }

    #[test]
    fn compaction_section_has_defaults_and_a_threshold_range() {
        let mut s = Settings::defaults();
        assert_eq!(s.compaction_config(), CompactionSection { enabled: true, threshold_pct: 80 }, "a missing section has the defaults");
        s.patch(serde_json::json!({"compaction": {"enabled": false}})).unwrap();
        assert_eq!(s.compaction_config(), CompactionSection { enabled: false, threshold_pct: 80 }, "a missing field takes the default");
        s.patch(serde_json::json!({"compaction": {"thresholdPct": 95}})).unwrap();
        assert_eq!(s.compaction_config().threshold_pct, 95, "the top end of the range is taken (50 is the other: 49 and 96 are refused below)");
        for pct in [49u8, 96, 0, 255] {
            assert_eq!(
                s.patch(serde_json::json!({"compaction": {"thresholdPct": pct}})).unwrap_err().to_string(),
                "invalid settings.compaction: thresholdPct must be between 50 and 95"
            );
        }
        assert_eq!(s.compaction_config().threshold_pct, 95, "a rejected patch changes nothing");
        assert!(s.patch(serde_json::json!({"compaction": {"enable": true}})).unwrap_err().contains("did you mean \"enabled\"?"));
        assert!(s.patch(serde_json::json!({"compaction": {"thresholdPct": "most"}})).is_err(), "a wrong type is rejected");
        // An invalid value is refused on load as well, like every typed section.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, serde_json::json!({"compaction": {"thresholdPct": 30}}).to_string()).unwrap();
        assert!(Settings::load(&path).unwrap_err().to_string().contains("thresholdPct must be between 50 and 95"), "refused on load");
    }

    #[test]
    fn desktop_prefs() {
        let mut s = Settings::defaults();
        assert_eq!(s.desktop_prefs(), Default::default());
        s.patch(serde_json::json!({"desktop": {"editor": "VS Code", "projectRoots": ["~/SystemInterface", ""]}})).unwrap();
        assert_eq!(s.desktop_prefs().editor, "VS Code");
        assert_eq!(s.desktop_prefs().project_roots, vec!["~/SystemInterface"]);
        assert!(s.patch(serde_json::json!({"desktop": {"editor": "notepad"}})).is_err());
        assert_eq!(s.desktop_prefs().editor, "VS Code", "rejected patch not applied");
    }

    #[test]
    fn every_default_group_is_a_known_group() {
        let known = crate::settings::known_groups();
        for g in DEFAULT_TOOL_GROUPS {
            assert!(known.contains(g), "{g} must be nameable in toolGroups");
        }
        assert!(known.contains(&"router"), "a group a tool belongs to");
    }

    #[test]
    fn distances() {
        assert_eq!(edit_distance("bran", "brain"), 1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "ab"), 2);
        let fields = ["brain".to_string(), "toolGroups".to_string()];
        assert_eq!(closest("Brain", &fields), Some(&"brain".to_string()));
        assert_eq!(closest("tool_group", &fields), Some(&"toolGroups".to_string()));
        assert_eq!(closest("orbTheme", &fields), None, "far enough to be another key");
    }
}
