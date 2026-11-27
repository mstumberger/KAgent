//! The `settings` tool (voice-control.md, tools.md section 19a): the agent reads the
//! settings and the models, and changes a setting through the one function the route
//! uses (`settings_apply`). Keys that decide what the agent may do are protected: they
//! are changed on the screen only.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use kagent_providers::ProviderStore;
use serde_json::{json, Map, Value};

use crate::settings::Settings;

mod keys;
use keys::{full_view, is_protected, is_secret, leaves, nearest, path_get, patch_of};

const DESCRIPTION: &str = "Read or change KAgent's own settings, models and speaking voice. get: all settings (or one `key`, a dotted path such as voice.brain); secrets are masked. models: the providers, their enabled models with the job each holds (chatModel = the Brain, fastModel, fallbackModel, visionModel, liveModel) and the voices of the live model. set: {changes: {<dotted key>: <value>, ...}} changes 1 to 8 settings at once, all or none; the owner is asked first. Examples: {\"action\":\"set\",\"changes\":{\"chatModel\":\"nvidia::meta/llama-3.3-70b-instruct\"}}, {\"changes\":{\"liveVoice\":\"Puck\"}}, {\"changes\":{\"voice.volume\":60}}. Safety settings (permission mode, rules, tool groups, privilege, hooks, Telegram, secrets) are changed on the screen only. Say a setting changed only after the result says so.";

/// What the tool needs of the daemon: the settings, the providers and the one change path.
#[async_trait]
pub trait SettingsHost: Send + Sync {
    fn settings(&self) -> Settings;
    fn providers(&self) -> &ProviderStore;
    /// Apply a patch (all or nothing), as `PUT /api/settings` does.
    async fn apply(&self, patch: Value) -> Result<(), String>;
}

/// The cell the daemon fills once its state exists (the tool is registered before that).
pub type HostCell = Arc<OnceLock<Arc<dyn SettingsHost>>>;

pub struct SettingsTool {
    contract: ToolContract,
    host: HostCell,
}

impl SettingsTool {
    pub fn new(host: HostCell) -> Self {
        let schema = json!({"type":"object","required":["action"],"properties":{
            "action":{"type":"string","enum":["get","models","set"]},
            "key":{"type":"string","description":"get: one setting by its dotted path (voice.brain); left out = all"},
            "changes":{"type":"object","description":"set: {dotted key: new value}, 1 to 8 keys; a model is `<providerId>::<modelId>` (see models)"}
        }});
        let mut contract = ToolContract::read_only("settings", DESCRIPTION, schema).mutating(SideEffect::System, Risk::Medium);
        contract.always_ask = true;
        Self { contract, host }
    }

    fn host(&self) -> anyhow::Result<&Arc<dyn SettingsHost>> {
        self.host.get().ok_or_else(|| anyhow::anyhow!("settings are not ready"))
    }
}

/// A value as said aloud: text as it is, nothing as "empty", a model by its id.
fn say(key: &str, v: &Value) -> String {
    match v {
        Value::Null => "empty".into(),
        Value::String(s) if s.is_empty() => "empty".into(),
        Value::String(s) if key.ends_with("Model") => {
            let id = s.split_once("::").map(|(_, id)| id).unwrap_or(s);
            id.strip_prefix("models/").unwrap_or(id).to_string()
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `change setting <key> from <old> to <new>`, from the stored value and the asked one.
fn summary(view: &Value, changes: &Map<String, Value>) -> String {
    let parts: Vec<String> = changes
        .iter()
        .map(|(k, new)| format!("change setting {k} from {} to {}", say(k, path_get(view, k).unwrap_or(&Value::Null)), say(k, new)))
        .collect();
    parts.join("; ")
}

/// The keys of `changes` and what is wrong with them, or the changes.
fn check_changes<'a>(input: &'a Value, view: &Value) -> Result<&'a Map<String, Value>, String> {
    let changes = input.get("changes").and_then(Value::as_object).ok_or("set needs `changes`: an object of dotted keys and new values")?;
    if changes.is_empty() || changes.len() > 8 {
        return Err(format!("set takes 1 to 8 keys, not {}", changes.len()));
    }
    let known = leaves(view);
    for key in changes.keys() {
        if is_protected(key) {
            return Err(format!("{key}: this setting is changed on the screen only"));
        }
        if path_get(view, key).is_none() {
            let near = nearest(key, &known, 3);
            return Err(format!("unknown setting '{key}'; the nearest are: {}", near.join(", ")));
        }
    }
    Ok(changes)
}

fn voices_of(providers: &ProviderStore, settings: &Settings) -> Vec<String> {
    let Ok(m) = providers.resolve(&settings.live_model) else { return Vec::new() };
    providers.templates().iter().find(|t| t.id == m.provider.template_id).map(|t| t.voices.clone()).unwrap_or_default()
}

/// The template's spelling of `voice`, or the error that lists the voices.
fn check_voice(voices: &[String], voice: &str) -> Result<String, String> {
    if voices.is_empty() {
        return Ok(voice.to_string());
    }
    voices.iter().find(|v| v.eq_ignore_ascii_case(voice.trim())).cloned().ok_or_else(|| format!("unknown voice '{voice}'; the voices are: {}", voices.join(", ")))
}

fn models(host: &dyn SettingsHost) -> Value {
    let s = host.settings();
    let vision = s.extra.get("visionModel").and_then(Value::as_str).unwrap_or("").to_string();
    let jobs = [("chatModel", &s.chat_model), ("fastModel", &s.fast_model), ("fallbackModel", &s.fallback_model), ("visionModel", &vision), ("liveModel", &s.live_model)];
    let file = host.providers().providers();
    let providers: Vec<Value> = file
        .order
        .iter()
        .filter_map(|id| file.providers.get(id))
        .map(|p| {
            let models: Vec<Value> = p
                .models
                .iter()
                .filter(|m| m.enabled)
                .map(|m| {
                    let r = format!("{}::{}", p.id, m.id);
                    let held: Vec<&str> = jobs.iter().filter(|(_, v)| **v == r).map(|(k, _)| *k).collect();
                    json!({"ref": r, "name": m.name, "roles": m.roles, "jobs": held})
                })
                .collect();
            json!({"id": p.id, "name": p.name, "enabled": p.enabled, "models": models})
        })
        .collect();
    json!({"providers": providers, "voices": voices_of(host.providers(), &s)})
}

#[async_trait]
impl Tool for SettingsTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let plain = |summary: String| CallAssessment::from_contract(&self.contract, summary, "settings".into());
        if action != "set" {
            return plain(format!("settings {action}")).as_read_only();
        }
        let Ok(host) = self.host() else { return plain("settings set".into()) };
        let view = full_view(&host.settings());
        match check_changes(input, &view) {
            Ok(changes) => plain(summary(&view, changes)),
            // A refused change is no change: the call says why, without a card.
            Err(_) => plain("settings set (refused)".into()).as_read_only(),
        }
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let host = self.host()?.clone();
        match input.get("action").and_then(Value::as_str).unwrap_or("") {
            "get" => {
                let settings = host.settings();
                let view = full_view(&settings);
                let Some(key) = input.get("key").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()) else {
                    return Ok(ToolOutput::json(&mask(&view, "")));
                };
                match path_get(&view, key) {
                    Some(v) => Ok(ToolOutput::json(&mask(v, key))),
                    None => Ok(ToolOutput::error(format!("unknown setting '{key}'; the nearest are: {}", nearest(key, &leaves(&view), 3).join(", ")))),
                }
            }
            "models" => Ok(ToolOutput::json(&models(host.as_ref()))),
            "set" => {
                let before = host.settings();
                let view = full_view(&before);
                let changes = match check_changes(&input, &view) {
                    Ok(c) => c.clone(),
                    Err(e) => return Ok(ToolOutput::error(e)),
                };
                let mut changes = changes;
                if let Some(v) = changes.get("liveVoice") {
                    match check_voice(&voices_of(host.providers(), &before), v.as_str().unwrap_or("")) {
                        Ok(spelled) => {
                            changes.insert("liveVoice".into(), Value::String(spelled));
                        }
                        Err(e) => return Ok(ToolOutput::error(e)),
                    }
                }
                if let Err(e) = host.apply(patch_of(&changes)).await {
                    return Ok(ToolOutput::error(format!("nothing was changed: {e}")));
                }
                let after = full_view(&host.settings());
                let rows: Vec<Value> = changes.keys().map(|k| json!({"key": k, "old": path_get(&view, k), "new": path_get(&after, k)})).collect();
                let mut notes = Vec::new();
                if changes.contains_key("liveVoice") {
                    notes.push("the voice changes after this sentence");
                }
                if changes.keys().any(|k| k == "voice.brain") {
                    notes.push("voice.brain applies to the next voice session");
                }
                Ok(ToolOutput::json(&json!({"changed": rows, "notes": notes})))
            }
            other => Ok(ToolOutput::error(format!("unknown action '{other}': get, models or set"))),
        }
    }
}

/// `value` with every secret replaced by `"<set>"` or `"<empty>"`.
fn mask(value: &Value, path: &str) -> Value {
    match value {
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), mask(v, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }))).collect()),
        v if is_secret(path) => Value::String(if matches!(v, Value::Null) || v.as_str() == Some("") { "<empty>" } else { "<set>" }.into()),
        v => v.clone(),
    }
}

#[cfg(test)]
mod tests;
