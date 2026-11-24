//! Change journal (docs/specs/config.md section 7): one JSON file per applied change,
//! with everything needed to undo it. The single write path for "what did KAgent change".

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeStatus {
    Applied,
    Failed,
    Undone,
    UndoFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub session_id: String,
    pub call_id: String,
    pub tool: String,
    pub summary: String,
    /// What was executed: a root-helper request, or `{"kind":"user",...}`.
    pub op: Value,
    /// How to reverse it (same shapes as `op`); `None` = not undoable.
    pub undo: Option<Value>,
    pub privileged: bool,
    pub status: ChangeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

type Listener = Box<dyn Fn(&Change) + Send + Sync>;

pub struct ChangeJournal {
    dir: PathBuf,
    listener: Mutex<Option<Listener>>,
}

impl ChangeJournal {
    pub fn new(dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir, listener: Mutex::new(None) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Called after every record/update (daemon: event broadcast + audit).
    pub fn set_listener(&self, f: impl Fn(&Change) + Send + Sync + 'static) {
        *self.listener.lock().unwrap() = Some(Box::new(f));
    }

    pub fn new_id() -> String {
        let now = Utc::now();
        format!("chg_{}_{}", now.format("%Y%m%d_%H%M%S"), &uuid::Uuid::new_v4().simple().to_string()[..6])
    }

    fn path(&self, id: &str) -> anyhow::Result<PathBuf> {
        if !id.starts_with("chg_") || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            anyhow::bail!("invalid change id {id}");
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// Directory for user-level file backups of one change.
    pub fn backup_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.bak"))
    }

    pub fn save(&self, change: &Change) -> anyhow::Result<()> {
        let path = self.path(&change.id)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(change)?)?;
        std::fs::rename(&tmp, &path)?;
        if let Some(l) = self.listener.lock().unwrap().as_ref() {
            l(change);
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Change> {
        Ok(serde_json::from_slice(&std::fs::read(self.path(id)?)?)?)
    }

    /// Newest first.
    pub fn list(&self, limit: usize) -> Vec<Change> {
        let mut out: Vec<Change> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .filter_map(|e| std::fs::read(e.path()).ok())
                    .filter_map(|b| serde_json::from_slice(&b).ok())
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a: &Change, b| b.ts.cmp(&a.ts));
        out.truncate(limit);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn save_list_get_and_reject_bad_ids() {
        let dir = tempfile::tempdir().unwrap();
        let j = ChangeJournal::new(dir.path()).unwrap();
        let seen = std::sync::Arc::new(Mutex::new(0));
        let s2 = seen.clone();
        j.set_listener(move |_| *s2.lock().unwrap() += 1);
        let mut c = Change {
            id: ChangeJournal::new_id(),
            ts: Utc::now(),
            session_id: "s".into(),
            call_id: "c".into(),
            tool: "sysctl".into(),
            summary: "sysctl vm.swappiness=10".into(),
            op: json!({"op": "sysctl_set"}),
            undo: Some(json!({"op": "sysctl_set", "value": "60"})),
            privileged: true,
            status: ChangeStatus::Applied,
            before: Some("60".into()),
            after: Some("10".into()),
            undone_at: None,
            note: None,
        };
        j.save(&c).unwrap();
        c.status = ChangeStatus::Undone;
        j.save(&c).unwrap();
        assert_eq!(j.list(10).len(), 1);
        assert_eq!(j.get(&c.id).unwrap().status, ChangeStatus::Undone);
        assert_eq!(*seen.lock().unwrap(), 2);
        assert!(j.get("../../etc/passwd").is_err());
    }
}
