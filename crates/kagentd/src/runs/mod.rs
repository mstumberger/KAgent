//! Workflow runs in the daemon (workflows.md): the definitions of `<config>/workflows` with the shipped ones, the engine of
//! `kagent_core::workflow` wired to the agent, the profiles and the settings, and the routes (`http.rs`).

pub mod http;
pub mod settings;
pub mod tickets;

#[cfg(test)]
mod gated;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kagent_core::agent::children::Profile;
use kagent_core::agent::Agent;
use kagent_core::tools::workflow::WorkflowAccess;
use kagent_core::workflow::{roles, shipped, Definition, Engine, EngineConfig};
use serde::Serialize;

use crate::subagents::ProfileHub;

pub use settings::WorkflowsSection;

/// A file of `<config>/workflows` that is not a valid definition, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidWorkflow {
    pub file: String,
    pub reason: String,
}

pub struct Runs {
    pub engine: Arc<Engine>,
    dir: PathBuf,
    profiles: Arc<ProfileHub>,
}

/// The profile a name stands for: the owner's files and the built-ins of `agents.md`, then the three roles.
pub fn profile_of(hub: &ProfileHub, name: &str) -> Option<Profile> {
    hub.find(name).or_else(|| roles::profile(name))
}

/// The shipped definitions and the valid files of `dir`; the others with their reasons. A file cannot take a shipped name.
pub fn catalog(dir: &Path, known: &dyn Fn(&str) -> bool) -> (Vec<(Definition, bool)>, Vec<InvalidWorkflow>) {
    let mut out: Vec<(Definition, bool)> = shipped::all(known).into_iter().map(|d| (d, true)).collect();
    let mut invalid = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))).collect()).unwrap_or_default();
    files.sort();
    for path in files {
        let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let stem = path.file_stem().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        if shipped::is_shipped(&stem) {
            invalid.push(InvalidWorkflow { file, reason: "a shipped workflow has this name: it cannot be a file".into() });
            continue;
        }
        match std::fs::read_to_string(&path).map_err(|e| format!("cannot be read: {e}")).and_then(|text| Definition::parse(&text, &stem, known)) {
            Ok(d) => out.push((d, false)),
            Err(reason) => {
                tracing::warn!("workflow {} is not used: {reason}", path.display());
                invalid.push(InvalidWorkflow { file, reason });
            }
        }
    }
    (out, invalid)
}

impl Runs {
    /// Once the agent exists: the engine over `<data>/runs`, the runs on disk read back, the tools attached.
    pub fn start(agent: Arc<Agent>, access: WorkflowAccess, profiles: Arc<ProfileHub>, data_runs: PathBuf, dir: PathBuf, max_runs: u32, project_check: kagent_core::tools::workflow::ProjectCheck) -> Arc<Self> {
        let (lookup_hub, def_hub, def_dir) = (profiles.clone(), profiles.clone(), dir.clone());
        let engine = Engine::new(
            agent,
            data_runs,
            Arc::new(move |name: &str| profile_of(&lookup_hub, name)),
            Arc::new(move |name: &str| {
                let hub = def_hub.clone();
                let (defs, _) = catalog(&def_dir, &move |p| profile_of(&hub, p).is_some());
                defs.into_iter().map(|(d, _)| d).find(|d| d.name == name).ok_or_else(|| format!("no workflow '{name}': call GET /api/workflows for the list"))
            }),
        );
        engine.set_config(EngineConfig { max_runs: max_runs as usize, ..Default::default() });
        let n = engine.recover();
        if n > 0 {
            tracing::info!("{n} workflow run(s) read back");
        }
        access.attach(&engine);
        access.set_project_check(project_check);
        Arc::new(Self { engine, dir, profiles })
    }

    pub fn set_max_runs(&self, max: u32) {
        let mut cfg = self.engine.config();
        cfg.max_runs = max as usize;
        self.engine.set_config(cfg);
    }

    /// The definitions now, and the files that are not valid.
    pub fn workflows(&self) -> (Vec<(Definition, bool)>, Vec<InvalidWorkflow>) {
        let hub = self.profiles.clone();
        catalog(&self.dir, &move |p| profile_of(&hub, p).is_some())
    }
}
