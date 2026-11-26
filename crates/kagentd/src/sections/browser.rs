//! settings.browser (config.md section 11): the DevTools port and binary of KAgent's
//! own browser window, and the attach backend of S27 (browser.md section 2). A section
//! is `#[serde(default)]` as a whole, so a missing field takes the value a missing
//! section gets: `backend: own`, attach off.

use serde::{Deserialize, Serialize};

use super::SectionError;

/// settings.browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrowserSection {
    pub port: u16,
    pub binary: String,
    /// KAgent's own window (the default), or the owner's running browser.
    pub backend: BrowserBackend,
    /// User data directory of the browser to attach to; empty = the first one found.
    pub attach_profile: String,
    /// How far the agent may act in the owner's own tabs without a card.
    pub attach_trust: AttachTrust,
}

impl Default for BrowserSection {
    fn default() -> Self {
        Self { port: 9333, binary: String::new(), backend: BrowserBackend::Own, attach_profile: String::new(), attach_trust: AttachTrust::Strict }
    }
}

impl BrowserSection {
    /// `attachProfile` is empty, an absolute path, or starts with `~/`.
    pub fn validate(&self) -> Result<(), SectionError> {
        let p = self.attach_profile.trim();
        if p.is_empty() || p.starts_with('/') || p.starts_with("~/") {
            return Ok(());
        }
        Err(SectionError::Invalid { key: "browser".into(), message: "attachProfile must be an absolute path, or start with ~/".into() })
    }
}

/// `browser.backend`: KAgent's own window, or the owner's running browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BrowserBackend {
    #[default]
    Own,
    Attach,
}

/// `browser.attachTrust` (browser.md section 4): `strict` asks for every action in the
/// owner's own tabs; `normal` asks as the own profile does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AttachTrust {
    #[default]
    Strict,
    Normal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn browser_section_has_the_attach_fields() {
        // Defaults, a patch, and what is refused (on a patch and on load).
        let mut s = Settings::defaults();
        let section = s.section::<BrowserSection>("browser").unwrap();
        assert_eq!((section.backend, section.attach_profile.as_str(), section.attach_trust), (BrowserBackend::Own, "", AttachTrust::Strict), "a missing section has the defaults: attach is off");
        s.patch(serde_json::json!({"browser": {"backend": "attach", "attachProfile": "~/.config/BraveSoftware/Brave-Browser", "attachTrust": "normal", "port": 9222}})).unwrap();
        let section = s.section::<BrowserSection>("browser").unwrap();
        assert_eq!((section.backend, section.attach_profile.as_str(), section.attach_trust), (BrowserBackend::Attach, "~/.config/BraveSoftware/Brave-Browser", AttachTrust::Normal));
        assert_eq!(s.browser_config(), (9222, String::new()), "the keys of before are still read");
        assert!(s.patch(serde_json::json!({"browser": {"backend": "Brave"}})).unwrap_err().contains("browser"), "an unknown backend is refused");
        assert!(s.patch(serde_json::json!({"browser": {"attachTrust": "yolo"}})).is_err());
        assert_eq!(
            s.patch(serde_json::json!({"browser": {"attachProfile": "BraveSoftware/Brave-Browser"}})).unwrap_err().to_string(),
            "invalid settings.browser: attachProfile must be an absolute path, or start with ~/"
        );
        assert!(s.patch(serde_json::json!({"browser": {"attachprofile": ""}})).unwrap_err().contains("did you mean \"attachProfile\"?"), "a typo is named");
        let section = s.section::<BrowserSection>("browser").unwrap();
        assert_eq!((section.backend, section.attach_trust), (BrowserBackend::Attach, AttachTrust::Normal), "a rejected patch changes nothing");
        for profile in ["/home/me/.config/chromium", " ~/config/x ", ""] {
            s.patch(serde_json::json!({"browser": {"attachProfile": profile}})).unwrap();
        }
        // A file as the daemon writes it: the three keys in camelCase.
        assert!(serde_json::to_string(&section).unwrap().contains("\"attachTrust\":\"normal\""));
        assert_eq!(serde_json::to_value(BrowserSection::default()).unwrap()["backend"], "own");

        // A value that is not allowed is refused on load as well, like every typed section.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        for (text, needle) in [
            (r#"{"browser": {"backend": "Brave"}}"#, "unknown variant"),
            (r#"{"browser": {"attachProfile": "relative/path"}}"#, "attachProfile must be an absolute path"),
        ] {
            std::fs::write(&path, text).unwrap();
            assert!(Settings::load(&path).unwrap_err().to_string().contains(needle), "{text}");
        }
    }
}
