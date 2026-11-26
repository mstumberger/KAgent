//! Subagents (agents.md): the profiles of `<config>/agents` with the built-ins, and what
//! a listing of sessions says about them. The routes are `http.rs`, the wiring into the
//! agent `runner.rs`, the files `profiles.rs`; the orchestration itself is
//! `kagent_core::agent::children`.

pub mod http;
mod profiles;
mod runner;
pub mod settings;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::{json, Value};

use kagent_core::agent::children::Profile;
use kagent_core::session::{SessionMeta, SessionStore};

pub use profiles::{check, parse, render, valid_name};
pub use runner::Subagents;
pub use settings::SubagentsSection;

/// What `GET /api/agents` says of a profile (api.md section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProfileView {
    pub name: String,
    pub description: String,
    pub built_in: bool,
    pub source: &'static str,
    pub model: String,
    /// Empty: every enabled group.
    pub tool_groups: Vec<String>,
    pub max_iterations: usize,
    pub prompt: String,
}

/// A file of the folder that is not a valid profile (agents.md section 1): the UI lists
/// it with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvalidProfile {
    pub file: String,
    pub reason: String,
}

pub fn view(profile: &Profile) -> AgentProfileView {
    let built_in = Profile::builtin(&profile.name).is_some();
    AgentProfileView {
        name: profile.name.clone(),
        description: profile.description.clone(),
        built_in,
        source: if built_in { "builtin" } else { "user" },
        model: profile.model.clone(),
        tool_groups: profile.tool_groups.clone(),
        max_iterations: profile.max_iterations,
        prompt: profile.prompt.clone(),
    }
}

#[derive(Default)]
struct Loaded {
    profiles: Vec<Profile>,
    invalid: Vec<InvalidProfile>,
}

/// The profiles of this daemon: the built-ins and the files of one folder.
pub struct ProfileHub {
    dir: PathBuf,
    state: RwLock<Loaded>,
}

impl ProfileHub {
    pub fn new(dir: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Self { dir: dir.into(), state: RwLock::new(Loaded::default()) })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read the folder again: the built-ins, then every valid `<name>.md` in name order;
    /// every other `.md` file with its reason, and a warning per file in the log. A folder
    /// that does not exist is empty. How many profiles of the owner are valid.
    pub fn reload(&self) -> usize {
        let known = crate::settings::known_groups();
        let mut files: Vec<(String, String)> = Vec::new();
        let mut invalid: Vec<InvalidProfile> = Vec::new();
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.ends_with(".md") {
                        continue;
                    }
                    match std::fs::read_to_string(entry.path()) {
                        Ok(raw) => files.push((name, raw)),
                        Err(e) => invalid.push(InvalidProfile { file: name, reason: format!("cannot be read: {e}") }),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(dir = %self.dir.display(), "the profiles cannot be read: {e}"),
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut profiles = vec![Profile::general(), Profile::explore()];
        for (file, raw) in files {
            let name = &file[..file.len() - 3];
            let read = if Profile::builtin(name).is_some() { Err("a built-in profile has this name: it cannot be a file".to_string()) } else { parse(&file, &raw, &known) };
            match read {
                Ok(profile) => profiles.push(profile),
                Err(reason) => {
                    tracing::warn!("profile {} is not used: {reason}", self.dir.join(&file).display());
                    invalid.push(InvalidProfile { file, reason });
                }
            }
        }
        let own = profiles.len() - 2;
        *self.state.write().unwrap() = Loaded { profiles, invalid };
        own
    }

    /// The profiles now, the built-ins first.
    pub fn profiles(&self) -> Vec<Profile> {
        self.state.read().unwrap().profiles.clone()
    }

    pub fn find(&self, name: &str) -> Option<Profile> {
        self.profiles().into_iter().find(|p| p.name == name)
    }

    /// The files that are not valid profiles, in name order.
    pub fn invalid(&self) -> Vec<InvalidProfile> {
        self.state.read().unwrap().invalid.clone()
    }
}

/// The sessions a listing shows (`GET /api/sessions`), each with how many subagents it
/// started (agents.md section 6: the row of a parent says "3 agents").
pub fn listed(all: Vec<SessionMeta>, parent: Option<&str>) -> Vec<(SessionMeta, usize)> {
    SessionStore::visible(all.clone(), parent)
        .into_iter()
        .map(|meta| {
            let agents = all.iter().filter(|m| m.parent.as_deref() == Some(meta.id.as_str())).count();
            (meta, agents)
        })
        .collect()
}

/// A session of a listing as JSON, with `agents`: how many subagents it started.
pub fn with_agents(meta: &SessionMeta, agents: usize) -> Value {
    let mut value = serde_json::to_value(meta).unwrap_or_default();
    if let Value::Object(fields) = &mut value {
        fields.insert("agents".into(), json!(agents));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hub(files: &[(&str, &str)]) -> (tempfile::TempDir, Arc<ProfileHub>) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("agents")).unwrap();
        for (name, text) in files {
            std::fs::write(dir.path().join("agents").join(name), text).unwrap();
        }
        let hub = ProfileHub::new(dir.path().join("agents"));
        hub.reload();
        (dir, hub)
    }

    #[test]
    fn the_builtins_are_always_there_and_the_files_are_read_in_name_order() {
        let empty = ProfileHub::new("/tmp/kagent-no-such-profile-folder");
        assert_eq!(empty.reload(), 0, "a folder that does not exist is empty");
        assert_eq!(empty.invalid(), vec![]);
        assert_eq!(empty.profiles(), [Profile::general(), Profile::explore()]);

        let (_dir, hub) = hub(&[
            ("zeta.md", "---\ndescription: last\n---\n"),
            ("alpha.md", "---\ndescription: first\nmaxIterations: 5\n---\n"),
            ("notes.txt", "not a profile file"),
            ("general.md", "---\ndescription: no\n---\n"),
            ("broken.md", "---\ndescription: broken\nmaxIterations: 99\n---\n"),
        ]);
        let profiles = hub.profiles();
        let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["general", "explore", "alpha", "zeta"], "the built-ins first, then the files by name");
        assert_eq!(hub.reload(), 2, "two profiles of the owner");
        assert_eq!(hub.find("alpha").unwrap().max_iterations, 5);
        assert!(hub.find("nope").is_none());
        let invalid = hub.invalid();
        assert_eq!(invalid.len(), 2, "{invalid:?}");
        assert!(invalid[0].file == "broken.md" && invalid[0].reason.contains("maxIterations must be between 1 and 80, not 99"), "{}", invalid[0].reason);
        assert_eq!(invalid[1].reason, "a built-in profile has this name: it cannot be a file", "a built-in name cannot be a file");
    }

    #[test]
    fn a_view_says_whats_builtin_and_whats_left_out() {
        let v = view(&Profile::explore());
        assert_eq!((v.built_in, v.source, v.max_iterations, v.tool_groups.len()), (true, "builtin", 25, 0));
        assert_eq!(serde_json::to_value(&v).unwrap()["builtIn"], json!(true));
        let own = Profile { name: "disk-audit".into(), description: "d".into(), tool_groups: vec!["files".into()], ..Profile::general() };
        assert_eq!(serde_json::to_value(view(&own)).unwrap(), json!({ "name": "disk-audit", "description": "d", "builtIn": false, "source": "user", "model": "", "toolGroups": ["files"], "maxIterations": 25, "prompt": "" }));
    }

    #[tokio::test]
    async fn a_listing_counts_the_subagents_of_each_session_and_hides_them() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new(dir.path()).unwrap();
        let a = store.create("a").await.unwrap();
        store.create("b").await.unwrap();
        for _ in 0..3 {
            store.create_with("job", kagent_core::session::SessionOrigin::Subagent, Some(a.id.clone())).await.unwrap();
        }
        let all = store.list().await.unwrap();
        let mut listed = listed(all, None);
        listed.sort_by(|a, b| a.0.title.cmp(&b.0.title));
        assert_eq!(listed.iter().map(|(m, n)| (m.title.as_str(), *n)).collect::<Vec<_>>(), [("a", 3), ("b", 0)], "children are not listed; their parent counts them");
        let value = with_agents(&listed[0].0, listed[0].1);
        assert_eq!((value["agents"].clone(), value["title"].clone()), (json!(3), json!("a")));
        assert!(value.get("parent").is_some(), "the meta's own fields stay");
    }
}
