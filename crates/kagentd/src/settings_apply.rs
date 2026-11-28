//! One way to change settings: the body of `PUT /api/settings`, shared with the
//! `settings` tool (docs/specs/voice-control.md section 1). Validates, applies the side
//! effects, saves, and sends `settingsChanged`.

use std::path::Path;
use std::sync::{Arc, RwLock};

use serde_json::Value;

use kagent_core::protocol::AgentEvent;
use kagent_providers::ProviderStore;

use crate::server::{bad_request, HttpError};
use crate::settings::Settings;
use crate::AppState;

mod notes;

/// The part of a change that needs no daemon: the role check of the models, the patch (all
/// or nothing) and the save. `locked` runs under the settings lock, after the patch and
/// before the save (the side effects that must see the new values). A patch that cannot be
/// saved is undone. Returns the stored settings.
pub fn patch_and_save(
    settings: &RwLock<Settings>,
    providers: &ProviderStore,
    path: &Path,
    patch: Value,
    locked: impl FnOnce(&Settings),
) -> Result<Settings, HttpError> {
    // A model must have the role of the job it is given (models.md section 4).
    crate::api::check_model_roles(providers, &patch)?;
    let wants_autostart = patch.get("startup").and_then(|v| v.get("autostart")).and_then(Value::as_bool);
    let mut s = settings.write().unwrap();
    let before = s.clone();
    s.patch(patch).map_err(bad_request)?;
    locked(&s);
    if let Err(e) = s.save(path) {
        *s = before;
        return Err(crate::api::internal(e));
    }
    if let Some(want) = wants_autostart {
        if want != s.startup.autostart {
            return Err(bad_request("startup.autostart patch did not apply"));
        }
    }
    Ok(s.clone())
}

/// Apply a settings patch all or nothing; returns the stored settings. The owner's change on the screen
/// (`PUT /api/settings`): the turns that run are told (agent-loop.md C.2).
pub async fn apply(st: &Arc<AppState>, patch: Value) -> Result<Value, HttpError> {
    let before = serde_json::to_value(&*st.settings.read().unwrap()).unwrap_or(Value::Null);
    let after = apply_quiet(st, patch).await?;
    if let Some(text) = notes::note_text(&before, &after) {
        st.agent.note_running(kagent_core::agent::notes::NoteKind::Setting, "settings", &text);
    }
    Ok(after)
}

/// The same change without a note: the `settings` tool's own (the model made it and has the result).
pub async fn apply_quiet(st: &Arc<AppState>, patch: Value) -> Result<Value, HttpError> {
    let wants_autostart = patch.get("startup").and_then(|v| v.get("autostart")).and_then(Value::as_bool);
    // A read-only calendar cannot be the default (calendar.md section 2): refused here, where the account can be asked.
    st.calendar.check_default(&patch).await.map_err(bad_request)?;
    let path = crate::paths::Paths::resolve().settings();
    patch_and_save(&st.settings, &st.providers, &path, patch, |s| {
        st.agent.permissions().set_mode(s.permission_mode);
        st.agent.permissions().set_rules(s.rules.clone());
        st.agent.permissions().set_overrides(crate::settings::parse_overrides(&s.tool_overrides));
        st.agent.set_tool_groups(Some(s.tool_groups.clone()));
        *st.services.desktop.write().unwrap() = s.desktop_prefs();
        if let Some(q) = s.extra.get("watchers").and_then(|w| w.get("quietHours")).and_then(Value::as_str) {
            st.alerts.set_quiet_hours(q);
        }
    })?;
    // At most `subagents.maxConcurrent` subagents run at once (agents.md section 2).
    let max_children = st.settings.read().unwrap().subagents_config().max_concurrent;
    st.agent.set_max_concurrent_children(max_children as usize).await;
    // At most `workflows.maxRuns` runs are active at once (workflows.md section 3.4).
    let max_runs = st.settings.read().unwrap().workflows_config().max_runs;
    st.runs.set_max_runs(max_runs);
    // Apply the systemd user unit change outside the settings lock (spawns
    // systemctl; slow paths must never hold the lock).
    if let Some(want) = wants_autostart {
        tokio::task::spawn_blocking(move || crate::startup::apply_requested(want));
    }
    let settings = st.settings.read().unwrap().clone();
    let _ = st.events.send(AgentEvent::SettingsChanged {
        settings: serde_json::to_value(&settings).unwrap_or(Value::Null),
    });
    Ok(serde_json::to_value(settings).unwrap_or(Value::Null))
}

/// The daemon as the `settings` tool sees it.
pub struct Host(pub Arc<AppState>);

#[async_trait::async_trait]
impl crate::settings_tool::SettingsHost for Host {
    fn settings(&self) -> Settings {
        self.0.settings.read().unwrap().clone()
    }
    fn providers(&self) -> &ProviderStore {
        &self.0.providers
    }
    async fn apply(&self, patch: Value) -> Result<(), String> {
        apply_quiet(&self.0, patch).await.map(|_| ()).map_err(|e| e.2)
    }
}
