//! API type adapters. Each adapter has a pure `build_body` + `Reducer` (unit tested
//! against SSE fixtures) and a small streaming entry used by `client.rs`.

pub mod anthropic;
pub mod ollama;
pub mod openai_chat;
pub mod openai_responses;

use serde_json::Value;

use kagent_core::model::ModelOptions;

use crate::rules::ModelConfig;

/// Deep-merge `over` into `base` (objects recursively, everything else replaced).
pub fn deep_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(k) {
                    Some(slot) if slot.is_object() && v.is_object() => deep_merge(slot, v),
                    Some(slot) => *slot = v.clone(),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// The request fragment for a reasoning level: the model's explicit patch when
/// present, else the per-API default (config.md section 3).
pub fn reasoning_patch(config: &ModelConfig, api: crate::config::ApiType, options: &ModelOptions) -> Option<Value> {
    let level = options.reasoning_level.as_deref()?;
    if let Some(patch) = config.reasoning_patches.get(level) {
        return Some(patch.clone());
    }
    match api {
        crate::config::ApiType::OpenaiChatCompletions => {
            if level == "disabled" {
                None
            } else {
                Some(serde_json::json!({ "reasoning_effort": level }))
            }
        }
        crate::config::ApiType::AnthropicMessages => match level {
            "disabled" => Some(serde_json::json!({"thinking": {"type": "disabled"}})),
            "enabled" => Some(serde_json::json!({"thinking": {"type": "adaptive"}})),
            other => Some(
                serde_json::json!({"thinking": {"type": "adaptive"}, "output_config": {"effort": other}}),
            ),
        },
        crate::config::ApiType::OpenaiResponses => Some(serde_json::json!({"reasoning": {"effort": level}})),
        // ollama::build_body maps the level to `think` itself.
        crate::config::ApiType::OllamaNative => None,
    }
}

/// Parse tool-call arguments that accumulated as a string; invalid JSON becomes
/// `{"_raw": "..."}` so schema validation reports it back to the model.
pub fn parse_tool_arguments(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::Object(Default::default());
    }
    match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => serde_json::json!({ "_raw": raw }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_objects() {
        let mut base = serde_json::json!({"a": 1, "nested": {"x": 1, "y": 2}, "keep": true});
        deep_merge(&mut base, &serde_json::json!({"nested": {"y": 9, "z": 3}, "b": 2}));
        assert_eq!(base, serde_json::json!({"a": 1, "nested": {"x": 1, "y": 9, "z": 3}, "keep": true, "b": 2}));
    }

    #[test]
    fn invalid_arguments_become_raw() {
        assert_eq!(parse_tool_arguments("{\"a\": 1}"), serde_json::json!({"a": 1}));
        assert_eq!(parse_tool_arguments("not json"), serde_json::json!({"_raw": "not json"}));
        assert_eq!(parse_tool_arguments(""), serde_json::json!({}));
    }

    #[test]
    fn reasoning_defaults_per_api() {
        use crate::config::ApiType;
        let cfg = ModelConfig::default();
        let mut o = ModelOptions::default();
        o.reasoning_level = Some("low".into());
        assert_eq!(
            reasoning_patch(&cfg, ApiType::OpenaiChatCompletions, &o),
            Some(serde_json::json!({"reasoning_effort": "low"}))
        );
        o.reasoning_level = Some("disabled".into());
        assert_eq!(reasoning_patch(&cfg, ApiType::OpenaiChatCompletions, &o), None);
        assert_eq!(
            reasoning_patch(&cfg, ApiType::AnthropicMessages, &o),
            Some(serde_json::json!({"thinking": {"type": "disabled"}}))
        );
    }
}
