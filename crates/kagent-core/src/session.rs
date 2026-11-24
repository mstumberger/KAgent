//! Sessions persisted as `<dir>/<id>.jsonl` (one message per line) plus
//! `<dir>/<id>.meta.json`. The JSONL doubles as the audit trail of every tool call.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::protocol::Message;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub message_count: usize,
}

#[derive(Clone)]
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn log_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", sanitize(id)))
    }
    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.meta.json", sanitize(id)))
    }

    pub async fn create(&self, title: &str) -> anyhow::Result<SessionMeta> {
        let now = Utc::now();
        let meta = SessionMeta {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            created_at: now,
            updated_at: now,
            message_count: 0,
        };
        self.save_meta(&meta).await?;
        tokio::fs::write(self.log_path(&meta.id), b"").await?;
        Ok(meta)
    }

    pub async fn save_meta(&self, meta: &SessionMeta) -> anyhow::Result<()> {
        write_atomic(&self.meta_path(&meta.id), &serde_json::to_vec_pretty(meta)?).await
    }

    pub async fn meta(&self, id: &str) -> anyhow::Result<SessionMeta> {
        let raw = tokio::fs::read(self.meta_path(id)).await?;
        Ok(serde_json::from_slice(&raw)?)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let mut out = Vec::new();
        let mut rd = tokio::fs::read_dir(&self.dir).await?;
        while let Some(entry) = rd.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_suffix(".meta.json") {
                if let Ok(m) = self.meta(id).await {
                    out.push(m);
                }
            }
        }
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(out)
    }

    pub async fn load_messages(&self, id: &str) -> anyhow::Result<Vec<Message>> {
        let raw = tokio::fs::read_to_string(self.log_path(id)).await.unwrap_or_default();
        Ok(raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect())
    }

    pub async fn append(&self, id: &str, messages: &[Message]) -> anyhow::Result<()> {
        let mut buf = Vec::new();
        for m in messages {
            serde_json::to_writer(&mut buf, m)?;
            buf.push(b'\n');
        }
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path(id))
            .await?;
        f.write_all(&buf).await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let _ = tokio::fs::remove_file(self.log_path(id)).await;
        let _ = tokio::fs::remove_file(self.meta_path(id)).await;
        Ok(())
    }
}

fn sanitize(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

pub async fn write_atomic(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let meta = store.create("t").await.unwrap();
        store.append(&meta.id, &[Message::user("hi"), Message::assistant("yo", vec![])]).await.unwrap();
        let msgs = store.load_messages(&meta.id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(store.list().await.unwrap().len(), 1);
        store.delete(&meta.id).await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
    }
}
