//! The provider routes (api.md section 3, models.md section 4). The work is in
//! functions over the store, tested without a server (`tests.rs`): a second daemon
//! cannot run next to the owner's. The axum handlers wrap them.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use kagent_core::model::ModelClient as _;
use kagent_providers::store::{ProviderPatch, Refused};
use kagent_providers::{ApiType, ProviderStore};

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::settings::Settings;
use crate::AppState;

/// The four message formats, as `apiType` takes them.
const API_TYPES: [&str; 4] = ["anthropic-messages", "openai-chat-completions", "openai-responses", "ollama-native"];

/// The fields of a model's `config` the API takes; another key is a typo.
const CONFIG_FIELDS: [&str; 11] = [
    "enabled", "contextWindow", "maxOutputTokens", "supportsToolCall", "supportsImage", "reasoningLevels", "reasoningPatches", "supportsLive", "liveProtocol", "requestPatch",
    "roles",
];

/// The models the settings name, `(setting, model ref)`: what a removed model may
/// still hold, and what keeps its roles at a load.
pub fn named_models(s: &Settings) -> Vec<(&'static str, String)> {
    let vision = s.extra.get("visionModel").and_then(Value::as_str).unwrap_or("");
    [("chatModel", s.chat_model.as_str()), ("fastModel", s.fast_model.as_str()), ("fallbackModel", s.fallback_model.as_str()), ("visionModel", vision), ("liveModel", s.live_model.as_str())]
        .into_iter()
        .filter(|(_, r)| !r.is_empty())
        .map(|(key, r)| (key, r.to_string()))
        .collect()
}

fn as_named<'a>(named: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    named.iter().map(|(key, r)| (*key, r.as_str())).collect()
}

fn refused(e: Refused) -> HttpError {
    match e {
        Refused::Invalid(message) => bad_request(message),
        Refused::Conflict(message) => HttpError(StatusCode::CONFLICT, "conflict", message),
        Refused::UnknownProvider => not_found("unknown provider"),
        Refused::NotSaved(message) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", message),
    }
}

/// What the typed patch cannot say: a key of a model's `config` that is no field, and
/// a number that is not a whole one (the parser's message would not name the field).
fn check_config_shape(body: &Value) -> Result<(), HttpError> {
    for m in body.get("models").and_then(Value::as_array).into_iter().flatten() {
        let id = m.get("id").and_then(Value::as_str).unwrap_or("?");
        for (field, value) in m.get("config").and_then(Value::as_object).into_iter().flatten() {
            if !CONFIG_FIELDS.contains(&field.as_str()) {
                return Err(bad_request(format!("config.{field} of '{id}' is not a model setting")));
            }
            let number = matches!(field.as_str(), "contextWindow" | "maxOutputTokens");
            if number && !value.is_null() && !value.is_u64() {
                return Err(bad_request(format!("config.{field} of '{id}': a whole number above 0")));
            }
        }
    }
    Ok(())
}

/// The body of `POST /api/providers`: the template, its key, and for `custom` a name
/// and a base URL. `apiType` is one of the four formats (models.md section 4).
pub fn add(store: &ProviderStore, body: &Value) -> ApiResult<Value> {
    let template_id = body.get("templateId").and_then(Value::as_str).ok_or_else(|| bad_request("templateId required"))?;
    let text = |key: &str| body.get(key).and_then(Value::as_str).map(str::to_string);
    let api_type = match body.get("apiType") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value::<ApiType>(v.clone()).map_err(|_| bad_request(format!("apiType must be one of {}", API_TYPES.join(", "))))?),
    };
    let headers: BTreeMap<String, String> = body
        .get("headers")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();
    let p = store.add_from_template(template_id, text("name"), text("baseUrl"), text("apiKey"), headers, api_type).map_err(|e| HttpError(StatusCode::BAD_REQUEST, "invalid_input", format!("{e:#}")))?;
    Ok(store.view(&p))
}

/// The body of `PUT /api/providers/:id`: every key of models.md section 4, checked as
/// a whole by the store before anything is written.
pub fn update(store: &ProviderStore, settings: &Settings, id: &str, body: &Value) -> ApiResult<Value> {
    check_config_shape(body)?;
    let patch: ProviderPatch = serde_json::from_value(body.clone()).map_err(|e| bad_request(format!("{e}")))?;
    let p = store.patch(id, &patch, &as_named(&named_models(settings))).map_err(refused)?;
    Ok(store.view(&p))
}

/// `GET /api/models` (models.md section 4): `chat`, `fast` and `vision` list the
/// models with role `chat`, `live` those with role `live`. Enabled models only.
pub fn model_lists(store: &ProviderStore) -> Value {
    let mut out: BTreeMap<&str, Vec<Value>> = [("chat", Vec::new()), ("fast", Vec::new()), ("live", Vec::new()), ("vision", Vec::new())].into_iter().collect();
    for (pid, model, config) in store.enabled_models() {
        let view = || json!({"id": model.id, "name": model.name, "ref": format!("{pid}::{}", model.id), "config": config});
        if model.roles.iter().any(|r| r == "chat") {
            for role in ["chat", "fast", "vision"] {
                out.get_mut(role).expect("a list above").push(view());
            }
        }
        if model.roles.iter().any(|r| r == "live") {
            out.get_mut("live").expect("a list above").push(view());
        }
    }
    Value::Object(out.into_iter().map(|(k, v)| (k.to_string(), Value::Array(v))).collect())
}

/// `chatModelValid` of `GET /api/state`: true only when the chat model can be used
/// (models.md section 4).
pub fn chat_model_valid(store: &ProviderStore, chat_model: &str) -> bool {
    store.usable(chat_model).is_some()
}

/// The role check of `PUT /api/settings`: a live model cannot answer text turns, and
/// the reverse (as before); a model without role `chat` cannot become `chatModel`,
/// `fastModel`, `fallbackModel` or `visionModel` (models.md section 4). A reference
/// the providers do not know is not refused here.
pub fn check_model_roles(store: &ProviderStore, patch: &Value) -> Result<(), HttpError> {
    for key in ["chatModel", "fastModel", "fallbackModel", "visionModel", "liveModel"] {
        let Some(r) = patch.get(key).and_then(Value::as_str).filter(|r| !r.is_empty()) else { continue };
        let Ok(m) = store.resolve(r) else { continue };
        let live = m.config.supports_live || (!m.config.roles.is_empty() && m.config.roles.iter().all(|x| x == "live")) || m.model.id.contains("-live") || m.model.id.contains("native-audio");
        if key == "liveModel" {
            if !live {
                return Err(bad_request(format!("{r} is not a live voice model; pick a Gemini *-live model")));
            }
            continue;
        }
        if live {
            return Err(bad_request(format!("{r} is a live voice model and cannot answer text chat; pick a text model (e.g. a Gemini flash model) and set it as the Voice tab's live model instead")));
        }
        if !m.model.roles.iter().any(|x| x == "chat") {
            let roles = if m.model.roles.is_empty() { "no role".to_string() } else { m.model.roles.join(", ") };
            return Err(bad_request(format!("{r} is not a chat model (its roles: {roles}); give it the chat role in its model settings, or pick another")));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ handlers

pub async fn provider_templates(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(super::ok_json(json!({"templates": st.providers.templates()})))
}

pub async fn list_providers(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(super::ok_json(json!({"providers": st.providers.masked()})))
}

pub async fn add_provider(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let view = add(&st.providers, &body)?;
    Ok((StatusCode::CREATED, Json(view)).into_response())
}

pub async fn provider_order(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let order: Vec<String> = body
        .get("order")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .ok_or_else(|| bad_request("order array required"))?;
    st.providers.set_order(order).map_err(|e| HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}")))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn update_provider(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let view = update(&st.providers, &st.settings.read().unwrap(), &id, &body)?;
    Ok(super::ok_json(view))
}

pub async fn delete_provider(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let had_chat = st
        .settings
        .read()
        .unwrap()
        .chat_model
        .starts_with(&format!("{id}::"));
    st.providers.delete(&id).map_err(|e| not_found(format!("{e:#}")))?;
    if had_chat {
        let mut s = st.settings.write().unwrap();
        s.chat_model = String::new();
        s.save(&crate::paths::Paths::resolve().settings()).map_err(super::internal)?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn fetch_models(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let provider = st.providers.provider(&id).ok_or_else(|| not_found(format!("unknown provider {id}")))?;
    match kagent_providers::discovery::fetch_models(&provider).await {
        Ok(ids) => {
            let merged = kagent_providers::discovery::merge_models(&provider, ids);
            let updated = st.providers.update(&id, |p| {
                *p = merged;
                p.models_fetched_at = Some(chrono::Utc::now().to_rfc3339());
            });
            match updated {
                Ok(p) => Ok(super::ok_json(st.providers.view(&p))),
                Err(e) => Err(super::internal(e)),
            }
        }
        Err(e) => {
            st.providers.set_error(&id, &format!("{e:#}"));
            Err(HttpError(StatusCode::BAD_GATEWAY, "provider_error", format!("{e:#}")))
        }
    }
}

pub async fn test_provider(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let model_id = body.get("modelId").and_then(Value::as_str).map(str::to_string);
    let provider = st.providers.provider(&id).ok_or_else(|| not_found(format!("unknown provider {id}")))?;
    let mid = model_id
        .or_else(|| provider.models.first().map(|m| m.id.clone()))
        .ok_or_else(|| bad_request("provider has no models; pass modelId"))?;
    let start = std::time::Instant::now();
    let client = kagent_providers::ProvidersClient::new(st.providers.clone());
    let cancel = CancellationToken::new();
    let request = kagent_core::model::ModelRequest {
        model_ref: format!("{id}::{mid}"),
        system: String::new(),
        messages: vec![kagent_core::protocol::Message::user("Reply with exactly: OK")],
        tools: vec![],
        options: Default::default(),
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), client.complete(request, tx, cancel)).await;
    let latency = start.elapsed().as_millis() as u64;
    let body = match result {
        Ok(Ok(r)) => json!({"ok": true, "latencyMs": latency, "reply": r.text}),
        Ok(Err(e)) => {
            st.providers.set_error(&id, &format!("{e:#}"));
            json!({"ok": false, "latencyMs": latency, "error": format!("{e:#}")})
        }
        Err(_) => json!({"ok": false, "latencyMs": latency, "error": "timed out after 30 s"}),
    };
    Ok(super::ok_json(body))
}

pub async fn list_models(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(super::ok_json(model_lists(&st.providers)))
}
