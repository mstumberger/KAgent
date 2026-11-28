//! What happens to a model in the owner's providers: a finished download is added to the Ollama
//! provider (Fetch models, then its roles), and a removal takes it away again (local-models.md
//! section 3).

use std::time::Duration;

use kagent_providers::config::{ModelEntry, ProviderEntry};

use super::fit::{Entry, Job};
use super::pull::{Outcome, Refusal};
use super::{ollama, LocalModels};

/// What the row of a downloaded model starts with: on, the roles of its jobs (vision and chat
/// are the chat role, vision with images; an embedding has no role), and what the list knows.
pub fn arrive(row: &mut ModelEntry, entry: &Entry) {
    row.enabled = true;
    let chat = entry.has(Job::Chat) || entry.has(Job::Vision);
    row.roles = if chat { vec!["chat".into()] } else { Vec::new() };
    if chat {
        row.config.supports_image = Some(entry.has(Job::Vision));
        row.config.supports_tool_call = Some(entry.tools);
        row.config.context_window = Some(entry.context);
    }
}

impl LocalModels {
    /// The Ollama provider, added from its template when the owner has none yet.
    pub(super) fn ensure_provider(&self) -> Result<ProviderEntry, String> {
        if let Some(p) = self.ollama_provider() {
            return Ok(p);
        }
        let base = self.base();
        self.providers.add_from_template("ollama", None, Some(base), None, Default::default(), None).map_err(|e| format!("{e:#}"))
    }

    /// After a download: Fetch models on the Ollama provider, then the new row is switched on
    /// with the roles of its jobs. It is not made the Brain or anything else.
    pub(super) async fn register(&self, id: &str) -> Result<(), String> {
        let entry = self.entry(id).ok_or_else(|| format!("{id} is not on the list"))?.clone();
        let provider = self.ensure_provider()?;
        let before: Vec<String> = provider.models.iter().map(|m| m.id.clone()).collect();
        let after = crate::api::providers::refresh_models(&self.providers, &provider.id).await.map_err(|e| e.2)?;
        let Some(found) = after.models.iter().find(|m| ollama::same_id(id, &m.id)).map(|m| m.id.clone()) else {
            return Err(format!("Ollama does not list {id} after the download"));
        };
        let is_new = !before.contains(&found);
        self.providers
            .update(&provider.id, |p| {
                if let Some(row) = p.models.iter_mut().find(|m| m.id == found) {
                    if is_new {
                        arrive(row, &entry);
                    } else {
                        row.enabled = true;
                    }
                }
            })
            .map_err(|e| format!("{e:#}"))?;
        Ok(())
    }

    /// `DELETE /api/local-models/:id`: an installed model of the list, refused while a setting
    /// names it.
    pub async fn remove(&self, id: &str) -> Outcome<()> {
        let entry = self.entry(id).ok_or_else(|| Refusal::Invalid(format!("{id} is not on the list of recommended models: remove other models in Ollama itself")))?.clone();
        if !entry.downloadable() {
            return Err(Refusal::Invalid(format!("{} is not installed by KAgent", entry.name)));
        }
        let base = self.base();
        let installed = ollama::tags(&base).await.map_err(|_| Refusal::Conflict(format!("Ollama is not reachable at {base}: start it, then try again")))?;
        let Some(have) = installed.into_iter().find(|i| ollama::same_id(id, &i.id)) else {
            return Err(Refusal::Missing(format!("{id} is not installed")));
        };
        let used: Vec<String> = self
            .ollama_providers()
            .iter()
            .flat_map(|p| self.named_as(&format!("{}::{}", p.id, have.id)))
            .map(String::from)
            .collect();
        if !used.is_empty() {
            return Err(Refusal::Conflict(format!("{id} is still set as {} in Settings: choose another model for that first, then remove it", used.join(", "))));
        }
        let http = ollama::client(Duration::from_secs(60));
        let resp = http.delete(format!("{base}/api/delete")).json(&serde_json::json!({"model": have.id})).send().await.map_err(|e| Refusal::Upstream(format!("Ollama did not answer: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            return Err(Refusal::Upstream(format!("Ollama refused to remove {id} ({status}): {}", resp.text().await.unwrap_or_default().trim())));
        }
        // The model is gone from the machine: its row goes from the providers too.
        for p in self.ollama_providers() {
            let _ = self.providers.update(&p.id, |p| p.models.retain(|m| m.id != have.id));
        }
        self.emit_pulls();
        Ok(())
    }
}
