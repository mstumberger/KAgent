//! The vision models of a watch (screen-watch.md section 2, steps 3 and 5): which local
//! model looks, what "local" means, and the two calls. The frame goes to the local model
//! only; the cloud model gets one frame per hit and only when `cloudConfirm` is on.

use std::time::Duration;

use base64::Engine as _;
use kagent_providers::{ApiType, ProviderStore};
use serde_json::{json, Value};

use crate::local_models::{ollama, LocalModels};
use crate::settings::Settings;

/// Where a request goes and how it is spoken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Api {
    /// Ollama's own `/api/chat` at `base`.
    Ollama,
    /// An OpenAI-compatible `/chat/completions` at `base` (LM Studio, llama.cpp on this machine).
    OpenAi { key: String },
}

/// The local model that looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModel {
    /// `gemma3:4b`.
    pub id: String,
    pub base: String,
    pub api: Api,
}

impl LocalModel {
    /// The same server and model: two jobs that use it wait for each other.
    pub fn same_as(&self, base: &str, id: &str) -> bool {
        self.id == id && ollama::clean_base(&self.base) == ollama::clean_base(base)
    }
}

/// "Local" means the model's provider is on this machine or this network: the address of its base URL is a
/// loopback, private (10/8, 172.16/12, 192.168/16, fc00::/7) or link-local IP literal, or the name
/// `localhost`. A host name that needs a lookup (`box.lan`, `ollama.com`) is not decided here and counts as NOT local:
/// a frame is never sent to a name nobody checked. The API type does not decide: Ollama can run on a server
/// the owner does not own. This one function is the whole rule.
pub fn is_local_url(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url.trim()) else { return false };
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
        Err(_) => host == "localhost" || host.ends_with(".localhost"),
    }
}

/// The model of the `visionModel` setting when its provider is local, else the recommended installed local vision model
/// (else any installed one). `Err` is the refusal of section 4, naming the page.
pub async fn local_model(providers: &ProviderStore, settings: &std::sync::RwLock<Settings>, lm: &LocalModels) -> Result<LocalModel, String> {
    let named = settings.read().unwrap().extra.get("visionModel").and_then(Value::as_str).unwrap_or("").to_string();
    if let Ok(r) = providers.resolve(&named) {
        if r.provider.enabled && r.model.enabled && is_local_url(&r.provider.api.base_url) {
            let api = if r.provider.api.api_type == ApiType::OllamaNative || r.provider.template_id == "ollama" { Api::Ollama } else { Api::OpenAi { key: r.provider.api_key.clone() } };
            let base = if api == Api::Ollama { ollama::clean_base(&r.provider.api.base_url) } else { r.provider.api.base_url.clone() };
            return Ok(LocalModel { id: r.model.id, base, api });
        }
    }
    let base = lm.base();
    let (installed, recommended) = lm.vision_picks().await;
    match installed.into_iter().next() {
        Some(id) if is_local_url(&base) => Ok(LocalModel { id, base, api: Api::Ollama }),
        _ => Err(match recommended {
            Some(id) => format!("Screen watch needs a local vision model: Settings -> Local models recommends {id} for this machine."),
            None => "Screen watch needs a local vision model: Settings -> Local models lists what this machine can run.".into(),
        }),
    }
}

fn client(secs: u64) -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(secs)).build().unwrap_or_default()
}

/// Ask the local model the look's question about `jpeg`: its raw reply.
pub async fn ask_local(model: &LocalModel, jpeg: &[u8], goal: &str) -> Result<String, String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
    let prompt = super::look::prompt(goal);
    match &model.api {
        Api::Ollama => {
            let body = json!({
                "model": model.id, "stream": false, "format": "json",
                "options": {"temperature": 0, "num_ctx": 4096, "num_predict": 160},
                "messages": [{"role": "user", "content": prompt, "images": [b64]}],
            });
            let v = post(&format!("{}/api/chat", ollama::clean_base(&model.base)), None, &body, 90).await?;
            Ok(v.pointer("/message/content").and_then(Value::as_str).unwrap_or("").to_string())
        }
        Api::OpenAi { key } => openai(&model.base, key, &model.id, &prompt, &b64, 90).await,
    }
}

/// Ask a cloud model the same question about the same frame.
pub async fn ask_cloud(base: &str, key: &str, model_id: &str, jpeg: &[u8], goal: &str) -> Result<String, String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(jpeg);
    openai(base, key, model_id, &super::look::prompt(goal), &b64, 60).await
}

async fn openai(base: &str, key: &str, model_id: &str, prompt: &str, b64: &str, secs: u64) -> Result<String, String> {
    let body = json!({
        "model": model_id.trim_start_matches("models/"), "max_tokens": 200, "temperature": 0,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": prompt},
            {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{b64}")}}
        ]}],
    });
    let v = post(&format!("{}/chat/completions", base.trim_end_matches('/')), Some(key), &body, secs).await?;
    Ok(v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("").to_string())
}

async fn post(url: &str, key: Option<&str>, body: &Value, secs: u64) -> Result<Value, String> {
    let mut req = client(secs).post(url).json(body);
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.map_err(|e| format!("the vision model did not answer: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!("the vision model answered {status}: {}", kagent_core::util::truncate_str(&v.to_string(), 200)));
    }
    Ok(v)
}

#[cfg(test)]
mod tests;
