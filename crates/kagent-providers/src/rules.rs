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
    /// Back to the defaults for what the model dialog edits (models.md section 2). What
    /// the rules decide for the owner (the request patch of a site, the live protocol) stays.
    fn reset_editable(&mut self) {
        let d = Self::default();
        self.context_window = d.context_window;
        self.max_output_tokens = d.max_output_tokens;
        self.supports_tool_call = d.supports_tool_call;
        self.supports_image = d.supports_image;
        self.reasoning_levels = d.reasoning_levels;
        self.reasoning_patches = d.reasoning_patches;
    }

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
/// order inside a class), then the personal model patch. A manual model (`smart`
/// false) takes the rules for what its owner cannot edit only: the six editable
/// fields go back to their defaults before the patch (models.md section 2).
pub fn resolve(
    catalog: &BuiltinCatalog,
    model_id: &str,
    api_type: &str,
    base_url: &str,
    template_id: &str,
    personal: &ModelConfigPatch,
    smart: bool,
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
    if !smart {
        out.reset_editable();
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
        let c = resolve(&cat, "mymodel", "openai-chat-completions", "https://x.example", "t", &Default::default(), true);
        assert_eq!(c.context_window, 200_000, "site rule wins over api and model rules");
        let c = resolve(&cat, "mymodel", "openai-chat-completions", "https://other", "t", &Default::default(), true);
        assert_eq!(c.context_window, 150_000, "api rule wins over model-only rule");
        let c = resolve(&cat, "mymodel", "anthropic-messages", "https://other", "t", &Default::default(), true);
        assert_eq!(c.context_window, 100_000, "model-only rule applies");
    }

    #[test]
    fn personal_patch_wins() {
        let cat = catalog();
        let personal = ModelConfigPatch { context_window: Some(50_000), ..Default::default() };
        let c = resolve(&cat, "mymodel", "openai-chat-completions", "https://x.example", "t", &personal, true);
        assert_eq!(c.context_window, 50_000);
    }

    #[test]
    fn a_manual_model_ignores_the_rules_for_the_editable_fields_and_keeps_their_request_patch() {
        // Scenario S40 item 1.
        let mut cat = catalog();
        cat.model_rules.push(ModelRule {
            match_: RuleMatch { model: Some("mymodel".into()), ..Default::default() },
            config: ModelConfigPatch {
                max_output_tokens: Some(9_000),
                supports_image: Some(true),
                reasoning_levels: Some(vec!["low".into(), "high".into()]),
                reasoning_patches: Some([("low".to_string(), json!({"effort": "low"}))].into_iter().collect()),
                request_patch: Some(json!({"extra_body": {"thinking": true}})),
                ..Default::default()
            },
        });
        let at = |personal: &ModelConfigPatch, smart| resolve(&cat, "mymodel", "openai-chat-completions", "https://x.example", "t", personal, smart);

        // Smart: the rules under the patch.
        let c = at(&ModelConfigPatch::default(), true);
        assert_eq!((c.context_window, c.max_output_tokens, c.supports_image), (200_000, 9_000, true));
        assert_eq!(c.reasoning_levels, vec!["low".to_string(), "high".to_string()]);
        assert_eq!(c.reasoning_patches["low"], json!({"effort": "low"}));

        // Manual with an empty patch: the six editable fields are the defaults.
        let c = at(&ModelConfigPatch::default(), false);
        assert_eq!((c.context_window, c.max_output_tokens, c.supports_tool_call, c.supports_image), (128_000, 8_192, true, false));
        assert!(c.reasoning_levels.is_empty() && c.reasoning_patches.is_empty());
        // What the owner cannot edit still comes from the rules.
        assert_eq!(c.request_patch, json!({"extra_body": {"thinking": true}}));

        // Manual with a patch: the patch alone.
        let personal = ModelConfigPatch {
            context_window: Some(77_000),
            max_output_tokens: Some(111),
            supports_tool_call: Some(false),
            supports_image: Some(true),
            reasoning_levels: Some(vec!["a".into()]),
            ..Default::default()
        };
        let c = at(&personal, false);
        assert_eq!((c.context_window, c.max_output_tokens, c.supports_tool_call, c.supports_image), (77_000, 111, false, true));
        assert_eq!(c.reasoning_levels, vec!["a".to_string()]);
        assert!(c.reasoning_patches.is_empty(), "no patch for the level: the per-API default applies");
    }

    #[test]
    fn generated_catalog_resolves_defaults() {
        let cat: BuiltinCatalog = serde_json::from_str(include_str!("../../../config/providers.builtin.json")).unwrap();
        // ZCode ships a global `.*` model rule: contextWindow 1M, tool calls on.
        let c = resolve(&cat, "zzz-unknown-model", "openai-chat-completions", "https://api.example.com/v1", "x", &Default::default(), true);
        assert_eq!(c.context_window, 1_000_000);
        assert!(c.supports_tool_call);
        assert_eq!(c.roles, vec!["chat".to_string()]);
        // Ollama's OpenAI endpoint truncates at the server default (measured 4096).
        let c = resolve(&cat, "zzz-unknown-model", "openai-chat-completions", "http://127.0.0.1:11434/v1", "ollama", &Default::default(), true);
        assert_eq!(c.context_window, 4096);
        // Native Ollama: KAgent sends num_ctx, so the window is real.
        let c = resolve(&cat, "qwen2.5:7b-instruct", "ollama-native", "http://127.0.0.1:11434", "ollama", &Default::default(), true);
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
