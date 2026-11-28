//! Recommended local models (local-models.md, S47): what this machine can run, a shipped
//! list per job with a fit verdict, and downloads through Ollama. KAgent installs nothing but
//! models, and only when the owner asks. `fit.rs` holds the pure rules, `ollama.rs` the calls
//! to Ollama, `http.rs` the routes and `tool.rs` the `local_models` tool.

pub mod fit;
pub mod http;
pub mod manage;
pub mod ollama;
pub mod pull;
pub mod tool;
pub mod vision;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_pull;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use kagent_core::platform::gpu::{self, Gpu};
use kagent_core::protocol::AgentEvent;
use kagent_providers::ProviderStore;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use self::fit::{Entry, Fit, Job};
use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PullState {
    Queued,
    Pulling,
    Failed,
}

/// One row of `pulls`: a download that is waiting, running or failed (a finished one is an
/// installed model; a cancelled one is gone).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRow {
    pub id: String,
    pub state: PullState,
    pub done_mb: u64,
    pub total_mb: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where the machine's numbers come from: the probes, or fixed values (tests).
pub enum Hardware {
    Probe,
    #[cfg_attr(not(test), allow(dead_code))] // built by the tests
    Fixed { gpu: Option<Gpu>, ram_total_mb: u64 },
}

/// Where the free disk space comes from: `df` on Ollama's model folder, or a fixed value (tests).
pub enum Disk {
    Probe,
    #[cfg_attr(not(test), allow(dead_code))] // built by the tests
    Fixed(Option<u64>),
}

/// The rows, whether a worker is going through them, and the way to stop the running one.
#[derive(Default)]
pub struct Pulls {
    pub rows: Vec<PullRow>,
    pub running: bool,
    pub cancel: HashMap<String, CancellationToken>,
}

pub struct LocalModels {
    pub(crate) providers: Arc<ProviderStore>,
    pub(crate) settings: Arc<RwLock<Settings>>,
    pub(crate) events: broadcast::Sender<AgentEvent>,
    pub(crate) list: Vec<Entry>,
    /// Ollama's address when it is not the provider's (tests).
    pub(crate) base_override: Option<String>,
    pub(crate) hardware: Hardware,
    pub(crate) disk: Disk,
    pub(crate) pulls: Mutex<Pulls>,
}

impl LocalModels {
    pub fn new(providers: Arc<ProviderStore>, settings: Arc<RwLock<Settings>>, events: broadcast::Sender<AgentEvent>) -> Arc<Self> {
        Arc::new(Self { providers, settings, events, list: fit::shipped(), base_override: None, hardware: Hardware::Probe, disk: Disk::Probe, pulls: Mutex::new(Pulls::default()) })
    }

    /// Ollama's address: the Ollama provider's, else the default.
    pub fn base(&self) -> String {
        if let Some(b) = &self.base_override {
            return b.clone();
        }
        match self.ollama_provider() {
            Some(p) => ollama::clean_base(&p.api.base_url),
            None => ollama::DEFAULT_BASE.into(),
        }
    }

    /// The owner's providers that speak Ollama's API, in their order.
    pub fn ollama_providers(&self) -> Vec<kagent_providers::config::ProviderEntry> {
        let file = self.providers.providers();
        file.order.iter().filter_map(|id| file.providers.get(id)).filter(|p| p.template_id == "ollama" || p.api.api_type == kagent_providers::ApiType::OllamaNative).cloned().collect()
    }

    /// The first of them, if any.
    pub fn ollama_provider(&self) -> Option<kagent_providers::config::ProviderEntry> {
        self.ollama_providers().into_iter().next()
    }

    async fn machine(&self) -> (Option<Gpu>, u64) {
        match &self.hardware {
            Hardware::Probe => (gpu::probe().await, gpu::ram_total_mb()),
            Hardware::Fixed { gpu, ram_total_mb } => (gpu.clone(), *ram_total_mb),
        }
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.list.iter().find(|e| e.id == id)
    }

    /// The settings that name a model of the list, `(setting, ref)`.
    pub(crate) fn named_as(&self, model_ref: &str) -> Vec<&'static str> {
        let s = self.settings.read().unwrap();
        crate::api::providers::named_models(&s).into_iter().filter(|(_, r)| r == model_ref).map(|(k, _)| k).collect()
    }

    /// `provider::id` of a model in the owner's Ollama provider, when it has a row for it.
    pub(crate) fn model_ref(&self, installed_id: &str) -> Option<String> {
        let p = self.ollama_provider()?;
        p.models.iter().any(|m| m.id == installed_id).then(|| format!("{}::{installed_id}", p.id))
    }

    pub fn pull_rows(&self) -> Vec<PullRow> {
        self.pulls.lock().unwrap().rows.clone()
    }

    pub(crate) fn emit_pulls(&self) {
        let _ = self.events.send(AgentEvent::LocalModelsChanged { pulls: json!(self.pull_rows()) });
    }

    /// `GET /api/local-models` (local-models.md section 1).
    pub async fn view(&self) -> Value {
        let (gpu, ram) = self.machine().await;
        let base = self.base();
        let (tags, version) = tokio::join!(ollama::tags(&base), ollama::version(&base));
        let reachable = tags.is_ok();
        let installed_all = tags.unwrap_or_default();
        let language = self.settings.read().unwrap().persona.language.clone();
        let fits: Vec<Fit> = self.list.iter().map(|e| fit::fit(e.vram_mb, gpu.as_ref(), ram)).collect();
        let picks = fit::recommend(&self.list, &fits, &language);

        let installed: Vec<Value> = installed_all
            .iter()
            .map(|i| {
                let listed = self.list.iter().find(|e| ollama::same_id(&e.id, &i.id));
                let r = self.model_ref(&i.id);
                json!({
                    "id": i.id, "sizeMb": i.size_mb, "jobs": listed.map(|e| e.jobs.clone()).unwrap_or_default(),
                    "listed": listed.is_some(), "ref": r, "usedFor": r.as_deref().map(|r| self.named_as(r)).unwrap_or_default(),
                })
            })
            .collect();

        let recommended: Vec<Value> = self
            .list
            .iter()
            .zip(&fits)
            .map(|(e, f)| {
                let have = installed_all.iter().find(|i| ollama::same_id(&e.id, &i.id));
                let r = have.and_then(|i| self.model_ref(&i.id));
                json!({
                    "id": e.id, "name": e.name, "jobs": e.jobs, "sizeMb": e.size_mb, "vramMb": e.vram_mb, "context": e.context,
                    "tools": e.tools, "languages": e.languages, "note": e.note, "engine": e.engine,
                    "downloadable": e.downloadable(), "fit": f, "fitNote": fit::in_use_note(e.vram_mb, gpu.as_ref()),
                    "recommendedFor": Job::ALL.iter().filter(|j| picks.get(j) == Some(&e.id)).collect::<Vec<_>>(),
                    "installed": have.is_some(), "installedId": have.map(|i| i.id.clone()),
                    "ref": r, "usedFor": r.as_deref().map(|r| self.named_as(r)).unwrap_or_default(),
                })
            })
            .collect();

        json!({
            "ollama": {"reachable": reachable, "version": version, "baseUrl": base},
            "gpu": gpu,
            "ramTotalMb": ram,
            "installed": installed,
            "recommended": recommended,
            "pulls": self.pull_rows(),
        })
    }
}
