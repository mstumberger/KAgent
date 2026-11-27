//! Provider API keys in the desktop keyring (Secret Service: KWallet on Plasma, GNOME
//! Keyring elsewhere). Keys are loaded into memory at startup; `providers.json` keeps
//! only `apiKeyRef: "secret-service"`. Without a keyring the 0600 file stays the store.
//! The Telegram bot token is kept the same way (`TokenStore`, bots.md section 1).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use secret_service::{EncryptionType, SecretService};
use tokio::sync::mpsc;

use kagent_providers::{ProviderStore, SecretSink};

const APP: &str = "kagent";
const KIND: &str = "provider";
/// The Telegram bot token is a kind of its own, apart from the provider keys.
const TELEGRAM_KIND: &str = "telegram";
const TELEGRAM_ID: &str = "bot";
const TELEGRAM_SECRET: &str = "telegram.secret";

enum Cmd {
    Put(String, String),
    Delete(String),
}

/// Non-blocking sink: forwards to a task that owns the D-Bus connection.
struct KeyringSink(mpsc::UnboundedSender<Cmd>);

impl SecretSink for KeyringSink {
    fn put(&self, id: &str, key: &str) {
        let _ = self.0.send(Cmd::Put(id.into(), key.into()));
    }
    fn delete(&self, id: &str) {
        let _ = self.0.send(Cmd::Delete(id.into()));
    }
}

fn attrs(id: &str) -> HashMap<&str, &str> {
    HashMap::from([("app", APP), ("kind", KIND), ("id", id)])
}

async fn put<'a>(ss: &SecretService<'_>, label: &str, attrs: HashMap<&'a str, &'a str>, key: &str) -> anyhow::Result<()> {
    let col = ss.get_default_collection().await?;
    col.unlock().await?;
    col.create_item(label, attrs, key.as_bytes(), true, "text/plain").await?;
    Ok(())
}

async fn delete<'a>(ss: &SecretService<'_>, attrs: HashMap<&'a str, &'a str>) -> anyhow::Result<()> {
    let col = ss.get_default_collection().await?;
    for item in col.search_items(attrs).await? {
        item.delete().await?;
    }
    Ok(())
}

async fn get<'a>(ss: &SecretService<'_>, attrs: HashMap<&'a str, &'a str>) -> anyhow::Result<Option<String>> {
    let col = ss.get_default_collection().await?;
    col.unlock().await?;
    match col.search_items(attrs).await?.first() {
        Some(item) => Ok(Some(String::from_utf8(item.get_secret().await?)?)),
        None => Ok(None),
    }
}

/// Connect, load keys of providers marked `secret-service`, install the sink and migrate
/// plain keys (the save moves them). Returns false when no keyring is available.
pub async fn attach(store: Arc<ProviderStore>) -> bool {
    // The keyring is shared by every KAgent on this login. A separate KAGENT_HOME (tests,
    // a second instance) must never read or overwrite the real keys: once a test daemon
    // stored its fake Gemini key over the owner's. Opt in with KAGENT_KEYRING=1.
    if std::env::var_os("KAGENT_HOME").is_some() && std::env::var_os("KAGENT_KEYRING").is_none() {
        tracing::info!("KAGENT_HOME is set: provider keys stay in this home's providers.json, the desktop keyring is not used");
        return false;
    }
    let ss = match tokio::time::timeout(std::time::Duration::from_secs(5), SecretService::connect(EncryptionType::Dh)).await {
        Ok(Ok(ss)) => ss,
        Ok(Err(e)) => {
            tracing::warn!("no Secret Service ({e}); provider keys stay in providers.json (0600)");
            return false;
        }
        Err(_) => {
            tracing::warn!("Secret Service did not answer; provider keys stay in providers.json (0600)");
            return false;
        }
    };
    let file = store.providers();
    for (id, p) in &file.providers {
        if p.api_key_ref.as_deref() == Some(kagent_providers::store::SECRET_SERVICE) && p.api_key.is_empty() {
            match get(&ss, attrs(id)).await {
                Ok(Some(k)) => store.restore_key(id, k),
                Ok(None) => tracing::warn!(provider = id.as_str(), "key missing from keyring; re-enter it in Providers"),
                Err(e) => tracing::warn!(provider = id.as_str(), "keyring read failed: {e}"),
            }
        }
    }
    drop(ss);

    let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            let res = match SecretService::connect(EncryptionType::Dh).await {
                Ok(ss) => match &cmd {
                    Cmd::Put(id, key) => put(&ss, &format!("KAgent provider {id}"), attrs(id), key).await,
                    Cmd::Delete(id) => delete(&ss, attrs(id)).await,
                },
                Err(e) => Err(e.into()),
            };
            if let Err(e) = res {
                tracing::warn!("keyring write failed: {e:#}");
            }
        }
    });
    store.set_secret_sink(Arc::new(KeyringSink(tx)));
    // Migrate plain keys now (save moves them into the keyring).
    let needs_migration = file.providers.values().any(|p| !p.api_key.is_empty() && p.api_key_ref.is_none());
    if needs_migration {
        match store.save() {
            Ok(()) => tracing::info!("moved provider API keys into the Secret Service"),
            Err(e) => tracing::warn!("key migration save failed: {e:#}"),
        }
    }
    true
}

/// Where one secret lives in the keyring and in the fallback file: the Telegram bot
/// token, the calendar's OAuth record and the Maps key are three of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    /// `kind` and `id` of the Secret Service item (`app=kagent`).
    pub kind: &'static str,
    pub id: &'static str,
    /// The label the keyring shows.
    pub label: &'static str,
    /// The 0600 file under `<config>` that holds it without a keyring.
    pub file: &'static str,
}

pub const TELEGRAM: Slot = Slot { kind: TELEGRAM_KIND, id: TELEGRAM_ID, label: "KAgent Telegram bot", file: TELEGRAM_SECRET };
/// The calendar's OAuth record (calendar.md section 1): client secret and refresh token, one JSON text.
pub const CALENDAR: Slot = Slot { kind: "calendar", id: "oauth", label: "KAgent Google Calendar", file: "calendar.json" };

/// Where a secret is kept (bots.md section 1, calendar.md section 1, config.md section 11).
pub enum TokenStore {
    /// The Secret Service, as `app=kagent, kind=<kind>, id=<id>`; the slot's file when none answers.
    Keyring(Slot),
    /// A file (0600).
    File(PathBuf),
}

impl TokenStore {
    /// The store of this daemon: the file under `KAGENT_HOME` (unless `KAGENT_KEYRING`
    /// is set, the rule of `attach`), else the keyring.
    pub fn telegram(config_dir: &Path) -> Self {
        Self::slot(config_dir, TELEGRAM)
    }

    /// The store of a slot, by the same rule.
    pub fn slot(config_dir: &Path, slot: Slot) -> Self {
        if std::env::var_os("KAGENT_HOME").is_some() && std::env::var_os("KAGENT_KEYRING").is_none() {
            Self::File(config_dir.join(slot.file))
        } else {
            Self::Keyring(slot)
        }
    }

    /// The token, or `None` when none is stored. A keyring that does not answer is not
    /// fatal: the file is read, where a write of such a moment left the token.
    pub async fn get(&self) -> anyhow::Result<Option<String>> {
        match self {
            Self::File(path) => Ok(read_secret(path)),
            Self::Keyring(slot) => match connect().await {
                Some(ss) => Ok(get(&ss, attrs_of(slot)).await?.or_else(|| read_secret(&secret_file(slot)))),
                None => {
                    tracing::warn!("no Secret Service for the {} secret; {} is read instead", slot.kind, secret_file(slot).display());
                    Ok(read_secret(&secret_file(slot)))
                }
            },
        }
    }

    /// Keep the token.
    pub async fn set(&self, token: &str) -> anyhow::Result<()> {
        match self {
            Self::File(path) => write_secret(path, token),
            Self::Keyring(slot) => match connect().await {
                Some(ss) => {
                    delete(&ss, attrs_of(slot)).await?;
                    put(&ss, slot.label, attrs_of(slot), token).await
                }
                None => {
                    tracing::warn!("no Secret Service for the {} secret; it is kept in {} (0600) instead", slot.kind, secret_file(slot).display());
                    write_secret(&secret_file(slot), token)
                }
            },
        }
    }

    /// Forget the token; forgetting none is not an error. The fallback file goes too.
    pub async fn clear(&self) -> anyhow::Result<()> {
        let keyring = match self {
            Self::File(path) => return remove_secret(path),
            Self::Keyring(slot) => match connect().await {
                Some(ss) => delete(&ss, attrs_of(slot)).await,
                None => {
                    tracing::warn!("no Secret Service: the {} secret may still be in the keyring", slot.kind);
                    Ok(())
                }
            },
        };
        match self {
            Self::Keyring(slot) => keyring.and_then(|_| remove_secret(&secret_file(slot))),
            Self::File(_) => keyring,
        }
    }
}

fn attrs_of(slot: &Slot) -> HashMap<&'static str, &'static str> {
    HashMap::from([("app", APP), ("kind", slot.kind), ("id", slot.id)])
}

/// The file a daemon without a keyring falls back to.
fn secret_file(slot: &Slot) -> PathBuf {
    crate::paths::Paths::resolve().config.join(slot.file)
}

fn read_secret(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// A file of 0600, private from its creation: a token is never readable by others, even
/// for the moment between its creation and a `chmod`.
fn write_secret(path: &Path, token: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(token.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn remove_secret(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The Secret Service, or `None` when none answers within 5 s.
async fn connect() -> Option<SecretService<'static>> {
    tokio::time::timeout(std::time::Duration::from_secs(5), SecretService::connect(EncryptionType::Dh)).await.ok()?.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_token_file_round_trips_private_and_forgets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join(TELEGRAM_SECRET);
        let store = TokenStore::File(path.clone());
        assert_eq!(store.get().await.unwrap(), None, "nothing stored yet");
        store.clear().await.unwrap();

        store.set("123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw").await.unwrap();
        assert_eq!(store.get().await.unwrap().as_deref(), Some("123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "only the owner may read the token");
        assert!(!path.with_extension("tmp").exists(), "written whole, in one step");

        // What a hand edit leaves: whitespace around the token is not part of it, an empty file is no token.
        std::fs::write(&path, "  42:abc\n").unwrap();
        assert_eq!(store.get().await.unwrap().as_deref(), Some("42:abc"));
        std::fs::write(&path, "\n").unwrap();
        assert_eq!(store.get().await.unwrap(), None);

        store.set("42:abc").await.unwrap();
        store.clear().await.unwrap();
        assert!(!path.exists());
        assert_eq!(store.get().await.unwrap(), None);
        store.clear().await.unwrap();
    }
}
