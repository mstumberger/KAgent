//! `<data>/runs/<runId>/` (workflows.md section 3.5): `run.json` written atomically, the plan beside it, long messages in
//! `messages/`.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::model::{PlanDoc, Run};

pub struct RunStore {
    dir: PathBuf,
}

impl RunStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn run_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id)
    }

    pub fn save(&self, run: &Run) -> std::io::Result<()> {
        let dir = self.run_dir(&run.id);
        std::fs::create_dir_all(&dir)?;
        write_atomic(&dir.join("run.json"), &serde_json::to_vec_pretty(run).map_err(std::io::Error::other)?)
    }

    pub fn save_plan(&self, id: &str, plan: &PlanDoc) -> std::io::Result<()> {
        let dir = self.run_dir(id);
        std::fs::create_dir_all(&dir)?;
        write_atomic(&dir.join("plan.json"), &serde_json::to_vec_pretty(plan).map_err(std::io::Error::other)?)?;
        write_atomic(&dir.join("plan.md"), super::plan::markdown(plan).as_bytes())
    }

    /// A long message as a file; its path relative to the run's folder.
    pub fn save_message(&self, id: &str, message_id: &str, text: &str) -> std::io::Result<String> {
        let dir = self.run_dir(id).join("messages");
        std::fs::create_dir_all(&dir)?;
        let name = format!("{message_id}.txt");
        write_atomic(&dir.join(&name), text.as_bytes())?;
        Ok(format!("messages/{name}"))
    }

    /// Every readable run, and the files that were not (left on disk).
    pub fn load_all(&self) -> (Vec<Run>, Vec<String>) {
        let (mut runs, mut skipped) = (Vec::new(), Vec::new());
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return (runs, skipped) };
        for entry in entries.flatten() {
            let path = entry.path().join("run.json");
            if !path.is_file() {
                continue;
            }
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|raw| serde_json::from_slice::<Run>(&raw).map_err(|e| e.to_string())) {
                Ok(run) => runs.push(run),
                Err(e) => {
                    tracing::warn!("the run file {} is not used: {e}", path.display());
                    skipped.push(path.display().to_string());
                }
            }
        }
        (runs, skipped)
    }
}

/// A temporary file in the same folder, then rename; mode 0600.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_round_trips_and_a_damaged_file_is_skipped_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::new(dir.path().join("runs"));
        let def = super::super::Definition::parse(include_str!("../../../../workflows/plan-only.yaml"), "plan-only", &|_| true).unwrap();
        let now = chrono::Utc::now();
        let run = Run {
            id: "run_aaaa1111".into(),
            workflow: "plan-only".into(),
            definition: def,
            project: "/tmp".into(),
            goal: "g".into(),
            title: "g".into(),
            state: super::super::RunState::Planning,
            reason: String::new(),
            origin_session: None,
            run_session: "s".into(),
            ticket: None,
            plan_summary: String::new(),
            tasks: vec![],
            questions: vec![],
            messages: vec![],
            planner_session: None,
            feedback: None,
            rejections: 0,
            report: String::new(),
            created_at: now,
            updated_at: now,
        };
        store.save(&run).unwrap();
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(store.run_dir("run_aaaa1111").join("run.json")).unwrap().permissions());
        assert_eq!(mode & 0o777, 0o600);
        std::fs::create_dir_all(store.run_dir("run_bad")).unwrap();
        std::fs::write(store.run_dir("run_bad").join("run.json"), "{oops").unwrap();
        let (runs, skipped) = store.load_all();
        assert_eq!((runs.len(), runs[0].id.as_str(), skipped.len()), (1, "run_aaaa1111", 1));
        assert_eq!(std::fs::read_to_string(store.run_dir("run_bad").join("run.json")).unwrap(), "{oops");
        assert_eq!(std::fs::read_dir(store.run_dir("run_aaaa1111")).unwrap().count(), 1, "no temporary file is left");
    }
}
