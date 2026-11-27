//! settings.voice (config.md section 11): who thinks in a voice session and what the
//! live model gets (voice.md). The section type moved here from `sections.rs` in S26.

use serde::{Deserialize, Serialize};

use super::SectionError;
use crate::settings::{known_groups, Settings};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct VoiceSection {
    /// `routed`: the chat model thinks and the live model reads its reply; `live`: the
    /// live model alone; `auto`: the live model decides per utterance.
    pub brain: VoiceBrain,
    /// Groups declared to the live model in `live` and `auto`. Empty: no tools.
    pub tool_groups: Vec<String>,
    /// The live model is told the summary, the memory, the todo list and the open alerts.
    pub context: bool,
    /// Terms the transcription should know (voice.md section 5).
    pub vocabulary: bool,
    /// Pause media players while a voice session is open.
    pub pause_media: bool,
    /// 0..=100: the loudness of KAgent's speech (the page's playback gain), not the system volume.
    pub volume: u8,
}

impl Default for VoiceSection {
    fn default() -> Self {
        Self {
            brain: VoiceBrain::Routed,
            tool_groups: ["desktop", "tasks", "system"].iter().map(|g| g.to_string()).collect(),
            context: true,
            vocabulary: true,
            pause_media: true,
            volume: 100,
        }
    }
}

impl VoiceSection {
    /// The groups must be known tool groups, as in `toolGroups`: an unknown name makes
    /// the settings file invalid, on a patch and on load.
    pub fn validate(&self) -> Result<(), SectionError> {
        if self.volume > 100 {
            return Err(SectionError::Invalid { key: "voice".into(), message: format!("volume must be between 0 and 100, not {}", self.volume) });
        }
        match self.tool_groups.iter().find(|g| !known_groups().contains(&g.as_str())) {
            Some(group) => Err(SectionError::Invalid { key: "voice".into(), message: format!("toolGroups must name tool groups: unknown tool group '{group}'") }),
            None => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VoiceBrain {
    /// The chat model thinks; the live model listens and reads out its reply.
    #[default]
    Routed,
    /// The live model alone.
    Live,
    Auto,
}

impl Settings {
    /// settings.voice: who thinks in a voice session, and what the live model gets.
    pub fn voice_config(&self) -> VoiceSection {
        self.section::<VoiceSection>("voice").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// voice.md section 3: group names are validated like `toolGroups`.
    #[test]
    fn voice_tool_groups_must_be_known_on_patch_and_on_load() {
        let mut s = Settings::defaults();
        assert_eq!(s.voice_config(), VoiceSection::default(), "a missing section has the defaults");
        for group in &s.voice_config().tool_groups {
            assert!(known_groups().contains(&group.as_str()), "the default group {group} is a known one");
        }
        s.patch(json!({"voice": {"toolGroups": ["desktop", "system"]}})).unwrap();
        let err = s.patch(json!({"voice": {"toolGroups": ["desktop", "nope"]}})).unwrap_err();
        assert!(err.contains("invalid settings.voice: toolGroups must name tool groups: unknown tool group 'nope'"), "{err}");
        assert_eq!(s.voice_config().tool_groups, vec!["desktop", "system"], "a rejected patch changes nothing");
        // An empty list is no tools (not every group, as for chat): the live model of
        // `live` mode then has nothing to call.
        s.patch(json!({"voice": {"toolGroups": []}})).unwrap();
        assert!(s.voice_config().tool_groups.is_empty());
        s.patch(json!({"voice": {"toolGroups": ["router", "other", "agent", "memory"]}})).unwrap();
        assert_eq!(s.voice_config().tool_groups.len(), 4);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, json!({"voice": {"brain": "auto", "toolGroups": ["router", "bogus"]}}).to_string()).unwrap();
        let err = Settings::load(&path).unwrap_err().to_string();
        assert!(err.contains("invalid settings.voice: toolGroups must name tool groups: unknown tool group 'bogus'"), "refused on load: {err}");
    }

    #[test]
    fn every_field_is_read() {
        let mut s = Settings::defaults();
        s.patch(json!({"voice": {"brain": "auto", "toolGroups": ["tasks"], "context": false, "vocabulary": false}})).unwrap();
        let v = s.voice_config();
        assert_eq!((v.brain, v.tool_groups.clone(), v.context, v.vocabulary, v.pause_media), (VoiceBrain::Auto, vec!["tasks".to_string()], false, false, true));
        assert_eq!(serde_json::to_value(&v).unwrap(), json!({"brain": "auto", "toolGroups": ["tasks"], "context": false, "vocabulary": false, "pauseMedia": true, "volume": 100}), "the names of config.md section 11");
    }

    #[test]
    fn volume_is_zero_to_one_hundred_and_defaults_to_full() {
        let mut s = Settings::defaults();
        assert_eq!(s.voice_config().volume, 100);
        s.patch(json!({"voice": {"volume": 0}})).unwrap();
        assert_eq!(s.voice_config().volume, 0, "0 is silence, not a missing value");
        s.patch(json!({"voice": {"volume": 60}})).unwrap();
        assert!(s.patch(json!({"voice": {"volume": 101}})).unwrap_err().contains("volume must be between 0 and 100"));
        assert!(s.patch(json!({"voice": {"volume": -5}})).is_err());
        assert_eq!(s.voice_config().volume, 60, "a refused patch changes nothing");
    }
}
