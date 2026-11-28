//! settings.search (web.md section 3, config.md section 11) and where the Brave Search
//! key is kept. The section type lives in the module that reads it, as Telegram's does.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use secret_service::{EncryptionType, SecretService};
use serde::{Deserialize, Serialize};

use super::guard;
use crate::secrets::TokenStore;
use crate::sections::SectionError;
use crate::settings::Settings;

/// settings.search: which engine `web_search` asks, and the owner's SearXNG instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchSection {
    pub engine: Engine,
    /// Base URL of a SearXNG instance, such as `https://searx.example.org`; empty = none.
    /// It may be on this machine or the local network: the owner sets it, never the model.
    pub searx_url: String,
    /// The ordered chain of engines (web.md section 3.1). Absent: derived from `engine`
    /// (`chain`). An empty list is "search off".
    pub engines: Option<Vec<Engine>>,
}

impl Default for SearchSection {
    fn default() -> Self {
        Self { engine: Engine::DuckDuckGo, searx_url: String::new(), engines: None }
    }
}

/// The engines of `web_search` (web.md section 3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    #[default]
    DuckDuckGo,
    Brave,
    Searxng,
    /// DuckDuckGo's result page read by the reader (web.md section 8); only in the chain.
    Reader,
    /// The tool answers that search is off.
    None,
}

impl Engine {
    pub fn name(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Brave => "Brave",
            Self::Searxng => "SearXNG",
            Self::Reader => "the reader (DuckDuckGo)",
            Self::None => "none",
        }
    }
}

impl SearchSection {
    /// The engines `web_search` tries, in order (web.md section 3.1): `engines` when set;
    /// else the old single `engine` and then the default fallbacks, without repeats.
    pub fn chain(&self) -> Vec<Engine> {
        if let Some(list) = &self.engines {
            return list.clone();
        }
        if self.engine == Engine::None {
            return Vec::new();
        }
        let mut chain = vec![self.engine];
        for fallback in [Engine::DuckDuckGo, Engine::Reader] {
            if !chain.contains(&fallback) {
                chain.push(fallback);
            }
        }
        chain
    }

    /// A `searxUrl` that is set but not an http(s) address makes the settings file
    /// invalid. Its address is not judged: the instance is the owner's.
    pub fn validate(&self) -> Result<(), SectionError> {
        if let Some(list) = &self.engines {
            let bad = |message: &str| SectionError::Invalid { key: "search".into(), message: message.into() };
            if list.contains(&Engine::None) {
                return Err(bad("engines lists duckduckgo, brave, searxng and reader; an empty list is search off"));
            }
            if list.iter().enumerate().any(|(n, e)| list[..n].contains(e)) {
                return Err(bad("engines names an engine twice"));
            }
        }
        if self.searx_url.trim().is_empty() {
            return Ok(());
        }
        guard::check_shape(&self.searx_url).map(|_| ()).map_err(|r| {
            SectionError::Invalid { key: "search".into(), message: format!("searxUrl must be an http or https address such as https://searx.example.org ({r})") }
        })
    }
}

impl Settings {
    /// settings.search.
    pub fn search_config(&self) -> SearchSection {
        self.section::<SearchSection>("search").unwrap_or_default()
    }
}

/// Where a secret is kept and how it is read and changed: the Brave key's store of this
/// daemon, or a store in memory in a test.
#[async_trait]
pub trait KeyStore: Send + Sync {
    /// The key, or `None` when none is stored.
    async fn get(&self) -> anyhow::Result<Option<String>>;
    /// Keep the key, in place of the one there was.
    async fn set(&self, key: &str) -> anyhow::Result<()>;
    /// Forget the key; forgetting none is not an error.
    async fn clear(&self) -> anyhow::Result<()>;
}

/// The file a daemon without a keyring falls back to, and the store of one under `KAGENT_HOME`.
const SEARCH_SECRET: &str = "search.secret";

/// Where the Brave Search key is kept (web.md section 3, config.md section 11).
pub enum BraveKeyStore {
    /// The Secret Service, as `app=kagent, kind=search, id=brave`; the file of the
    /// config directory when no keyring answers. Holds that directory.
    Keyring(PathBuf),
    File(TokenStore),
}

impl BraveKeyStore {
    /// The store of this home: the file under `KAGENT_HOME` (unless `KAGENT_KEYRING` is
    /// set, the rule of `secrets::attach`), else the keyring.
    pub fn for_config(config_dir: &Path) -> Self {
        if std::env::var_os("KAGENT_HOME").is_some() && std::env::var_os("KAGENT_KEYRING").is_none() {
            Self::File(file_store(config_dir))
        } else {
            Self::Keyring(config_dir.to_path_buf())
        }
    }
}

fn file_store(config_dir: &Path) -> TokenStore {
    TokenStore::File(config_dir.join(SEARCH_SECRET))
}

/// `secrets.rs` keeps its keyring calls to itself (and to the Telegram token), so these
/// are the same calls once more, for the attributes of the key.
fn attrs() -> HashMap<&'static str, &'static str> {
    HashMap::from([("app", "kagent"), ("kind", "search"), ("id", "brave")])
}

/// The Secret Service, or `None` when none answers within 5 s (the rule of `secrets.rs`).
async fn connect() -> Option<SecretService<'static>> {
    tokio::time::timeout(std::time::Duration::from_secs(5), SecretService::connect(EncryptionType::Dh)).await.ok()?.ok()
}

#[async_trait]
impl KeyStore for BraveKeyStore {
    /// The key. A keyring that does not answer is not fatal: the file is read, where a
    /// write of such a moment left the key.
    async fn get(&self) -> anyhow::Result<Option<String>> {
        match self {
            Self::File(store) => store.get().await,
            Self::Keyring(config) => match connect().await {
                Some(ss) => match read_keyring(&ss).await? {
                    Some(key) => Ok(Some(key)),
                    None => file_store(config).get().await,
                },
                None => {
                    tracing::warn!("no Secret Service for the Brave key; {} is read instead", config.join(SEARCH_SECRET).display());
                    file_store(config).get().await
                }
            },
        }
    }

    async fn set(&self, key: &str) -> anyhow::Result<()> {
        match self {
            Self::File(store) => store.set(key).await,
            Self::Keyring(config) => match connect().await {
                Some(ss) => {
                    let col = ss.get_default_collection().await?;
                    col.unlock().await?;
                    for item in col.search_items(attrs()).await? {
                        item.delete().await?;
                    }
                    col.create_item("KAgent Brave Search key", attrs(), key.as_bytes(), true, "text/plain").await?;
                    Ok(())
                }
                None => {
                    tracing::warn!("no Secret Service: the Brave key is kept in {} (0600) instead", config.join(SEARCH_SECRET).display());
                    file_store(config).set(key).await
                }
            },
        }
    }

    /// Forget the key; the fallback file goes too.
    async fn clear(&self) -> anyhow::Result<()> {
        match self {
            Self::File(store) => store.clear().await,
            Self::Keyring(config) => {
                match connect().await {
                    Some(ss) => {
                        let col = ss.get_default_collection().await?;
                        for item in col.search_items(attrs()).await? {
                            item.delete().await?;
                        }
                    }
                    None => tracing::warn!("no Secret Service: the Brave key may still be in the keyring"),
                }
                file_store(config).clear().await
            }
        }
    }
}

async fn read_keyring(ss: &SecretService<'_>) -> anyhow::Result<Option<String>> {
    let col = ss.get_default_collection().await?;
    col.unlock().await?;
    Ok(match col.search_items(attrs()).await?.first() {
        Some(item) => Some(String::from_utf8(item.get_secret().await?)?),
        None => None,
    })
}

/// The shape of a key the store takes: 16 to 128 printable ASCII characters and no
/// space. It stops a paste of something else; whether Brave takes the key is the
/// engine's answer at the first search.
pub fn check_key(key: &str) -> Result<(), String> {
    let length = key.chars().count();
    if (16..=128).contains(&length) && key.chars().all(|c| ('!'..='~').contains(&c)) {
        return Ok(());
    }
    Err(format!("the key must be 16 to 128 characters of ASCII text without spaces ({} given)", if length == 0 { "none".to_string() } else { length.to_string() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_section_has_the_defaults_of_the_spec_and_refuses_what_is_not_an_address() {
        let mut s = Settings::defaults();
        assert_eq!(s.search_config(), SearchSection::default(), "a missing section has the defaults");
        assert_eq!(serde_json::to_value(SearchSection::default()).unwrap(), json!({"engine": "duckduckgo", "searxUrl": "", "engines": null}), "config.md section 11");
        // The instance is the owner's: one on this machine is allowed.
        s.patch(json!({"search": {"engine": "searxng", "searxUrl": "http://127.0.0.1:8088/"}})).unwrap();
        assert_eq!(s.search_config(), SearchSection { engine: Engine::Searxng, searx_url: "http://127.0.0.1:8088/".into(), engines: None });
        for engine in ["brave", "searxng", "none", "duckduckgo"] {
            s.patch(json!({"search": {"engine": engine}})).unwrap();
        }
        assert_eq!(s.search_config().searx_url, "http://127.0.0.1:8088/", "a missing field takes what the file has");
        for url in ["ftp://searx.example.org/", "http://u:p@searx.example.org/", "searx", "https://", "file:///tmp/searx"] {
            let err = s.patch(json!({"search": {"searxUrl": url}})).unwrap_err().to_string();
            assert!(err.contains("invalid settings.search: searxUrl must be an http or https address"), "{url}: {err}");
        }
        assert!(s.patch(json!({"search": {"engine": "bing"}})).is_err(), "an engine that does not exist");
        assert!(s.patch(json!({"search": {"searxUrl": 8088}})).is_err(), "a wrong type is rejected");
        assert!(s.patch(json!({"search": "duckduckgo"})).is_err(), "the section must be an object");
        let typo = s.patch(json!({"search": {"engin": "brave"}})).unwrap_err();
        assert!(typo.contains("did you mean \"engine\"?"), "{typo}");
        assert!(s.patch(json!({"search": {"searxurl": "https://searx.example.org/"}})).unwrap_err().contains("did you mean \"searxUrl\"?"), "case does not hide a typo");
        assert_eq!(s.search_config(), SearchSection { engine: Engine::DuckDuckGo, searx_url: "http://127.0.0.1:8088/".into(), engines: None }, "nothing of a refused patch stays");

        // An invalid value is refused on load as well, like every typed section.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, json!({"search": {"searxUrl": "searx"}}).to_string()).unwrap();
        assert!(Settings::load(&path).unwrap_err().to_string().contains("searxUrl must be an http or https address"), "refused on load");
    }

    /// An old settings file with the single engine means a chain of that engine plus the fallbacks.
    #[test]
    fn the_old_single_engine_setting_is_a_chain_with_the_default_fallbacks() {
        let load = |search: serde_json::Value| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            std::fs::write(&path, json!({ "search": search }).to_string()).unwrap();
            Settings::load(&path).unwrap().search_config()
        };
        use Engine::*;
        assert_eq!(load(json!({"engine": "brave"})).chain(), [Brave, DuckDuckGo, Reader]);
        assert_eq!(load(json!({"engine": "searxng", "searxUrl": "http://127.0.0.1:8088/"})).chain(), [Searxng, DuckDuckGo, Reader]);
        assert_eq!(load(json!({"engine": "duckduckgo"})).chain(), [DuckDuckGo, Reader], "no repeats");
        assert_eq!(load(json!({"engine": "none"})).chain(), Vec::<Engine>::new(), "off is off");
        assert_eq!(load(json!({})).chain(), [DuckDuckGo, Reader], "no section at all");
        assert_eq!(load(json!({"engine": "brave", "engines": ["searxng", "reader"]})).chain(), [Searxng, Reader], "the list wins once it is set");
        assert_eq!(load(json!({"engines": []})).chain(), Vec::<Engine>::new(), "an empty list is off");
        let mut s = Settings::defaults();
        assert!(s.patch(json!({"search": {"engines": ["brave", "brave"]}})).unwrap_err().contains("twice"));
        assert!(s.patch(json!({"search": {"engines": ["none"]}})).is_err());
        assert!(s.patch(json!({"search": {"engines": ["bing"]}})).is_err());
        s.patch(json!({"search": {"engines": ["reader", "brave"]}})).unwrap();
        assert_eq!(s.search_config().chain(), [Reader, Brave]);
    }

    #[tokio::test]
    async fn the_brave_key_round_trips_private_and_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let store = BraveKeyStore::File(file_store(dir.path()));
        assert_eq!(store.get().await.unwrap(), None, "nothing stored yet");
        store.clear().await.unwrap();
        store.set("BSAtest-key_that_is_long_enough").await.unwrap();
        assert_eq!(store.get().await.unwrap().as_deref(), Some("BSAtest-key_that_is_long_enough"));
        let path = dir.path().join(SEARCH_SECRET);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "only the owner may read the key");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "BSAtest-key_that_is_long_enough", "the file is the key and nothing else");
        store.set("BSAtest-a_second_key_now_x").await.unwrap();
        assert_eq!(store.get().await.unwrap().as_deref(), Some("BSAtest-a_second_key_now_x"), "the new key replaces the old one");
        store.clear().await.unwrap();
        assert_eq!(store.get().await.unwrap(), None);
        assert!(!path.exists());
        store.clear().await.unwrap();
    }

    #[test]
    fn a_key_of_another_shape_is_refused() {
        assert!(check_key("BSAtest-key_that_is_long_enough").is_ok());
        let long = "x".repeat(129);
        for key in ["", "  ", "short", "has a space in it and is long", "töo-shört", long.as_str()] {
            assert!(check_key(key).is_err(), "{key:?}");
        }
        assert!(check_key(&"k".repeat(16)).is_ok() && check_key(&"k".repeat(128)).is_ok(), "both ends of the range");
        assert_eq!(check_key("short").unwrap_err(), "the key must be 16 to 128 characters of ASCII text without spaces (5 given)");
        assert_eq!(check_key("").unwrap_err(), "the key must be 16 to 128 characters of ASCII text without spaces (none given)");
    }
}
