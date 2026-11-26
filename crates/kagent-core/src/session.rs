//! Sessions persisted as `<dir>/<id>.jsonl` (one message per line) plus
//! `<dir>/<id>.meta.json`. The JSONL doubles as the audit trail of every tool call.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::protocol::{Message, Role};
use crate::util::truncate_str;

/// A message of a digest is cut to this many characters (compaction.md section 5).
const DIGEST_MESSAGE_CHARS: usize = 600;

/// Where a session came from (config.md section 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionOrigin {
    #[default]
    Chat,
    Voice,
    Task,
    /// An alert investigation started from the Alerts page.
    Alert,
    Telegram,
    /// A subagent's own session; not in the session list unless its parent is asked for.
    Subagent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub message_count: usize,
    #[serde(default)]
    pub origin: SessionOrigin,
    /// The session this one was started from (subagents); `None` at top level.
    #[serde(default)]
    pub parent: Option<String>,
    /// The summary of the first `summary_upto` stored messages (compaction.md section
    /// 4); both `None` until the conversation was compacted.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_upto: Option<usize>,
    /// The session's todo list (plan-todo.md section 1); empty until the agent or the
    /// owner writes one. Saved with the meta on every change.
    #[serde(default)]
    pub todo: Vec<crate::todo::TodoItem>,
    /// The session is in plan mode (plan-todo.md section 2): the agent researches only until
    /// a plan is approved or the owner cancels the plan. Saved with the meta on every change.
    #[serde(default)]
    pub planning: bool,
    /// The plan the owner approved last (plan-todo.md section 2): its file and its text, in
    /// the prompt of every later request of the session. `None` until one is approved.
    #[serde(default)]
    pub approved_plan: Option<crate::plan::ApprovedPlan>,
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

    /// A chat session (`create_with` names the origin).
    pub async fn create(&self, title: &str) -> anyhow::Result<SessionMeta> {
        self.create_with(title, SessionOrigin::Chat, None).await
    }

    pub async fn create_with(&self, title: &str, origin: SessionOrigin, parent: Option<String>) -> anyhow::Result<SessionMeta> {
        let now = Utc::now();
        let meta = SessionMeta {
            id: uuid::Uuid::new_v4().to_string(),
            title: title.into(),
            created_at: now,
            updated_at: now,
            message_count: 0,
            origin,
            parent,
            summary: None,
            summary_upto: None,
            todo: Vec::new(),
            planning: false,
            approved_plan: None,
        };
        self.save_meta(&meta).await?;
        tokio::fs::write(self.log_path(&meta.id), b"").await?;
        Ok(meta)
    }

    /// The sessions a listing shows: with `parent`, its children; without, every
    /// session but those of subagents, which hang off their parent.
    pub fn visible(metas: Vec<SessionMeta>, parent: Option<&str>) -> Vec<SessionMeta> {
        match parent {
            Some(id) => metas.into_iter().filter(|m| m.parent.as_deref() == Some(id)).collect(),
            None => metas.into_iter().filter(|m| m.origin != SessionOrigin::Subagent).collect(),
        }
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

    /// What another feature tells a model about this session (compaction.md section
    /// 5): the summary when there is one, else the last user and the last assistant
    /// message, each cut to 600 chars, oldest first, as `User: ...` and `Assistant: ...`
    /// lines. `max_chars` caps the whole text. Empty when there is neither.
    pub async fn digest(&self, id: &str, max_chars: usize) -> anyhow::Result<String> {
        if let Some(summary) = self.meta(id).await?.summary.filter(|s| !s.trim().is_empty()) {
            return Ok(truncate_str(summary.trim(), max_chars).to_string());
        }
        let messages = self.load_messages(id).await?;
        let line = |role: Role, label: &str| {
            messages
                .iter()
                .rposition(|m| m.role == role && !m.content.trim().is_empty())
                .map(|i| (i, format!("{label}: {}", truncate_str(messages[i].content.trim(), DIGEST_MESSAGE_CHARS))))
        };
        // Oldest first: where each of the two stands in the session decides.
        let mut parts: Vec<(usize, String)> = [line(Role::User, "User"), line(Role::Assistant, "Assistant")].into_iter().flatten().collect();
        parts.sort_by_key(|(i, _)| *i);
        Ok(truncate_str(&parts.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join("\n"), max_chars).to_string())
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
        let mut meta = store.create("t").await.unwrap();
        // The compaction fields (compaction.md section 4) round trip.
        assert_eq!((meta.summary, meta.summary_upto), (None, None), "a new session has no summary");
        assert!(!meta.planning && meta.approved_plan.is_none(), "a new session is not planning and has no approved plan");
        meta.summary = Some("The user asked about swap.".into());
        meta.summary_upto = Some(2);
        store.save_meta(&meta).await.unwrap();
        assert_eq!(store.meta(&meta.id).await.unwrap(), meta);
        // The plan fields (plan-todo.md section 2) round trip, under their JSON names.
        meta.planning = true;
        meta.approved_plan = Some(crate::plan::ApprovedPlan { path: "/data/plans/t-1.md".into(), text: "1. step".into() });
        store.save_meta(&meta).await.unwrap();
        assert_eq!(store.meta(&meta.id).await.unwrap(), meta);
        let raw: serde_json::Value = serde_json::from_slice(&tokio::fs::read(dir.path().join(format!("{}.meta.json", meta.id))).await.unwrap()).unwrap();
        assert_eq!((raw["planning"].clone(), raw["approvedPlan"].clone()), (serde_json::json!(true), serde_json::json!({ "path": "/data/plans/t-1.md", "text": "1. step" })));
        store.append(&meta.id, &[Message::user("hi"), Message::assistant("yo", vec![])]).await.unwrap();
        let msgs = store.load_messages(&meta.id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(store.list().await.unwrap().len(), 1);
        store.delete(&meta.id).await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn origin_and_parent_load_with_defaults_and_filter_listings() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        // A meta file from before the fields existed.
        let old = r#"{"id":"old","title":"Old","createdAt":"2026-11-01T10:00:00Z","updatedAt":"2026-11-01T10:00:00Z"}"#;
        tokio::fs::write(dir.path().join("old.meta.json"), old).await.unwrap();
        let m = store.meta("old").await.unwrap();
        assert_eq!((m.origin, m.parent.as_deref()), (SessionOrigin::Chat, None));
        assert_eq!((m.summary, m.summary_upto), (None, None), "a meta from before the compaction fields loads");
        assert!(m.todo.is_empty(), "and before the todo field");
        assert!(!m.planning && m.approved_plan.is_none(), "and before the plan fields");

        let plain = store.create("chat").await.unwrap();
        assert_eq!(plain.origin, SessionOrigin::Chat, "create makes a chat session");

        let chat = store.create_with("c", SessionOrigin::Chat, None).await.unwrap();
        let child = store.create_with("child", SessionOrigin::Subagent, Some(chat.id.clone())).await.unwrap();
        store.create_with("grandchild", SessionOrigin::Subagent, Some(child.id.clone())).await.unwrap();
        let task = store.create_with("task", SessionOrigin::Task, None).await.unwrap();
        // Written and read back with the phase 4 fields.
        assert_eq!(store.meta(&child.id).await.unwrap(), child);

        let all = store.list().await.unwrap();
        let ids = |v: Vec<SessionMeta>| v.into_iter().map(|m| m.id).collect::<Vec<_>>();
        let mut listed = ids(SessionStore::visible(all.clone(), None));
        listed.sort();
        let mut top_level = vec![plain.id, chat.id.clone(), "old".to_string(), task.id.clone()];
        top_level.sort();
        assert_eq!(listed, top_level, "subagent sessions are hidden unless a parent is asked for");
        assert_eq!(ids(SessionStore::visible(all.clone(), Some(&chat.id))), vec![child.id.clone()], "only direct children");
        assert_eq!(ids(SessionStore::visible(all, Some("nobody"))), Vec::<String>::new());
    }

    #[tokio::test]
    async fn digest_is_the_summary_or_the_last_user_and_assistant_lines() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let meta = store.create("t").await.unwrap();
        store
            .append(&meta.id, &[Message::user("first question"), Message::assistant("first answer", vec![]), Message::user("second question"), Message::assistant("second answer", vec![])])
            .await
            .unwrap();
        assert_eq!(store.digest(&meta.id, 2000).await.unwrap(), "User: second question\nAssistant: second answer", "without a summary: the last user and the last assistant message, oldest first");

        // The last assistant message with text, not a step that only called tools.
        store
            .append(&meta.id, &[Message::assistant("", vec![]), Message::user("third question"), Message::user(&"x".repeat(900))])
            .await
            .unwrap();
        assert_eq!(
            store.digest(&meta.id, 2000).await.unwrap(),
            format!("Assistant: second answer\nUser: {}", "x".repeat(600)),
            "each message is cut to 600 chars and ordered as the session has them"
        );
        assert_eq!(store.digest(&meta.id, 40).await.unwrap().chars().count(), 40, "max_chars caps the whole digest");

        // With a summary: the summary alone.
        let mut with = store.create_with("s", SessionOrigin::Chat, None).await.unwrap();
        with.summary = Some("  The user asked about swap twice. ".into());
        with.summary_upto = Some(4);
        store.save_meta(&with).await.unwrap();
        assert_eq!(store.digest(&with.id, 2000).await.unwrap(), "The user asked about swap twice.");
        assert_eq!(store.digest(&with.id, 10).await.unwrap(), "The user a");

        // A session with no message and no summary: an empty digest.
        let empty = store.create("e").await.unwrap();
        assert_eq!(store.digest(&empty.id, 2000).await.unwrap(), "");
    }
}
