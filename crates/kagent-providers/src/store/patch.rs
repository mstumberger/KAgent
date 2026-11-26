//! The keys of `PUT /api/providers/:id` (models.md section 4): one plan over a copy
//! of the entry, every check before anything is written. The model keys are checked in
//! the order of the spec: `addModels`, `models`, `removeModels`, `modelOrder`.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use crate::config::{ApiType, BuiltinCatalog, ModelConfigPatch, ModelEntry, ProviderEntry};
use crate::discovery::{roles_from_id, same_model};
use crate::rules::resolve;

use super::Named;

/// Why a patch was refused. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// 400 `invalid_input`; the message names the field.
    Invalid(String),
    /// 409 `conflict`: a model that a setting names.
    Conflict(String),
    /// 404: there is no provider with that id.
    UnknownProvider,
    /// 500: the file could not be written.
    NotSaved(String),
}

/// The body of `PUT /api/providers/:id`. A key left out keeps its value.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPatch {
    pub name: Option<String>,
    pub base_url: Option<String>,
    /// A string replaces the key ("" keeps the stored one), `null` clears it.
    #[serde(default, deserialize_with = "key_option")]
    pub api_key: Option<Option<String>>,
    pub headers: Option<BTreeMap<String, String>>,
    pub enabled: Option<bool>,
    pub api_type: Option<ApiType>,
    #[serde(default)]
    pub add_models: Vec<ModelAdd>,
    #[serde(default)]
    pub models: Vec<ModelChange>,
    #[serde(default)]
    pub remove_models: Vec<String>,
    pub model_order: Option<Vec<String>>,
}

fn key_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::<String>::deserialize(d)?))
}

/// One new row of `addModels`: disabled, smart, with the roles of its id.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelAdd {
    pub id: String,
}

/// One changed row of `models`; `config` replaces the patch, `roles` the roles.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelChange {
    pub id: String,
    pub enabled: Option<bool>,
    pub roles: Option<Vec<String>>,
    pub smart: Option<bool>,
    pub config: Option<ModelConfigPatch>,
}

/// The fields a manual model holds in `config` (models.md section 2).
const MANUAL_FIELDS: [(&str, fn(&ModelConfigPatch) -> bool); 5] = [
    ("contextWindow", |c| c.context_window.is_none()),
    ("maxOutputTokens", |c| c.max_output_tokens.is_none()),
    ("reasoningLevels", |c| c.reasoning_levels.is_none()),
    ("supportsToolCall", |c| c.supports_tool_call.is_none()),
    ("supportsImage", |c| c.supports_image.is_none()),
];

/// The first entry of a list that comes twice.
fn repeated<'a, T: PartialEq>(items: &'a [T]) -> Option<&'a T> {
    items.iter().enumerate().find(|(i, x)| items[..*i].contains(x)).map(|(_, x)| x)
}

/// The entry as `patch` leaves it, or why the patch is refused.
pub(super) fn plan(catalog: &BuiltinCatalog, entry: &ProviderEntry, patch: &ProviderPatch, named: &Named<'_>) -> Result<ProviderEntry, Refused> {
    let mut out = entry.clone();
    if let Some(t) = patch.api_type {
        out.api.api_type = t;
    }
    if let Some(name) = &patch.name {
        out.name = name.clone();
    }
    if let Some(base) = &patch.base_url {
        out.api.base_url = base.clone();
    }
    match &patch.api_key {
        Some(Some(key)) if !key.is_empty() => out.api_key = key.clone(),
        Some(None) => out.api_key = String::new(),
        _ => {}
    }
    if let Some(h) = &patch.headers {
        out.api.headers = h.clone();
    }
    if let Some(e) = patch.enabled {
        out.enabled = e;
    }
    add_models(&mut out, &patch.add_models)?;
    for change in &patch.models {
        change_model(catalog, &mut out, change)?;
    }
    remove_models(&mut out, &patch.remove_models, named)?;
    if let Some(order) = &patch.model_order {
        reorder(&mut out, order)?;
    }
    Ok(out)
}

fn add_models(out: &mut ProviderEntry, adds: &[ModelAdd]) -> Result<(), Refused> {
    for add in adds {
        let id = &add.id;
        if id.is_empty() || id.chars().count() > 200 || id.chars().any(char::is_whitespace) {
            return Err(Refused::Invalid("addModels: an id is 1 to 200 characters without whitespace".into()));
        }
        if let Some(row) = out.models.iter().find(|m| same_model(&m.id, id)) {
            let twin = if row.id == *id { String::new() } else { format!(" (as '{}')", row.id) };
            return Err(Refused::Invalid(format!("addModels: model '{id}' already exists{twin}")));
        }
        let roles = roles_from_id(id);
        out.models.push(ModelEntry { id: id.clone(), name: String::new(), enabled: false, roles, smart: true, config: ModelConfigPatch::default() });
    }
    Ok(())
}

fn change_model(catalog: &BuiltinCatalog, provider: &mut ProviderEntry, change: &ModelChange) -> Result<(), Refused> {
    let before = provider
        .models
        .iter()
        .find(|m| m.id == change.id)
        .cloned()
        .ok_or_else(|| Refused::Invalid(format!("models: unknown model '{}'", change.id)))?;
    let mut after = before.clone();
    if let Some(e) = change.enabled {
        after.enabled = e;
    }
    if let Some(s) = change.smart {
        after.smart = s;
    }
    // The patch the row ends with: the one of the request, else the stored one.
    let config = change.config.as_ref().unwrap_or(&before.config);
    let field = |what: &str| format!("config.{what} of '{}'", change.id);
    if config.context_window == Some(0) {
        return Err(Refused::Invalid(format!("{}: a whole number above 0", field("contextWindow"))));
    }
    if config.max_output_tokens == Some(0) {
        return Err(Refused::Invalid(format!("{}: a whole number above 0", field("maxOutputTokens"))));
    }
    if let Some(levels) = &config.reasoning_levels {
        if levels.iter().any(|l| l.trim().is_empty()) {
            return Err(Refused::Invalid(format!("{}: no blank entry", field("reasoningLevels"))));
        }
        if let Some(repeated) = repeated(levels) {
            return Err(Refused::Invalid(format!("{}: '{repeated}' is a repeat", field("reasoningLevels"))));
        }
    }
    if change.config.is_some() || change.smart.is_some() {
        if let Some(patches) = &config.reasoning_patches {
            let effective = resolve(catalog, &after.id, provider.api.api_type.as_str(), &provider.api.base_url, &provider.template_id, config, after.smart);
            if let Some(key) = patches.keys().find(|k| !effective.reasoning_levels.contains(k)) {
                let levels = effective.reasoning_levels.join(", ");
                return Err(Refused::Invalid(format!("{}: '{key}' is not one of the reasoning levels ({levels})", field("reasoningPatches"))));
            }
            if patches.values().any(|v| !v.is_object()) {
                return Err(Refused::Invalid(format!("{}: every value is an object", field("reasoningPatches"))));
            }
        }
    }
    if let Some(roles) = &change.roles {
        if let Some(bad) = roles.iter().find(|r| r.as_str() != "chat" && r.as_str() != "live") {
            return Err(Refused::Invalid(format!("models: '{}': role '{bad}' is not one of chat, live", change.id)));
        }
        if let Some(repeated) = repeated(roles) {
            return Err(Refused::Invalid(format!("models: '{}': role '{repeated}' is repeated", change.id)));
        }
        after.roles = roles.clone();
    }
    // A manual model holds the six editable fields, `reasoningPatches` excepted.
    if (change.smart.is_some() || change.config.is_some()) && !after.smart {
        if let Some((name, _)) = MANUAL_FIELDS.iter().find(|(_, missing)| missing(config)) {
            return Err(Refused::Invalid(format!("{}: required while smart is false", field(name))));
        }
    }
    if let Some(config) = &change.config {
        after.config = config.clone();
    }
    *provider.models.iter_mut().find(|m| m.id == change.id).expect("found above") = after;
    Ok(())
}

fn remove_models(out: &mut ProviderEntry, ids: &[String], named: &Named<'_>) -> Result<(), Refused> {
    for id in ids {
        if !out.models.iter().any(|m| m.id == *id) {
            return Err(Refused::Invalid(format!("removeModels: unknown model '{id}'")));
        }
        let reference = format!("{}::{id}", out.id);
        if let Some((setting, _)) = named.iter().find(|(_, r)| *r == reference) {
            return Err(Refused::Conflict(format!("removeModels: '{reference}' is the {setting}")));
        }
        out.models.retain(|m| m.id != *id);
    }
    Ok(())
}

fn reorder(out: &mut ProviderEntry, order: &[String]) -> Result<(), Refused> {
    let once = |id: &String| order.iter().filter(|o| *o == id).count() == 1;
    let every = out.models.len() == order.len() && out.models.iter().all(|m| once(&m.id));
    if !every {
        return Err(Refused::Invalid("modelOrder: name every model exactly once".into()));
    }
    let position = |id: &str| order.iter().position(|o| o == id).expect("checked above");
    out.models.sort_by_key(|m| position(&m.id));
    Ok(())
}
