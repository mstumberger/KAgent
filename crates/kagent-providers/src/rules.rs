//! Model config resolution: builtin rules layered in ZCode's fixed order, then the
//! personal model patch (docs/specs/config.md section 3).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::config::{BuiltinCatalog, ModelConfigPatch, RuleMatch};

/// Fully resolved model configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub enabled: bool,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub supports_tool_call: bool,
    pub supports_image: bool,
    pub reasoning_levels: Vec<String>,
    pub reasoning_patches: BTreeMap<String, Value>,
    pub supports_live: bool,
    pub live_protocol: Option<String>,
    pub request_patch: Value,
    pub roles: Vec<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            context_window: 128_000,
            max_output_tokens: 8_192,
            supports_tool_call: true,
            supports_image: false,
            reasoning_levels: Vec::new(),
            reasoning_patches: BTreeMap::new(),
            supports_live: false,
            live_protocol: None,
            request_patch: Value::Object(Default::default()),
            roles: vec!["chat".into()],
        }
    }
}

impl ModelConfig {
    fn apply(&mut self, patch: &ModelConfigPatch) {
        if let Some(v) = patch.enabled {
            self.enabled = v;
        }
        if let Some(v) = patch.context_window {
            self.context_window = v;
        }
        if let Some(v) = patch.max_output_tokens {
            self.max_output_tokens = v;
        }
        if let Some(v) = patch.supports_tool_call {
            self.supports_tool_call = v;
        }
        if let Some(v) = patch.supports_image {
            self.supports_image = v;
        }
        if let Some(v) = &patch.reasoning_levels {
            self.reasoning_levels = v.clone();
        }
        if let Some(v) = &patch.reasoning_patches {
            self.reasoning_patches = v.clone();
        }
        if let Some(v) = patch.supports_live {
            self.supports_live = v;
        }
        if let Some(v) = &patch.live_protocol {
            self.live_protocol = Some(v.clone());
        }
        if let Some(v) = &patch.request_patch {
            self.request_patch = v.clone();
        }
        if let Some(v) = &patch.roles {
            self.roles = v.clone();
        }
    }
}

fn regex_matches(pattern: &str, text: &str) -> bool {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    // Compiled once per pattern: resolve() runs on every turn over ~370 rules.
    static CACHE: OnceLock<Mutex<HashMap<String, Option<regex::Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().unwrap();
    let re = cache.entry(pattern.to_string()).or_insert_with(|| {
        // Anchored, case-insensitive, same convention ZCode uses. Invalid patterns
        // never match (logged once).
        let anchored = format!("^(?:{pattern})$");
        let built = regex::RegexBuilder::new(&anchored).case_insensitive(true).build();
        if let Err(e) = &built {
            tracing::warn!(pattern, "invalid model rule regex: {e}");
        }
        built.ok()
    });
    re.as_ref().is_some_and(|re| re.is_match(text))
}

fn rule_matches(m: &RuleMatch, model_id: &str, api_type: &str, base_url: &str, template_id: &str) -> bool {
    let Some(model_pattern) = &m.model else { return false };
    if !regex_matches(model_pattern, model_id) {
        return false;
    }
    if let Some(api) = &m.api_type {
        if api != api_type {
            return false;
        }
    }
    if let Some(base) = &m.base_url {
        if !regex_matches(base, base_url) {
            return false;
        }
    }
    if let Some(t) = &m.template_id {
        if t != template_id {
            return false;
        }
    }
    true
}

/// Rule precedence class (lowest first), fixed as in ZCode.
fn precedence(m: &RuleMatch) -> u8 {
    if m.template_id.is_some() {
        3
    } else if m.base_url.is_some() {
        2
    } else if m.api_type.is_some() {
        1
    } else {
        0
    }
}

/// Resolve a model's config: defaults, then builtin rules (class by class, file
/// order inside a class), then the personal model patch.
pub fn resolve(
    catalog: &BuiltinCatalog,
    model_id: &str,
    api_type: &str,
    base_url: &str,
    template_id: &str,
    personal: &ModelConfigPatch,
) -> ModelConfig {
    let mut out = ModelConfig::default();
    for class in 0..=3 {
        for rule in &catalog.model_rules {
            if precedence(&rule.match_) != class {
                continue;
            }
            if rule_matches(&rule.match_, model_id, api_type, base_url, template_id) {
                out.apply(&rule.config);
            }
        }
    }
    out.apply(personal);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ModelRule, Template};
    use serde_json::json;

    fn catalog() -> BuiltinCatalog {
        let raw = include_str!("../../../config/providers.builtin.json");
        let mut cat: BuiltinCatalog = serde_json::from_str(raw).unwrap();
        // Synthetic rules with known values to pin the precedence order.
        cat.model_rules = vec![
            ModelRule {
                match_: RuleMatch { model: Some("mymodel".into()), ..Default::default() },
                config: ModelConfigPatch { context_window: Some(100_000), ..Default::default() },
            },
            ModelRule {
                match_: RuleMatch {
                    model: Some("mymodel".into()),
                    api_type: Some("openai-chat-completions".into()),
                    ..Default::default()
                },
                config: ModelConfigPatch { context_window: Some(150_000), ..Default::default() },
            },
            ModelRule {
                match_: RuleMatch {
                    model: Some("mymodel".into()),
                    api_type: Some("openai-chat-completions".into()),
                    base_url: Some("https://x\\.example".into()),
                    ..Default::default()
                },
                config: ModelConfigPatch { context_window: Some(200_000), ..Default::default() },
            },
        ];
        cat.templates = vec![Template { id: "t".into(), ..Default::default() }];
        cat
    }

    #[test]
    fn precedence_model_api_site() {
        let cat = catalog();
        let c = resolve(&cat, "mymodel", "openai-chat-completions", "https://x.example", "t", &Default::default());
        assert_eq!(c.context_window, 200_000, "site rule wins over api and model rules");
        let c = resolve(&cat, "mymodel", "openai-chat-completions", "https://other", "t", &Default::default());
        assert_eq!(c.context_window, 150_000, "api rule wins over model-only rule");
        let c = resolve(&cat, "mymodel", "anthropic-messages", "https://other", "t", &Default::default());
        assert_eq!(c.context_window, 100_000, "model-only rule applies");
    }

    #[test]
    fn personal_patch_wins() {
        let cat = catalog();
        let personal = ModelConfigPatch { context_window: Some(50_000), ..Default::default() };
        let c = resolve(&cat, "mymodel", "openai-chat-completions", "https://x.example", "t", &personal);
        assert_eq!(c.context_window, 50_000);
    }

    #[test]
    fn generated_catalog_resolves_defaults() {
        let cat: BuiltinCatalog = serde_json::from_str(include_str!("../../../config/providers.builtin.json")).unwrap();
        // ZCode ships a global `.*` model rule: contextWindow 1M, tool calls on.
        let c = resolve(&cat, "zzz-unknown-model", "openai-chat-completions", "https://api.example.com/v1", "x", &Default::default());
        assert_eq!(c.context_window, 1_000_000);
        assert!(c.supports_tool_call);
        assert_eq!(c.roles, vec!["chat".to_string()]);
        // Ollama's OpenAI endpoint truncates at the server default (measured 4096).
        let c = resolve(&cat, "zzz-unknown-model", "openai-chat-completions", "http://127.0.0.1:11434/v1", "ollama", &Default::default());
        assert_eq!(c.context_window, 4096);
        // Native Ollama: KAgent sends num_ctx, so the window is real.
        let c = resolve(&cat, "qwen2.5:7b-instruct", "ollama-native", "http://127.0.0.1:11434", "ollama", &Default::default());
        assert_eq!((c.context_window, c.max_output_tokens), (16384, 4096));
    }

    #[test]
    fn scoped_rules_stay_scoped() {
        // Regression: RuleMatch used `rename` instead of `rename_all`, so apiType /
        // baseUrl / templateId were dropped and every scoped rule became global.
        let cat: BuiltinCatalog = serde_json::from_str(include_str!("../../../config/providers.builtin.json")).unwrap();
        let scoped = cat.model_rules.iter().filter(|r| r.match_.api_type.is_some() || r.match_.base_url.is_some() || r.match_.template_id.is_some()).count();
        assert!(scoped > 100, "only {scoped} scoped rules parsed");
        let bad: Result<RuleMatch, _> = serde_json::from_str(r#"{"model":".*","baseURL":"x"}"#);
        assert!(bad.is_err());
    }

    #[test]
    fn regex_is_case_insensitive_and_anchored() {
        assert!(regex_matches("glm-5\\.3", "GLM-5.3"));
        assert!(!regex_matches("glm-5", "glm-5.3"));
        assert!(regex_matches(".*", "anything"));
        assert!(regex_matches("https://opencode\\.ai/zen/go/v1/?", "https://opencode.ai/zen/go/v1/"));
        assert!(regex_matches("https://opencode\\.ai/zen/go/v1/?", "https://opencode.ai/zen/go/v1"));
        let _ = json!({});
    }
}
