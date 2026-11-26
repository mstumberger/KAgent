//! Git sources of skills (skills.md section 8): the store (`<config>/skill-sources.json`),
//! what a new source may be, the command lines of git, the scan of a source, the sync
//! plan and what it does to the imported folder. One copy of the file behind a lock,
//! and one lock for every change to the imported skills (import, add, sync, remove).
//!
//! - `validate`: what a new source may be, and the id it gets.
//! - `git`: the command lines, and the one way they run.
//! - `read`: the limits, and the skills a source holds with their hashes.
//! - `plan`: the sync plan, and what doing it writes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use kagent_core::skills::frontmatter::valid_name;
use serde::{Deserialize, Serialize};

mod git;
mod plan;
mod read;
mod validate;

pub use git::{checkout_argv, clone_argv, fetch_argv, git, head_argv, GitError};
#[cfg(test)]
pub use git::tail;
pub use plan::{install, plan_sync, SyncPlan};
pub use read::{check_limits, scan, subdir_path};
#[cfg(test)]
pub use read::Found;
pub use validate::{id_for, is_local, validate};

/// One skill a source installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillRecord {
    pub name: String,
    /// The skill's folder in the source.
    pub path: PathBuf,
    /// SHA-256 of its files when it was installed.
    pub hash: String,
    pub synced_at: String,
}

/// A source, as the file holds it (skills.md section 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRecord {
    pub id: String,
    pub name: String,
    /// An `https://` address, or the absolute path of a folder.
    pub url: String,
    /// Branch, tag or commit; empty is the default branch.
    #[serde(default)]
    pub r#ref: String,
    /// The folder inside the repository that holds the skills; empty is its root.
    #[serde(default)]
    pub subdir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub skills: Vec<SkillRecord>,
}

/// A skill imported by hand from another tool's folder (skills.md section 7): where it
/// came from, for the detail of the skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRecord {
    pub name: String,
    /// The tool of a known folder, else the folder's path.
    pub source: String,
    pub path: PathBuf,
    pub hash: String,
}

/// `<config>/skill-sources.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceFile {
    #[serde(default = "version_one")]
    pub schema_version: u32,
    #[serde(default)]
    pub sources: Vec<SourceRecord>,
    /// Skills imported by hand; a source's skills are with the source.
    #[serde(default)]
    pub imports: Vec<ImportRecord>,
}

fn version_one() -> u32 {
    1
}

impl Default for SourceFile {
    fn default() -> Self {
        Self { schema_version: 1, sources: Vec::new(), imports: Vec::new() }
    }
}

impl SourceFile {
    /// As the file must be: an id is used as a folder name and as a segment of the API.
    pub fn check(&self) -> Result<(), String> {
        for source in &self.sources {
            if !valid_name(&source.id) {
                return Err(format!("the id '{}' of the source '{}' is not a valid id", source.id, source.name));
            }
        }
        Ok(())
    }
}

/// Where an imported skill came from (api.md `SkillView.importedFrom`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedFrom {
    /// The name of the source, or the tool the skill was imported from.
    pub source: String,
    /// Its folder in the source.
    pub path: PathBuf,
    pub hash: String,
}

/// One skill of a source, as the API lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillViewRow {
    pub name: String,
    pub hash: String,
    pub synced_at: String,
}

/// A source as the API lists it (api.md `SourceView`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub url: String,
    pub r#ref: String,
    pub subdir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub skills: Vec<SkillViewRow>,
}

pub fn view_of(record: &SourceRecord) -> SourceView {
    SourceView {
        id: record.id.clone(),
        name: record.name.clone(),
        url: record.url.clone(),
        r#ref: record.r#ref.clone(),
        subdir: record.subdir.clone(),
        commit: record.commit.clone(),
        last_sync: record.last_sync.clone(),
        last_error: record.last_error.clone(),
        skills: record.skills.iter().map(|s| SkillViewRow { name: s.name.clone(), hash: s.hash.clone(), synced_at: s.synced_at.clone() }).collect(),
    }
}

/// One suggestion of the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogEntry {
    pub name: String,
    pub url: String,
    pub subdir: String,
    pub note: String,
}

static CATALOG: OnceLock<Vec<CatalogEntry>> = OnceLock::new();

/// The suggestions of `config/skill-sources.builtin.json`.
pub fn catalog() -> &'static [CatalogEntry] {
    CATALOG.get_or_init(|| {
        #[derive(Deserialize)]
        struct File {
            sources: Vec<CatalogEntry>,
        }
        serde_json::from_str::<File>(include_str!("../../../config/skill-sources.builtin.json")).expect("config/skill-sources.builtin.json is well formed").sources
    })
}


// ------------------------------------------------------------------ the store

/// The sources of the daemon: one copy of the file behind a lock, and one lock for
/// every change to the imported skills, so two of them never write one folder.
#[derive(Debug)]
pub struct SourceStore {
    path: PathBuf,
    /// `<data>/skill-sources`: the clones.
    clones: PathBuf,
    file: RwLock<SourceFile>,
    writers: tokio::sync::Mutex<()>,
}

impl SourceStore {
    /// Read the file; a missing file is empty. A damaged file is an error: the daemon
    /// then does not start, and the owner's sources are never dropped silently.
    pub fn load(path: impl Into<PathBuf>, clones: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let file = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str::<SourceFile>(&raw).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => SourceFile::default(),
            Err(e) => return Err(format!("{}: cannot be read: {e}", path.display())),
        };
        file.check().map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { path, clones: clones.into(), file: RwLock::new(file), writers: tokio::sync::Mutex::new(()) })
    }

    /// Held by everything that installs or removes imported skills.
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.writers.lock().await
    }

    pub fn list(&self) -> Vec<SourceRecord> {
        self.file.read().unwrap().sources.clone()
    }

    pub fn get(&self, id: &str) -> Option<SourceRecord> {
        self.list().into_iter().find(|s| s.id == id)
    }

    /// `<data>/skill-sources/<id>`.
    pub fn clone_dir(&self, id: &str) -> PathBuf {
        self.clones.join(id)
    }

    /// Save `record` in place of the one with its id, or add it. The skills a source
    /// installed are its own from now on: a note of a hand-made import of the same name goes.
    pub fn put(&self, record: &SourceRecord) -> Result<(), String> {
        let mut file = self.file.write().unwrap();
        file.imports.retain(|i| !record.skills.iter().any(|s| s.name == i.name));
        file.sources.retain(|s| s.id != record.id);
        file.sources.push(record.clone());
        save(&file, &self.path)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut file = self.file.write().unwrap();
        file.sources.retain(|s| s.id != id);
        save(&file, &self.path)
    }

    /// Remember where skills imported by hand came from (one record per name).
    pub fn note_import(&self, records: &[ImportRecord]) -> Result<(), String> {
        let mut file = self.file.write().unwrap();
        file.imports.retain(|i| !records.iter().any(|r| r.name == i.name));
        file.imports.extend_from_slice(records);
        save(&file, &self.path)
    }

    /// Where the imported skill `name` came from, when KAgent recorded it.
    pub fn provenance(&self, name: &str) -> Option<ImportedFrom> {
        let file = self.file.read().unwrap();
        file.imports
            .iter()
            .find(|i| i.name == name)
            .map(|i| ImportedFrom { source: i.source.clone(), path: i.path.clone(), hash: i.hash.clone() })
            .or_else(|| {
                file.sources.iter().find_map(|s| {
                    s.skills.iter().find(|k| k.name == name).map(|k| ImportedFrom { source: s.name.clone(), path: k.path.clone(), hash: k.hash.clone() })
                })
            })
    }

    /// `name` was imported by hand: no source may remove or replace it.
    pub fn imported_by_hand(&self, name: &str) -> bool {
        self.file.read().unwrap().imports.iter().any(|i| i.name == name)
    }
}

/// Atomic and 0600, like every config file.
fn save(file: &SourceFile, path: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let action = |e: std::io::Error| format!("{}: cannot be written: {e}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(action)?;
    }
    let tmp = path.with_extension("tmp");
    let mut text = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    text.push('\n');
    let mut out = std::fs::File::create(&tmp).map_err(action)?;
    out.write_all(text.as_bytes()).map_err(action)?;
    out.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(action)?;
    drop(out);
    std::fs::rename(&tmp, path).map_err(action)
}

#[cfg(test)]
mod tests;
