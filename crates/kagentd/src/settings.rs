//! settings.json: defaults, load/save (0600, atomic), unknown keys preserved.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use kagent_core::permission::{AllowRule, PermissionMode};
use kagent_core::tool::PrivilegeMethod;

pub const DEFAULT_TOOL_GROUPS: &[&str] = &[
    "system", "kernel", "services", "files", "shell", "hardware", "skills", "change", "desktop", "security", "trace", "tasks",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub chat_model: String,
    #[serde(default)]
    pub fast_model: String,
    /// Default tool groups that existed when this file was last migrated; groups added
    /// in later releases are switched on once for existing settings.
    #[serde(default)]
    pub known_tool_groups: Vec<String>,
    /// Chat model used when `chat_model` is overloaded or out of quota (429/503).
    #[serde(default)]
    pub fallback_model: String,
    #[serde(default)]
    pub live_model: String,
    #[serde(default = "default_voice")]
    pub live_voice: String,
    #[serde(default = "default_mode")]
    pub permission_mode: PermissionMode,
    #[serde(default)]
    pub rules: Vec<AllowRule>,
    #[serde(default)]
    pub tool_overrides: BTreeMap<String, String>,
    #[serde(default = "default_groups")]
    pub tool_groups: Vec<String>,
    #[serde(default)]
    pub mcp_tool_groups: Vec<String>,
    #[serde(default = "default_privilege")]
    pub privilege: PrivilegeMethodSerde,
    #[serde(default = "default_iterations")]
    pub max_iterations: usize,
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_sec: u64,
    #[serde(default)]
    pub options: ModelOptionsSerde,
    #[serde(default = "default_persona")]
    pub persona: Persona,
    #[serde(default)]
    pub startup: Startup,
    /// Forward compatibility: unknown keys survive a load/save round trip.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// settings.startup: autostart with the session + greeting on daemon start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Startup {
    /// Keep the systemd user unit kagentd.service enabled (starts at login).
    #[serde(default = "default_true")]
    pub autostart: bool,
    #[serde(default)]
    pub greeting: Greeting,
}

impl Default for Startup {
    fn default() -> Self {
        Self { autostart: true, greeting: Greeting::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Greeting {
    /// Desktop notification greeting once the daemon is up.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Empty = auto ("Good morning/afternoon/evening, <address>. ...").
    #[serde(default)]
    pub text: String,
}

impl Default for Greeting {
    fn default() -> Self {
        Self { enabled: true, text: String::new() }
    }
}

fn default_true() -> bool {
    true
}

fn default_schema() -> u32 {
    1
}
fn default_voice() -> String {
    "Kore".into()
}
fn default_mode() -> PermissionMode {
    PermissionMode::Ask
}
fn default_groups() -> Vec<String> {
    DEFAULT_TOOL_GROUPS.iter().map(|s| s.to_string()).collect()
}
fn default_privilege() -> PrivilegeMethodSerde {
    PrivilegeMethodSerde::Sudo
}
fn default_iterations() -> usize {
    40
}
fn default_approval_timeout() -> u64 {
    600
}
fn default_persona() -> Persona {
    Persona { name: "KAgent".into(), address: "Marko".into(), language: "auto".into() }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Persona {
    pub name: String,
    pub address: String,
    pub language: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum PrivilegeMethodSerde {
    #[default]
    Sudo,
    Pkexec,
    None,
}

impl From<PrivilegeMethodSerde> for PrivilegeMethod {
    fn from(v: PrivilegeMethodSerde) -> Self {
        match v {
            PrivilegeMethodSerde::Sudo => PrivilegeMethod::Sudo,
            PrivilegeMethodSerde::Pkexec => PrivilegeMethod::Pkexec,
            PrivilegeMethodSerde::None => PrivilegeMethod::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelOptionsSerde {
    pub reasoning_level: Option<String>,
    pub max_output_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// Parse `toolOverrides`; unknown values are ignored with a warning.
pub fn parse_overrides(raw: &BTreeMap<String, String>) -> std::collections::HashMap<String, kagent_core::permission::ToolOverride> {
    raw.iter()
        .filter_map(|(tool, v)| match kagent_core::permission::ToolOverride::parse(v) {
            Some(o) => Some((tool.clone(), o)),
            None => {
                tracing::warn!(tool, value = v.as_str(), "ignoring unknown tool override (use allow|ask|deny)");
                None
            }
        })
        .collect()
}

impl Settings {
    pub fn defaults() -> Self {
        serde_json::from_str("{}").unwrap()
    }

    /// Load or create; missing fields filled with defaults.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            let s = Self::defaults();
            s.save(path)?;
            return Ok(s);
        }
        let mut s: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if s.migrate_tool_groups() {
            s.save(path)?;
        }
        Ok(s)
    }

    /// Enable default groups this file has never seen (e.g. `security`, `trace` for
    /// settings written before they existed). Returns true when something changed.
    pub fn migrate_tool_groups(&mut self) -> bool {
        // Files from before this field: they knew the original S0-S9 default groups.
        const ORIGINAL: [&str; 9] = ["system", "kernel", "services", "files", "shell", "hardware", "skills", "change", "desktop"];
        let known: Vec<String> = if self.known_tool_groups.is_empty() { ORIGINAL.iter().map(|g| g.to_string()).collect() } else { self.known_tool_groups.clone() };
        let mut changed = self.known_tool_groups.is_empty();
        for g in DEFAULT_TOOL_GROUPS {
            if !known.iter().any(|k| k == g) && !self.tool_groups.is_empty() && !self.tool_groups.iter().any(|t| t == g) {
                self.tool_groups.push(g.to_string());
                changed = true;
            }
        }
        if self.known_tool_groups.len() != DEFAULT_TOOL_GROUPS.len() {
            self.known_tool_groups = DEFAULT_TOOL_GROUPS.iter().map(|g| g.to_string()).collect();
            changed = true;
        }
        changed
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(serde_json::to_vec_pretty(self)?.as_slice())?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        drop(f);
        std::fs::rename(tmp, path)?;
        Ok(())
    }

    /// Apply a partial patch (PUT /api/settings). Unknown keys land in `extra`.
    /// Merge a partial update. An invalid patch (wrong type, unknown enum value) is
    /// rejected as a whole and reported, never half-applied or silently dropped.
    pub fn patch(&mut self, patch: Value) -> Result<(), String> {
        let merged = serde_json::to_value(&*self).ok().unwrap_or(Value::Object(Default::default()));
        let mut merged = match (merged, patch) {
            (Value::Object(mut m), Value::Object(p)) => {
                for (k, v) in p {
                    m.insert(k, v);
                }
                Value::Object(m)
            }
            (_, p) => p,
        };
        if let Value::Object(m) = &mut merged {
            m.remove("schemaVersion");
        }
        let next = serde_json::from_value::<Settings>(merged).map_err(|e| format!("invalid settings: {e}"))?;
        let editor = next.desktop_prefs().editor;
        if !editor.is_empty() && kagent_core::tools::editor::editor_id(&editor).is_none() {
            return Err(format!("invalid settings: unknown desktop.editor '{editor}'"));
        }
        *self = next;
        Ok(())
    }

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

    /// settings.browser: `{"port": 9333, "binary": ""}` (empty binary = auto-detect).
    pub fn browser_config(&self) -> (u16, String) {
        let b = self.extra.get("browser");
        let port = b.and_then(|b| b.get("port")).and_then(Value::as_u64).filter(|p| (1024..=65535).contains(p)).unwrap_or(9333) as u16;
        (port, b.and_then(|b| b.get("binary")).and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// settings.startWidget (default true): open the small widget when the daemon starts.
    pub fn start_widget(&self) -> bool {
        self.extra.get("startWidget").and_then(Value::as_bool).unwrap_or(true)
    }

    /// settings.voice.pauseMedia (default true): pause players during a voice session.
    pub fn voice_pauses_media(&self) -> bool {
        self.extra.get("voice").and_then(|v| v.get("pauseMedia")).and_then(Value::as_bool).unwrap_or(true)
    }

    pub fn privilege_method(&self) -> PrivilegeMethod {
        self.privilege.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_default_groups_are_enabled_once() {
        let mut s = Settings::defaults();
        s.tool_groups = vec!["system".into(), "desktop".into()];
        s.known_tool_groups = vec![];
        assert!(s.migrate_tool_groups());
        assert!(s.tool_groups.contains(&"security".to_string()) && s.tool_groups.contains(&"trace".to_string()));
        assert!(!s.tool_groups.contains(&"kernel".to_string()), "a group the user turned off stays off");
        // The user now turns security off again: it stays off.
        s.tool_groups.retain(|g| g != "security");
        assert!(!s.migrate_tool_groups());
        assert!(!s.tool_groups.contains(&"security".to_string()));
    }

    #[test]
    fn desktop_browser_and_voice_prefs() {
        let mut s = Settings::defaults();
        assert_eq!(s.desktop_prefs(), Default::default());
        assert_eq!(s.browser_config(), (9333, String::new()));
        assert!(s.voice_pauses_media());
        s.patch(serde_json::json!({"desktop": {"editor": "VS Code", "projectRoots": ["~/SystemInterface", ""]}, "browser": {"port": 80}, "voice": {"pauseMedia": false}})).unwrap();
        assert_eq!(s.desktop_prefs().editor, "VS Code");
        assert_eq!(s.desktop_prefs().project_roots, vec!["~/SystemInterface"]);
        assert_eq!(s.browser_config().0, 9333, "privileged port rejected");
        assert!(!s.voice_pauses_media());
        assert!(s.patch(serde_json::json!({"desktop": {"editor": "notepad"}})).is_err());
        assert_eq!(s.desktop_prefs().editor, "VS Code", "rejected patch not applied");
    }

    #[test]
    fn defaults_roundtrip_and_patch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings::load(&path).unwrap();
        assert_eq!(s.permission_mode, PermissionMode::Ask);
        assert_eq!(s.tool_groups, DEFAULT_TOOL_GROUPS.iter().map(|g| g.to_string()).collect::<Vec<_>>());
        assert!(s.tool_groups.iter().any(|g| g == "security"));
        assert!(s.startup.autostart && s.startup.greeting.enabled, "startup defaults on");
        assert!(path.exists());

        let mut s2 = Settings::load(&path).unwrap();
        s2.patch(serde_json::json!({"permissionMode": "trusted", "futureKey": {"a": 1}})).unwrap();
        // Invalid values are rejected as a whole and leave settings untouched.
        assert!(s2.patch(serde_json::json!({"permissionMode": "yolo", "maxIterations": 1})).is_err());
        assert_eq!(s2.permission_mode, PermissionMode::Trusted);
        assert_ne!(s2.max_iterations, 1);
        assert_eq!(s2.permission_mode, PermissionMode::Trusted);
        assert_eq!(s2.extra.get("futureKey").and_then(|v| v.get("a")), Some(&serde_json::json!(1)));
        s2.save(&path).unwrap();
        let s3 = Settings::load(&path).unwrap();
        assert_eq!(s3.permission_mode, PermissionMode::Trusted);
        assert!(s3.extra.contains_key("futureKey"));

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
