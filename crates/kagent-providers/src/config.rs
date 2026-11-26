//! Provider config types (docs/specs/config.md sections 3-4): the same API types,
//! template/provider/model split and layering order ZCode uses.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The three provider API shapes KAgent speaks (same names as ZCode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ApiType {
    #[serde(rename = "anthropic-messages")]
    AnthropicMessages,
    #[default]
    #[serde(rename = "openai-chat-completions")]
    OpenaiChatCompletions,
    #[serde(rename = "openai-responses")]
    OpenaiResponses,
    /// Ollama /api/chat: honours num_ctx (the OpenAI endpoint silently does not).
    #[serde(rename = "ollama-native")]
    OllamaNative,
}

impl ApiType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AnthropicMessages => "anthropic-messages",
            Self::OpenaiChatCompletions => "openai-chat-completions",
            Self::OllamaNative => "ollama-native",
            Self::OpenaiResponses => "openai-responses",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApiConfig {
    #[serde(rename = "type", default = "default_api_type")]
    pub api_type: ApiType,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

fn default_api_type() -> ApiType {
    ApiType::OpenaiChatCompletions
}

/// "Add provider" picker entry, generated from ZCode's templates (plus extras).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub key_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub live_models: Vec<String>,
    #[serde(default)]
    pub voices: Vec<String>,
}

/// A user-added provider (providers.json).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub template_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub api: ApiConfig,
    #[serde(default)]
    pub api_key: String,
    /// `secret-service` when the key lives in the desktop keyring (KWallet / GNOME
    /// Keyring) instead of this file. The in-memory entry still holds the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_ref: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    #[serde(default)]
    pub models_fetched_at: Option<String>,
    #[serde(default)]
    pub last_error: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The jobs the model can take (`chat`, `live`; empty is none). A file written
    /// before S40 has no `roles`: its rows are chat models (models.md section 1).
    #[serde(default = "default_roles")]
    pub roles: Vec<String>,
    /// On: `config` holds only what the owner changed and the built-in rules give the
    /// rest. Off: the six editable fields come from `config` alone (models.md section 2).
    #[serde(default = "default_true")]
    pub smart: bool,
    #[serde(default)]
    pub config: ModelConfigPatch,
}

fn default_roles() -> Vec<String> {
    vec!["chat".into()]
}

/// Partial model config; layers overlay each other (every field optional).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfigPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_tool_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_image: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_levels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_patches: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_live: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_patch: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}

impl ModelConfigPatch {
    /// `true` while nothing is set.
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none()
            && self.context_window.is_none()
            && self.max_output_tokens.is_none()
            && self.supports_tool_call.is_none()
            && self.supports_image.is_none()
            && self.reasoning_levels.is_none()
            && self.reasoning_patches.is_none()
            && self.supports_live.is_none()
            && self.live_protocol.is_none()
            && self.request_patch.is_none()
            && self.roles.is_none()
    }

    /// `self` overlaid by `over`: `over`'s present fields win.
    pub fn overlay(&self, over: &Self) -> Self {
        Self {
            enabled: over.enabled.or(self.enabled),
            context_window: over.context_window.or(self.context_window),
            max_output_tokens: over.max_output_tokens.or(self.max_output_tokens),
            supports_tool_call: over.supports_tool_call.or(self.supports_tool_call),
            supports_image: over.supports_image.or(self.supports_image),
            reasoning_levels: over.reasoning_levels.clone().or_else(|| self.reasoning_levels.clone()),
            reasoning_patches: over.reasoning_patches.clone().or_else(|| self.reasoning_patches.clone()),
            supports_live: over.supports_live.or(self.supports_live),
            live_protocol: over.live_protocol.clone().or_else(|| self.live_protocol.clone()),
            request_patch: over.request_patch.clone().or_else(|| self.request_patch.clone()),
            roles: over.roles.clone().or_else(|| self.roles.clone()),
        }
    }
}

/// The generated builtin catalog shipped with the daemon.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BuiltinCatalog {
    #[serde(default)]
    pub templates: Vec<Template>,
    #[serde(default, rename = "modelRules")]
    pub model_rules: Vec<ModelRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelRule {
    #[serde(rename = "match")]
    pub match_: RuleMatch,
    pub config: ModelConfigPatch,
}

// deny_unknown_fields: a misspelled match key must fail loudly at load time instead of
// silently turning a scoped rule into a global one.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleMatch {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_type: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub template_id: Option<String>,
}

/// The on-disk personal file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProvidersFile {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
}

/// Mask an API key for display: `****` when short, else `abc...wxyz`.
pub fn mask_key(key: &str) -> String {
    if key.len() < 12 {
        "****".into()
    } else {
        format!("{}...{}", &key[..3], &key[key.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masking() {
        assert_eq!(mask_key("sk-ant-abcdefghijkl1234"), "sk-...1234");
        assert_eq!(mask_key("short"), "****");
        assert_eq!(mask_key(""), "****");
    }

    #[test]
    fn overlay_precedence() {
        let base = ModelConfigPatch {
            context_window: Some(100_000),
            reasoning_levels: Some(vec!["low".into()]),
            ..Default::default()
        };
        let over = ModelConfigPatch { context_window: Some(50_000), ..Default::default() };
        let merged = base.overlay(&over);
        assert_eq!(merged.context_window, Some(50_000));
        assert_eq!(merged.reasoning_levels, Some(vec!["low".into()]));
    }

    #[test]
    fn entry_defaults_of_a_file_written_before_s40() {
        let e: ModelEntry = serde_json::from_str(r#"{"id": "m"}"#).unwrap();
        assert!(e.enabled && e.smart, "smart is the default, also for an old file");
        assert_eq!(e.roles, vec!["chat".to_string()], "a row without roles is a chat model");
        // An empty list is no job, and a manual model is written as such.
        let e: ModelEntry = serde_json::from_str(r#"{"id": "m", "roles": [], "smart": false}"#).unwrap();
        assert!(e.roles.is_empty());
        assert!(!e.smart);
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"id":"m","name":"","enabled":true,"roles":[],"smart":false,"config":{}}"#);
    }

    #[test]
    fn parses_generated_catalog() {
        let raw = include_str!("../../../config/providers.builtin.json");
        let cat: BuiltinCatalog = serde_json::from_str(raw).expect("generated catalog parses");
        assert!(cat.templates.len() >= 20, "need >= 20 templates, got {}", cat.templates.len());
        for id in ["anthropic", "openai", "ollama", "gemini"] {
            assert!(cat.templates.iter().any(|t| t.id == id), "missing template {id}");
        }
        assert!(cat.templates.iter().any(|t| t.api.api_type == ApiType::AnthropicMessages));
        assert!(!cat.model_rules.is_empty());
    }
}
