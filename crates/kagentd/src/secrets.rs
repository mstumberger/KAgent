//! Provider API keys in the desktop keyring (Secret Service: KWallet on Plasma, GNOME
//! Keyring elsewhere). Keys are loaded into memory at startup; `providers.json` keeps
//! only `apiKeyRef: "secret-service"`. Without a keyring the 0600 file stays the store.

use std::collections::HashMap;
use std::sync::Arc;

use secret_service::{EncryptionType, SecretService};
use tokio::sync::mpsc;

use kagent_providers::{ProviderStore, SecretSink};

const APP: &str = "kagent";
const KIND: &str = "provider";

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

async fn put(ss: &SecretService<'_>, id: &str, key: &str) -> anyhow::Result<()> {
    let col = ss.get_default_collection().await?;
    col.unlock().await?;
    col.create_item(&format!("KAgent provider {id}"), attrs(id), key.as_bytes(), true, "text/plain").await?;
    Ok(())
}

async fn delete(ss: &SecretService<'_>, id: &str) -> anyhow::Result<()> {
    let col = ss.get_default_collection().await?;
    for item in col.search_items(attrs(id)).await? {
        item.delete().await?;
    }
    Ok(())
}

async fn get(ss: &SecretService<'_>, id: &str) -> anyhow::Result<Option<String>> {
    let col = ss.get_default_collection().await?;
    col.unlock().await?;
    match col.search_items(attrs(id)).await?.first() {
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
            match get(&ss, id).await {
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
                    Cmd::Put(id, key) => put(&ss, id, key).await,
                    Cmd::Delete(id) => delete(&ss, id).await,
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
