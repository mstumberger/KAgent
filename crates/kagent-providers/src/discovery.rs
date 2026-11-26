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

/// `true` when two ids name one model: they differ only by a leading `models/`.
/// Gemini's OpenAI-compatible `/models` answers `models/gemini-2.5-flash` where its
/// template lists `gemini-3.8-pro` (models.md section 3).
pub fn same_model(a: &str, b: &str) -> bool {
    fn bare(id: &str) -> &str {
        id.strip_prefix("models/").unwrap_or(id)
    }
    bare(a) == bare(b)
}

/// The jobs a model id says it has (models.md section 3): first match wins.
pub fn roles_from_id(id: &str) -> Vec<String> {
    const LIVE: [&str; 3] = ["-live", "native-audio", "realtime"];
    const NO_JOB: [&str; 15] = ["embed", "tts", "transcribe", "audio", "whisper", "lyria", "imagen", "image", "banana", "veo", "robotics", "computer-use", "deep-research", "antigravity", "aqa"];
    let id = id.to_lowercase();
    if LIVE.iter().any(|k| id.contains(k)) {
        vec!["live".into()]
    } else if NO_JOB.iter().any(|k| id.contains(k)) {
        Vec::new()
    } else {
        vec!["chat".into()]
    }
}

fn new_row(id: String) -> ModelEntry {
    let roles = roles_from_id(&id);
    ModelEntry { id, name: String::new(), enabled: false, roles, smart: true, config: ModelConfigPatch::default() }
}

/// Merge discovered ids into a provider's model list: a `models/` twin of a row is
/// that row; a new row arrives disabled, with the roles of its id; existing entries
/// keep their flags and config.
pub fn merge_models(provider: &ProviderEntry, discovered: Vec<String>) -> ProviderEntry {
    let mut out = provider.clone();
    for id in discovered.into_iter().filter(|id| !id.is_empty()) {
        if out.models.iter().any(|m| same_model(&m.id, &id)) {
            continue;
        }
        out.models.push(new_row(id));
    }
    out
}

/// Rows the fetch before S40 marked `chat` get the roles of their id (models.md
/// section 3, item 4): a row whose `config` is empty, whose roles are `["chat"]` and
/// whose id matches the first or the second row of the table. A model `named` in
/// settings is left alone. Answers how many rows changed.
pub fn correct_roles(provider: &mut ProviderEntry, named: &[String]) -> usize {
    let mut fixed = 0;
    for m in &mut provider.models {
        if !m.config.is_empty() || m.roles != ["chat"] || named.iter().any(|r| r == &format!("{}::{}", provider.id, m.id)) {
            continue;
        }
        let roles = roles_from_id(&m.id);
        if roles != ["chat"] {
            m.roles = roles;
            fixed += 1;
        }
    }
    fixed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn entry(id: &str) -> ModelEntry {
        ModelEntry { id: id.into(), name: String::new(), enabled: true, roles: vec!["chat".into()], smart: true, config: ModelConfigPatch::default() }
    }

    fn provider(models: Vec<ModelEntry>) -> ProviderEntry {
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
            models,
            models_fetched_at: None,
            last_error: String::new(),
        }
    }

    #[test]
    fn merge_keeps_flags_and_new_rows_arrive_disabled_at_any_size() {
        let p = provider(vec![entry("a"), entry("b")]);
        let merged = merge_models(&p, vec!["b".into(), "c".into(), "d".into(), "e".into()]);
        let ids: Vec<&str> = merged.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c", "d", "e"], "a known id is not added twice");
        assert!(merged.models.iter().find(|m| m.id == "b").unwrap().enabled, "existing stays enabled");
        assert!(merged.models.iter().filter(|m| m.id != "a" && m.id != "b").all(|m| !m.enabled), "new rows are disabled");
        let merged = merge_models(&provider(vec![]), vec!["only-model".into()]);
        assert!(!merged.models[0].enabled, "disabled in a small list too");
        assert_eq!(p.models.len(), 2, "the provider itself is not changed");
    }

    #[test]
    fn a_models_twin_is_the_row() {
        // Scenario S40 item 2.
        let mut row = entry("gemini-3.8-flash");
        row.enabled = false;
        row.config.context_window = Some(123);
        let p = provider(vec![row]);
        let merged = merge_models(&p, vec!["models/gemini-3.8-flash".into(), "models/gemini-3.8-flash-tts".into(), "models/gemini-4-neon".into()]);
        let ids: Vec<&str> = merged.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-3.8-flash", "models/gemini-3.8-flash-tts", "models/gemini-4-neon"]);
        let kept = &merged.models[0];
        assert!(!kept.enabled && kept.config.context_window == Some(123), "the row keeps its id, flags and config");
        let (tts, neon) = (&merged.models[1], &merged.models[2]);
        assert!(!tts.enabled && tts.roles.is_empty(), "the text-to-speech model arrives disabled with no role");
        assert!(!neon.enabled && neon.roles == ["chat"], "the chat model arrives disabled with role chat");
        // Either way round, and two twins in one answer are one row.
        assert!(same_model("models/x", "x") && !same_model("x", "y"));
        let merged = merge_models(&provider(vec![]), vec!["models/gemini-3.8-flash".into(), "gemini-3.8-flash".into()]);
        assert_eq!(merged.models.len(), 1);
    }

    #[test]
    fn roles_from_the_id() {
        for (id, roles) in [
            ("models/gemini-3.8-flash-tts", vec![]),
            ("models/lyria-3.5", vec![]),
            ("text-embedding-3-large", vec![]),
            ("models/gemini-3.8-live", vec!["live"]),
            ("gemini-2.0-flash-exp-native-audio", vec!["live"]),
            ("gpt-realtime", vec!["live"]),
            ("some-chat-model", vec!["chat"]),
            ("qwen2.5:7b-instruct", vec!["chat"]),
        ] {
            assert_eq!(roles_from_id(id), roles, "{id}");
        }
    }

    #[test]
    fn wrong_rows_of_the_old_fetch_get_the_roles_of_their_id_at_load() {
        // Scenario S40 item 3: the file before S40 marked every fetched model `chat`.
        let fetched = |id: &str| ModelEntry { enabled: false, ..entry(id) };
        let mut p = provider(vec![fetched("models/gemini-3.8-flash-tts"), fetched("models/gemini-3.8-live"), entry("gemini-3.8-pro"), entry("models/gemini-3.8-flash-tts")]);
        p.id = "gemini".into();
        let named = ["gemini::models/gemini-3.8-flash-tts".to_string()];
        let before = p.clone();
        assert_eq!(correct_roles(&mut p, &[]), 3, "both text-to-speech rows and the live row");
        assert_eq!(correct_roles(&mut p, &[]), 0, "idempotent: a second load changes nothing");
        let roles: Vec<&str> = p.models.iter().flat_map(|m| m.roles.iter().map(String::as_str)).collect();
        assert_eq!(roles, ["live", "chat"], "the text-to-speech rows have no job, the live row has live, the chat model stays");
        for (was, now) in before.models.iter().zip(&p.models) {
            assert_eq!((was.id.as_str(), was.enabled, &was.config), (now.id.as_str(), now.enabled, &now.config), "nothing else moves");
        }

        // The model that is the chatModel is left alone.
        let mut p = provider(vec![fetched("models/gemini-3.8-flash-tts"), fetched("models/gemini-3.8-live"), entry("gemini-3.8-pro")]);
        p.id = "gemini".into();
        assert_eq!(correct_roles(&mut p, &named), 1);
        assert_eq!(p.models[0].roles, ["chat"], "named in settings");
        assert_eq!(p.models[1].roles, ["live"]);
        assert_eq!(p.models[2].roles, ["chat"], "a chat model stays one");

        // A row the owner configured is not the old fetch's: it stays too.
        let mut own = entry("models/gemini-3.8-flash-tts");
        own.config.context_window = Some(8_192);
        let mut p = provider(vec![own]);
        assert_eq!(correct_roles(&mut p, &[]), 0);
        assert_eq!(p.models[0].roles, ["chat"]);
    }

    #[test]
    fn anthropic_base_normalization() {
        assert_eq!(anthropic_messages_base("https://api.anthropic.com/v1"), "https://api.anthropic.com");
        assert_eq!(anthropic_messages_base("https://api.anthropic.com"), "https://api.anthropic.com");
        assert_eq!(anthropic_messages_base("https://api.anthropic.com/"), "https://api.anthropic.com");
    }
}
