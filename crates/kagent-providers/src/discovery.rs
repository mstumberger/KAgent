//! Model discovery: "Fetch models" for a provider (openai `/models`, anthropic
//! `/v1/models`, Ollama native `/api/tags`), merged without losing flags.

use serde::Deserialize;

use crate::config::{ApiType, ModelConfigPatch, ModelEntry, ProviderEntry};

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelId>,
}

#[derive(Deserialize)]
struct ModelId {
    id: String,
}

#[derive(Deserialize)]
struct OllamaTags {
    #[serde(default)]
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

fn anthropic_messages_base(base: &str) -> String {
    // ZCode stores anthropic bases with or without a trailing /v1; the messages
    // endpoint is always <base>/v1/messages.
    base.trim_end_matches('/').trim_end_matches("/v1").to_string()
}

/// Fetch the model ids a provider offers. No store mutation here.
pub async fn fetch_models(provider: &ProviderEntry) -> anyhow::Result<Vec<String>> {
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
    let base = provider.api.base_url.trim_end_matches('/');
    let mut req = match provider.api.api_type {
        ApiType::AnthropicMessages => {
            let url = format!("{}/v1/models?limit=1000", anthropic_messages_base(base));
            http.get(url)
                .header("x-api-key", &provider.api_key)
                .header("anthropic-version", "2023-06-01")
        }
        ApiType::OpenaiChatCompletions | ApiType::OpenaiResponses | ApiType::OllamaNative => {
            if base.ends_with(":11434/v1") || provider.api.api_type == ApiType::OllamaNative {
                let url = format!("{}{}", base.trim_end_matches("/v1").trim_end_matches("/api"), "/api/tags");
                return Ok(http
                    .get(url)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<OllamaTags>()
                    .await?
                    .models
                    .into_iter()
                    .map(|m| m.name)
                    .collect());
            }
            http.get(format!("{base}/models")).bearer_auth(&provider.api_key)
        }
    };
    for (k, v) in &provider.api.headers {
        req = req.header(k, v);
    }
    let models = req
        .send()
        .await?
        .error_for_status()?
        .json::<ModelsResponse>()
        .await?
        .data
        .into_iter()
        .map(|m| m.id)
        .collect();
    Ok(models)
}

/// Merge discovered ids into a provider's model list: new ids appended disabled
/// unless the list stays small; existing entries keep their flags and config.
pub fn merge_models(provider: &ProviderEntry, discovered: Vec<String>) -> ProviderEntry {
    let total_after = provider.models.len() + discovered.len();
    let mut out = provider.clone();
    let mut seen: Vec<String> = out.models.iter().map(|m| m.id.clone()).collect();
    for id in discovered {
        if seen.contains(&id) || id.is_empty() {
            continue;
        }
        seen.push(id.clone());
        out.models.push(ModelEntry {
            enabled: total_after < 5,
            id,
            name: String::new(),
            roles: vec!["chat".into()],
            config: ModelConfigPatch::default(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn provider(models: Vec<&str>) -> ProviderEntry {
        ProviderEntry {
            id: "p".into(),
            name: "P".into(),
            template_id: "t".into(),
            enabled: true,
            api: crate::config::ApiConfig {
                api_type: ApiType::OpenaiChatCompletions,
                base_url: "http://127.0.0.1:9999/v1".into(),
                headers: BTreeMap::new(),
            },
            api_key: String::new(),
            api_key_ref: None,
            models: models
                .into_iter()
                .map(|id| ModelEntry {
                    id: id.into(),
                    name: String::new(),
                    enabled: true,
                    roles: vec!["chat".into()],
                    config: ModelConfigPatch::default(),
                })
                .collect(),
            models_fetched_at: None,
            last_error: String::new(),
        }
    }

    #[test]
    fn merge_keeps_flags_and_disables_new_when_large() {
        let p = provider(vec!["a", "b"]);
        let merged = merge_models(&p, vec!["b".into(), "c".into(), "d".into(), "e".into()]);
        let ids: Vec<&str> = merged.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c", "d", "e"]);
        assert!(merged.models.iter().find(|m| m.id == "b").unwrap().enabled, "existing stays enabled");
        assert!(!merged.models.iter().find(|m| m.id == "c").unwrap().enabled, "new disabled when list is large");
    }

    #[test]
    fn merge_enables_new_when_small() {
        let p = provider(vec![]);
        let merged = merge_models(&p, vec!["only-model".into()]);
        assert!(merged.models[0].enabled);
    }

    #[test]
    fn anthropic_base_normalization() {
        assert_eq!(anthropic_messages_base("https://api.anthropic.com/v1"), "https://api.anthropic.com");
        assert_eq!(anthropic_messages_base("https://api.anthropic.com"), "https://api.anthropic.com");
        assert_eq!(anthropic_messages_base("https://api.anthropic.com/"), "https://api.anthropic.com");
    }
}
