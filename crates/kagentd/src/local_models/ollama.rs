//! The few calls to Ollama (local-models.md): its version, its installed models, and
//! (`pull.rs`) the pull stream. KAgent never installs Ollama; an unreachable one is a state.

use std::time::Duration;

use serde::Deserialize;

pub const DEFAULT_BASE: &str = "http://127.0.0.1:11434";

/// The address of Ollama's own API from a provider's base URL (which may end in `/v1`).
pub fn clean_base(base: &str) -> String {
    let b = base.trim().trim_end_matches('/').trim_end_matches("/v1").trim_end_matches("/api").trim_end_matches('/');
    if b.is_empty() {
        DEFAULT_BASE.into()
    } else {
        b.into()
    }
}

/// One installed model of `GET /api/tags`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub size_mb: u64,
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    #[serde(default)]
    size: u64,
}

#[derive(Deserialize)]
struct Version {
    version: String,
}

pub fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder().timeout(timeout).build().unwrap_or_default()
}

/// `GET /api/tags`.
pub async fn tags(base: &str) -> anyhow::Result<Vec<Installed>> {
    let t: Tags = client(Duration::from_secs(4)).get(format!("{base}/api/tags")).send().await?.error_for_status()?.json().await?;
    Ok(t.models.into_iter().map(|m| Installed { id: m.name, size_mb: m.size / (1024 * 1024) }).collect())
}

/// `GET /api/version`.
pub async fn version(base: &str) -> Option<String> {
    let r = client(Duration::from_secs(4)).get(format!("{base}/api/version")).send().await.ok()?.error_for_status().ok()?;
    r.json::<Version>().await.ok().map(|v| v.version)
}

/// A listed id and an installed name are one model: equal, or the listed id has no tag and
/// Ollama's is `:latest` (`nomic-embed-text` is installed as `nomic-embed-text:latest`).
pub fn same_id(listed: &str, installed: &str) -> bool {
    listed == installed || (!listed.contains(':') && installed == format!("{listed}:latest"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases() {
        assert_eq!(clean_base("http://127.0.0.1:11434/v1"), "http://127.0.0.1:11434");
        assert_eq!(clean_base("http://127.0.0.1:11434/"), "http://127.0.0.1:11434");
        assert_eq!(clean_base("http://box:11434/api"), "http://box:11434");
        assert_eq!(clean_base(""), DEFAULT_BASE);
    }

    #[test]
    fn ids() {
        assert!(same_id("gemma3:4b", "gemma3:4b"));
        assert!(same_id("nomic-embed-text", "nomic-embed-text:latest"));
        assert!(!same_id("gemma3:4b", "gemma3:12b"));
        assert!(!same_id("gemma3:4b", "gemma3:4b:latest"));
        assert!(!same_id("nomic-embed-text", "nomic-embed-text-v2:latest"));
    }
}
