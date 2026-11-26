//! Import from other tools (skills.md section 7): the folders that are looked at, the
//! skills they hold, and what an import will do. The copy rule an import and a source
//! share is `copy`. Free of the hub and of the server: a caller passes what the library
//! holds.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use kagent_core::skills::SkillSource;
use kagent_core::skills::frontmatter::valid_name;
use kagent_core::skills::library::{read_skill, skill_dirs};
use serde::{Deserialize, Serialize};

use crate::skills::Installed;

mod copy;

pub use copy::{bytes_of, clear_staging, hash_of, link, skill_files, stage};
#[cfg(test)]
pub use copy::SkillFile;

/// At most this many bytes of files in one imported skill (copy mode).
pub const MAX_SKILL_BYTES: u64 = 20 * 1024 * 1024;
/// The reason the API gives when a name is not overwritten.
pub const NAME_EXISTS: &str = "name exists";
/// ... and when a skill is over that limit.
pub const TOO_LARGE: &str = "more than 20 MB";

/// The folders of other tools that are looked at, under the home directory, in this order.
pub const KNOWN: [(&str, &str); 9] = [
    ("ZCode", ".zcode/skills"),
    ("shared (.agents)", ".agents/skills"),
    ("Claude Code", ".claude/skills"),
    ("Codex", ".codex/skills"),
    ("OpenCode", ".config/opencode/skills"),
    ("OpenCode", ".opencode/skills"),
    ("Goose", ".config/goose/skills"),
    ("Kiro", ".kiro/skills"),
    ("Windsurf", ".codeium/windsurf/skills"),
];

/// How a skill is imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// A copy of the folder under `<config>/skills-imported/<name>`.
    #[default]
    Copy,
    /// `<config>/skills-imported/<name>` as a link to the folder.
    Symlink,
}

impl ImportMode {
    /// The `mode` of POST `/api/skills/import`: `copy` when it is left out.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("copy") {
            "copy" => Ok(Self::Copy),
            "symlink" => Ok(Self::Symlink),
            other => Err(format!("mode must be copy or symlink, not '{other}'")),
        }
    }
}

/// One skill of a folder, as the import dialog lists it (api.md `skills[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub name: String,
    pub description: String,
    /// The skill's folder as found; `real_path` is it with every link resolved.
    pub path: PathBuf,
    pub real_path: PathBuf,
    /// A skill of the library has the same folder or the same name.
    pub installed: bool,
    /// The name of that skill.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
}

/// One folder that was looked at (api.md `sources[]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSource {
    pub agent: &'static str,
    pub path: PathBuf,
    pub exists: bool,
    pub skills: Vec<Candidate>,
}

/// Why a skill was not imported or installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportItem {
    pub name: String,
    /// The folder as found.
    pub dir: PathBuf,
    /// The folder with every link resolved.
    pub real_dir: PathBuf,
    /// SHA-256 of its files: what the detail of the skill remembers.
    pub hash: String,
}

/// What an import will do. Decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportPlan {
    pub items: Vec<ImportItem>,
    pub skipped: Vec<Skipped>,
}

/// What an import did (api.md: `imported` and `skipped`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub imported: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// Every folder of [`KNOWN`] under `home`: whether it is there, and the skills it holds.
/// The same real path is listed once (the first folder of the table wins).
pub fn detect(home: &Path, installed: &[Installed]) -> Vec<ImportSource> {
    let mut seen = BTreeSet::new();
    KNOWN.iter()
        .map(|(agent, rel)| {
            let path = home.join(rel);
            let exists = path.is_dir();
            let skills = if exists { candidates(&path, installed).into_iter().filter(|c| seen.insert(c.real_path.clone())).collect() } else { Vec::new() };
            ImportSource { agent, path, exists, skills }
        })
        .collect()
}

/// The skills under a folder that holds skills, in the order of the library. What the
/// library would not load is left out (the log says which).
pub fn candidates(root: &Path, installed: &[Installed]) -> Vec<Candidate> {
    let (mut out, mut seen): (Vec<Candidate>, BTreeSet<PathBuf>) = (Vec::new(), BTreeSet::new());
    for (dir, _) in skill_dirs(root) {
        let Ok(real_path) = std::fs::canonicalize(&dir) else { continue };
        if !seen.insert(real_path.clone()) {
            continue;
        }
        let entry = match read_skill(&dir, SkillSource::Imported) {
            Ok(entry) => entry,
            Err(reason) => {
                tracing::warn!("import: {} is not offered: {reason}", dir.display());
                continue;
            }
        };
        let same_folder = installed.iter().find(|s| s.real_dir == real_path);
        let same_name = installed.iter().find(|s| s.name == entry.name);
        out.push(Candidate {
            installed: same_folder.is_some() || same_name.is_some(),
            duplicate_of: same_folder.or(same_name).map(|s| s.name.clone()),
            name: entry.name,
            description: entry.description,
            path: dir,
            real_path,
        });
    }
    out
}

/// A skill has the name, or something of that name is in the imported folder already
/// (a link that leads nowhere, too).
fn taken(name: &str, installed: &[Installed], imported_root: &Path) -> bool {
    installed.iter().any(|s| s.name == name) || std::fs::symlink_metadata(imported_root.join(name)).is_ok()
}

/// What importing `names` from the folder `root` does. Decided before anything is
/// written: a name is never overwritten (skills.md section 7). `Err` for a request that
/// cannot be planned at all.
pub fn plan_import(root: &Path, names: &[String], mode: ImportMode, installed: &[Installed], imported_root: &Path) -> Result<ImportPlan, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a folder that holds skills", root.display()));
    }
    if let Some(bad) = names.iter().find(|n| !valid_name(n)) {
        return Err(format!("'{bad}' is not a valid skill name"));
    }
    let candidates = candidates(root, installed);
    let mut plan = ImportPlan::default();
    let mut asked = BTreeSet::new();
    for name in names {
        if !asked.insert(name.as_str()) {
            continue;
        }
        let mut skip = |reason: String| plan.skipped.push(Skipped { name: name.clone(), reason });
        let Some(found) = candidates.iter().find(|c| c.name == *name) else {
            skip("not found".into());
            continue;
        };
        if found.installed || taken(name, installed, imported_root) {
            skip(NAME_EXISTS.into());
            continue;
        }
        let files = match skill_files(&found.path) {
            Ok(files) => files,
            Err(e) => {
                skip(format!("cannot be read: {e}"));
                continue;
            }
        };
        if mode == ImportMode::Copy && bytes_of(&files) > MAX_SKILL_BYTES {
            skip(TOO_LARGE.into());
            continue;
        }
        let hash = match hash_of(&files, &found.path) {
            Ok(hash) => hash,
            Err(e) => {
                skip(format!("cannot be read: {e}"));
                continue;
            }
        };
        plan.items.push(ImportItem { name: name.clone(), dir: found.path.clone(), real_dir: found.real_path.clone(), hash });
    }
    Ok(plan)
}

/// Do `plan` under `imported_root`: copy each skill, or link to it. A skill that cannot
/// be written is in the answer with why; the others are done.
pub async fn apply_import(plan: &ImportPlan, mode: ImportMode, imported_root: &Path) -> ImportOutcome {
    let mut out = ImportOutcome { imported: Vec::new(), skipped: plan.skipped.clone() };
    if mode == ImportMode::Copy && !plan.items.is_empty() {
        clear_staging(imported_root).await;
    }
    for item in &plan.items {
        let dest = imported_root.join(&item.name);
        let done = match mode {
            ImportMode::Copy => stage(&item.dir, &dest, imported_root, false).await,
            ImportMode::Symlink => link(&item.real_dir, &dest).await,
        };
        match done {
            Ok(()) => out.imported.push(item.name.clone()),
            Err(e) => out.skipped.push(Skipped { name: item.name.clone(), reason: reason_of(&e) }),
        }
    }
    if mode == ImportMode::Copy && !plan.items.is_empty() {
        clear_staging(imported_root).await;
    }
    out
}

/// What an import or a sync says of a write that failed: its reason when it is one
/// (`name exists`, `more than 20 MB`), the system's words otherwise.
pub fn reason_of(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::AlreadyExists => NAME_EXISTS.into(),
        io::ErrorKind::FileTooLarge => TOO_LARGE.into(),
        _ => format!("cannot be written: {e}"),
    }
}

/// What the detail of an imported skill says about where it came from: the tool of a
/// known folder, else the folder's path.
pub fn origin_label(root: &Path, home: &Path) -> String {
    let real = std::fs::canonicalize(root).ok();
    KNOWN.iter()
        .find(|(_, rel)| std::fs::canonicalize(home.join(rel)).ok() == real)
        .map(|(agent, _)| agent.to_string())
        .unwrap_or_else(|| root.display().to_string())
}

#[cfg(test)]
mod tests;
