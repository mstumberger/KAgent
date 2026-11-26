//! settings.telegram and the pairing record (bots.md section 1, config.md section 11).
//! The section type lives in the module that reads it, as memory's does, so
//! `sections.rs` stays near 400 lines.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::sections::SectionError;
use crate::settings::Settings;

/// settings.telegram: off until the owner pairs a chat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramSection {
    pub enabled: bool,
    /// New alerts of severity `review` and `high` are pushed to the paired chat.
    pub notify_alerts: bool,
    /// A card that waits `approval_forward_sec` is sent to the paired chat with buttons.
    pub forward_approvals: bool,
    /// 5..=600.
    pub approval_forward_sec: u32,
    /// Root (privileged) cards may be forwarded too.
    pub allow_privileged: bool,
}

impl Default for TelegramSection {
    fn default() -> Self {
        Self { enabled: false, notify_alerts: true, forward_approvals: false, approval_forward_sec: 20, allow_privileged: false }
    }
}

impl TelegramSection {
    /// `approvalForwardSec` outside 5..=600 makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        if (5..=600).contains(&self.approval_forward_sec) {
            return Ok(());
        }
        Err(SectionError::Invalid { key: "telegram".into(), message: format!("approvalForwardSec must be between 5 and 600, not {}", self.approval_forward_sec) })
    }
}

impl Settings {
    /// settings.telegram.
    pub fn telegram_config(&self) -> TelegramSection {
        self.section::<TelegramSection>("telegram").unwrap_or_default()
    }
}

/// `<config>/telegram.json` (0600): the one chat that is paired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pairing {
    pub bot_username: String,
    pub chat_id: i64,
    pub user_id: i64,
    /// RFC 3339.
    pub paired_at: String,
}

impl Pairing {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("telegram.json")
    }

    /// The pairing, or `None` when there is none. A file that cannot be read is not a
    /// pairing: it is named in the log and ignored.
    pub fn load(config_dir: &Path) -> Option<Self> {
        let path = Self::path(config_dir);
        let raw = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Self>(&raw) {
            Ok(pairing) => Some(pairing),
            Err(e) => {
                tracing::warn!("{}: not read, so Telegram is not paired: {e}", path.display());
                None
            }
        }
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        write_private(&Self::path(config_dir), serde_json::to_vec_pretty(self)?)
    }

    /// Forget the chat; forgetting none is not an error.
    pub fn forget(config_dir: &Path) -> anyhow::Result<()> {
        match std::fs::remove_file(Self::path(config_dir)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// A file of 0600, written whole. It is created private, so its content is never
/// readable by others, not even between its creation and a `chmod`.
pub(super) fn write_private(path: &Path, bytes: Vec<u8>) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(&bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// How long a pairing code is good for.
pub const CODE_TTL: Duration = Duration::from_secs(600);

/// A pairing code (bots.md section 2): six digits, ten minutes, one use. "One use" is
/// the bot's: it takes the code from memory the moment a chat pairs with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub code: String,
    pub expires_at: SystemTime,
}

impl PairingCode {
    pub fn new(now: SystemTime) -> Self {
        use rand::Rng;
        Self { code: format!("{:06}", rand::rng().random_range(0..1_000_000u32)), expires_at: now + CODE_TTL }
    }

    /// `true` when `guess` is this code and it is still good at `now`.
    pub fn matches(&self, guess: &str, now: SystemTime) -> bool {
        !self.expired(now) && self.code == guess.trim()
    }

    pub fn expired(&self, now: SystemTime) -> bool {
        now >= self.expires_at
    }

    /// What the settings API answers: the code and when it expires.
    pub fn view(&self) -> serde_json::Value {
        let at = chrono::DateTime::<chrono::Utc>::from(self.expires_at);
        json!({ "code": self.code, "expiresAt": at.to_rfc3339() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_section_has_the_defaults_of_the_spec_and_refuses_what_is_out_of_range() {
        let mut s = Settings::defaults();
        assert_eq!(s.telegram_config(), TelegramSection::default(), "a missing section has the defaults: off");
        assert_eq!(
            serde_json::to_value(TelegramSection::default()).unwrap(),
            json!({"enabled": false, "notifyAlerts": true, "forwardApprovals": false, "approvalForwardSec": 20, "allowPrivileged": false}),
            "config.md section 11"
        );
        s.patch(json!({"telegram": {"enabled": true, "approvalForwardSec": 600}})).unwrap();
        let t = s.telegram_config();
        assert!(t.enabled && t.notify_alerts && !t.forward_approvals && !t.allow_privileged, "a missing field takes the default");
        assert_eq!(t.approval_forward_sec, 600, "the top end of the range is taken");
        for sec in [4u32, 601, 0] {
            assert_eq!(
                s.patch(json!({"telegram": {"approvalForwardSec": sec}})).unwrap_err().to_string(),
                format!("invalid settings.telegram: approvalForwardSec must be between 5 and 600, not {sec}")
            );
        }
        assert_eq!(s.telegram_config().approval_forward_sec, 600, "a rejected patch changes nothing");
        s.patch(json!({"telegram": {"approvalForwardSec": 5}})).unwrap();
        assert_eq!(s.telegram_config().approval_forward_sec, 5, "the other end too");
        assert!(s.patch(json!({"telegram": {"forwardApprovals": "yes"}})).is_err(), "a wrong type is rejected");
        assert!(s.patch(json!({"telegram": 3})).is_err(), "the section must be an object");
        let typo = s.patch(json!({"telegram": {"allowPriviledged": true}})).unwrap_err();
        assert!(typo.contains("did you mean \"allowPrivileged\"?"), "{typo}");
        assert!(s.patch(json!({"telegram": {"notifyalert": true}})).unwrap_err().contains("did you mean \"notifyAlerts\"?"), "case does not hide a typo");
        assert!(!s.telegram_config().allow_privileged, "nothing of a refused patch stays");

        // An invalid value is refused on load as well, like every typed section.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, json!({"telegram": {"approvalForwardSec": 2}}).to_string()).unwrap();
        assert!(Settings::load(&path).unwrap_err().to_string().contains("approvalForwardSec must be between 5 and 600"), "refused on load");
    }

    #[test]
    fn the_pairing_record_round_trips_private_and_a_damaged_file_is_no_pairing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Pairing::load(dir.path()), None, "no file: not paired");
        let pairing = Pairing { bot_username: "kagent_bot".into(), chat_id: 42, user_id: 7, paired_at: "2026-11-26T10:00:00+00:00".into() };
        pairing.save(dir.path()).unwrap();
        assert_eq!(Pairing::load(dir.path()), Some(pairing.clone()));
        let path = Pairing::path(dir.path());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "only the owner may read it");
        let raw: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(raw, json!({"botUsername": "kagent_bot", "chatId": 42, "userId": 7, "pairedAt": "2026-11-26T10:00:00+00:00"}), "config.md section 11");
        assert!(!path.with_extension("tmp").exists(), "written whole, in one step");

        // Damaged (here: half a record): not a pairing, and nothing else breaks.
        std::fs::write(&path, "{\"chatId\": 42}").unwrap();
        assert_eq!(Pairing::load(dir.path()), None);
        Pairing::forget(dir.path()).unwrap();
        assert!(!path.exists());
        Pairing::forget(dir.path()).unwrap();
    }

    #[test]
    fn a_pairing_code_is_six_digits_good_for_ten_minutes() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let code = PairingCode::new(t0);
        assert_eq!(code.code.len(), 6, "{}", code.code);
        assert!(code.code.bytes().all(|b| b.is_ascii_digit()), "{}", code.code);
        assert!(code.matches(&code.code, t0));
        assert!(code.matches(&format!("  {} ", code.code), t0), "what a message carries around the code is not part of it");
        assert!(!code.matches("0000000", t0) && !code.matches("", t0), "another text is not the code");
        assert!(code.matches(&code.code, t0 + CODE_TTL - Duration::from_secs(1)), "good for ten minutes");
        assert!(!code.matches(&code.code, t0 + CODE_TTL), "expired after ten minutes");
        assert!(code.expired(t0 + CODE_TTL));
        assert_eq!(code.expires_at, t0 + CODE_TTL);
        let at = chrono::DateTime::<chrono::Utc>::from(t0 + CODE_TTL);
        assert_eq!(code.view(), json!({"code": code.code, "expiresAt": at.to_rfc3339()}), "the settings API shows the code and its expiry");
        let shape: Vec<String> = (0..200).map(|_| PairingCode::new(t0).code).collect();
        assert!(shape.iter().any(|c| c.starts_with('0')), "six digits, also with a leading zero: {:?} {:?}", shape[0], shape[1]);
    }
}
