//! settings.json: defaults, load/save (0600, atomic), unknown keys preserved, and the
//! patch `PUT /api/settings` applies. Typed views of settings objects are in
//! `sections.rs`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use serde_json::Value;

use kagent_core::permission::{AllowRule, PermissionMode, ToolOverride};
use kagent_core::tool::PrivilegeMethod;

use crate::sections::{closest, fields_of, BrowserSection, CompactionSection, SectionError, SkillsSection, TasksSection, VoiceSection};
use crate::startup::Startup;

pub const DEFAULT_TOOL_GROUPS: &[&str] = &[
    "system", "kernel", "services", "files", "shell", "hardware", "skills", "change", "desktop", "security", "trace", "tasks", "agent", "memory", "web",
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
    Persona { name: "KAgent".into(), address: String::new(), language: "auto".into() }
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
    /// No longer read from S40 on (models.md section 6): the model's own output limit
    /// is the only source. Kept, so that a value in an old file survives a load and a
    /// save, and goes when the owner clears it.
    pub max_output_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

/// Every tool group a settings file may name: the defaults and the groups of
/// `GROUP_PRIORITY` (`router`, `other`).
pub fn known_groups() -> Vec<&'static str> {
    let mut known: Vec<&'static str> = DEFAULT_TOOL_GROUPS.to_vec();
    for g in kagent_core::context::GROUP_PRIORITY {
        if !known.contains(&g) {
            known.push(g);
        }
    }
    known
}

/// Merge a patch into `base` the way `PUT /api/settings` does: an object updates only
/// the keys it names, at every depth; arrays and scalars are replaced.
pub fn merge_patch(base: &mut Value, patch: &Value) {
    if let (Value::Object(base), Value::Object(patch)) = (base, patch) {
        for (key, value) in patch {
            match base.get_mut(key) {
                Some(current) if current.is_object() && value.is_object() => merge_patch(current, value),
                _ => {
                    base.insert(key.clone(), value.clone());
                }
            }
        }
    }
}

/// `toolGroups` in a patch may only name tool groups.
fn check_tool_groups(patch: &Value) -> Result<(), String> {
    let Some(groups) = patch.get("toolGroups").and_then(Value::as_array) else { return Ok(()) };
    let known = known_groups();
    for group in groups.iter().filter_map(Value::as_str) {
        if !known.contains(&group) {
            return Err(format!("invalid settings: unknown tool group '{group}'"));
        }
    }
    Ok(())
}

/// `toolOverrides` in a patch may only be `allow`, `ask` or `deny`; `null` removes the
/// tool's override (the recursive merge cannot drop a key).
fn check_tool_overrides(patch: &Value) -> Result<(), String> {
    let Some(overrides) = patch.get("toolOverrides").and_then(Value::as_object) else { return Ok(()) };
    for (tool, value) in overrides {
        if value.is_null() {
            continue;
        }
        if ToolOverride::parse(value.as_str().unwrap_or("")).is_none() {
            return Err(format!("invalid settings: toolOverrides.{tool} must be allow, ask or deny"));
        }
    }
    Ok(())
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
        s.validate_sections().map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
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

    /// Apply a partial patch (PUT /api/settings). Objects merge key by key, arrays are
    /// replaced, unknown keys land in `extra`. An invalid patch (wrong type, unknown
    /// enum value, unknown tool group, a typo in a typed section) is rejected as a
    /// whole and reported, never half-applied or silently dropped.
    pub fn patch(&mut self, patch: Value) -> Result<(), String> {
        if !patch.is_object() {
            return Err("invalid settings: the patch must be an object".into());
        }
        // Only what the patch names is checked: an entry already in the file (a hand
        // edit) must not block an unrelated update.
        check_tool_groups(&patch)?;
        check_tool_overrides(&patch)?;
        let mut merged = serde_json::to_value(&*self).ok().unwrap_or(Value::Object(Default::default()));
        merge_patch(&mut merged, &patch);
        // `{"tool": null}` in a `toolOverrides` patch removes that override.
        if let Some(Value::Object(overrides)) = merged.get_mut("toolOverrides") {
            for (tool, value) in patch.get("toolOverrides").and_then(Value::as_object).into_iter().flatten() {
                if value.is_null() {
                    overrides.remove(tool);
                }
            }
        }
        if let Value::Object(m) = &mut merged {
            m.remove("schemaVersion");
        }
        let next = serde_json::from_value::<Settings>(merged).map_err(|e| format!("invalid settings: {e}"))?;
        let editor = next.desktop_prefs().editor;
        if !editor.is_empty() && kagent_core::tools::editor::editor_id(&editor).is_none() {
            return Err(format!("invalid settings: unknown desktop.editor '{editor}'"));
        }
        next.validate_sections().map_err(|e| e.to_string())?;
        *self = next;
        Ok(())
    }

    /// One settings object as a typed section. A missing or null section gives the
    /// type's defaults; unknown keys are left alone (they stay in the file); a key that
    /// looks like a misspelt field is an error.
    pub fn section<T>(&self, key: &str) -> Result<T, SectionError>
    where
        T: Serialize + DeserializeOwned + Default,
    {
        let Some(raw) = self.extra.get(key).filter(|v| !v.is_null()) else {
            return Ok(T::default());
        };
        let Value::Object(fields) = raw else {
            return Err(SectionError::NotObject { key: key.into() });
        };
        let known = fields_of::<T>();
        for name in fields.keys() {
            if !known.contains(name) {
                if let Some(suggestion) = closest(name, &known) {
                    return Err(SectionError::Typo { key: format!("{key}.{name}"), suggestion: suggestion.clone() });
                }
            }
        }
        serde_json::from_value(raw.clone()).map_err(|e| SectionError::Invalid { key: key.into(), message: e.to_string() })
    }

    /// Every typed section, as load and `PUT /api/settings` require them to be.
    pub fn validate_sections(&self) -> Result<(), SectionError> {
        self.section::<VoiceSection>("voice")?.validate()?;
        self.section::<BrowserSection>("browser")?.validate()?;
        self.section::<TasksSection>("tasks")?.validate()?;
        self.section::<SkillsSection>("skills")?.validate()?;
        self.section::<CompactionSection>("compaction")?.validate()?;
        self.section::<crate::memory::MemorySection>("memory")?.validate()?;
        self.section::<crate::web::settings::SearchSection>("search")?.validate()?;
        self.section::<crate::bots::settings::TelegramSection>("telegram")?.validate()?;
        self.section::<crate::subagents::SubagentsSection>("subagents")?.validate()?;
        self.section::<crate::hooks::config::HooksSection>("hooks")?.validate()
    }

    /// settings.startWidget (default true): open the small widget when the daemon starts.
    pub fn start_widget(&self) -> bool {
        self.extra.get("startWidget").and_then(Value::as_bool).unwrap_or(true)
    }

    pub fn privilege_method(&self) -> PrivilegeMethod {
        self.privilege.into()
    }
}

#[cfg(test)]
mod tests;
