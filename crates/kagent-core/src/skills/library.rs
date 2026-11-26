//! The merged skills library (skills.md sections 2-4): roots with a precedence,
//! hidden skills, a cheap check for changes and the prompt index.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::frontmatter;

/// Heading of the skills index in the system prompt.
pub const INDEX_HEADING: &str = "Skills (call load_skill with the name before using one)";
/// A skill body larger than this is skipped.
pub const MAX_BODY_BYTES: usize = 100 * 1024;
/// Not read: a `SKILL.md` larger than the body limit plus room for frontmatter.
const MAX_FILE_BYTES: u64 = MAX_BODY_BYTES as u64 + 64 * 1024;
const SKILL_FILE: &str = "SKILL.md";

/// Where a skill comes from. Declaration order is precedence, highest first: on a name
/// clash the higher row wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    /// `<config>/skills/<name>/`
    User,
    /// `<config>/skills-imported/<name>/`
    Imported,
    /// The playbooks shipped with KAgent.
    Bundled,
}

impl SkillSource {
    pub const ALL: [Self; 3] = [Self::User, Self::Imported, Self::Bundled];

    /// The name the settings and the API use.
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Imported => "imported",
            Self::Bundled => "bundled",
        }
    }
}

/// One place skills are read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub source: SkillSource,
}

/// One skill of the merged library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    /// `when_to_use` of the frontmatter; empty when absent.
    pub when_to_use: String,
    pub source: SkillSource,
    /// The `SKILL.md` file.
    pub path: PathBuf,
    /// The skill's directory: what the `${..._SKILL_DIR}` variables expand to.
    pub dir: PathBuf,
    /// Not named in `skills.disabled`.
    pub enabled: bool,
    /// A skill of a higher root has the same name: this one is not loaded.
    pub shadowed: bool,
    /// Size of the body in bytes.
    pub size_bytes: u64,
    /// The body without its frontmatter.
    pub body: String,
}

impl SkillEntry {
    /// In the prompt index and loadable: enabled and not shadowed.
    pub fn available(&self) -> bool {
        self.enabled && !self.shadowed
    }
}

/// Every skill of the roots, merged on the name.
#[derive(Debug, Clone)]
pub struct SkillLibrary {
    /// Highest precedence first.
    roots: Vec<SkillRoot>,
    /// Sorted, without repeats.
    disabled: Vec<String>,
    /// One per name, the one that wins it: user, imported, bundled, each alphabetical.
    entries: Vec<SkillEntry>,
    /// Those a higher root has the same name as.
    shadowed: Vec<SkillEntry>,
    /// Every directory and `SKILL.md` seen by the last scan, for the change check.
    stamps: Vec<(PathBuf, Stamp)>,
}

impl SkillLibrary {
    /// Read every root once. `roots` in any order; `disabled` hides skills by name.
    pub fn scan(roots: &[SkillRoot], disabled: &[String]) -> Self {
        let mut roots = roots.to_vec();
        roots.sort_by(|a, b| (a.source, &a.path).cmp(&(b.source, &b.path)));
        roots.dedup();
        let mut library = Self { roots, disabled: normalized(disabled), entries: Vec::new(), shadowed: Vec::new(), stamps: Vec::new() };
        library.rescan();
        library
    }

    /// Rescan when a root, a skill file or the disabled list changed since the last
    /// scan: a few `stat` calls per directory and per `SKILL.md`. `true` when it did,
    /// so an edited skill (same size or not) is seen without a restart.
    pub fn refresh_if_changed(&mut self, disabled: &[String]) -> bool {
        let disabled = normalized(disabled);
        if disabled == self.disabled && self.stamps.iter().all(|(path, was)| stamp(path) == *was) {
            return false;
        }
        self.disabled = disabled;
        self.rescan();
        true
    }

    /// Read every root again now (after `skill_save`, `skill_delete` or a change made
    /// over the API), so `load_skill` and the UI see it before the next turn.
    pub fn reload(&mut self) {
        self.rescan();
    }

    pub fn roots(&self) -> &[SkillRoot] {
        &self.roots
    }

    /// The skill that owns `name`, enabled or not; `None` for an unknown name.
    pub fn get(&self, name: &str) -> Option<&SkillEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// The skills the prompt lists and `load_skill` loads.
    pub fn available(&self) -> impl Iterator<Item = &SkillEntry> {
        self.entries.iter().filter(|e| e.available())
    }

    pub fn available_count(&self) -> usize {
        self.available().count()
    }

    /// Every skill found, the shadowed ones included.
    pub fn all(&self) -> impl Iterator<Item = &SkillEntry> {
        self.entries.iter().chain(self.shadowed.iter())
    }

    pub fn shadowed(&self) -> &[SkillEntry] {
        &self.shadowed
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The prompt index (skills.md section 4): `- name: description` lines, user,
    /// imported, bundled, each alphabetical, while `budget_chars` lasts (whole lines
    /// only); the other names follow on one line. Empty when no skill is available.
    pub fn index(&self, budget_chars: usize) -> String {
        let (mut lines, mut used, mut rest) = (String::new(), 0usize, Vec::new());
        for entry in self.available() {
            let line = if entry.description.is_empty() {
                format!("- {}\n", entry.name)
            } else {
                format!("- {}: {}\n", entry.name, one_line(&entry.description))
            };
            let length = line.chars().count();
            if rest.is_empty() && used + length <= budget_chars {
                used += length;
                lines.push_str(&line);
            } else {
                rest.push(entry.name.as_str());
            }
        }
        if lines.is_empty() && rest.is_empty() {
            return String::new();
        }
        let mut out = format!("\n{INDEX_HEADING}:\n{lines}");
        if !rest.is_empty() {
            out.push_str(&format!("Also available (names only): {}\n", rest.join(", ")));
        }
        out
    }

    fn rescan(&mut self) {
        let mut stamps = Vec::new();
        let mut found = Vec::new();
        for root in &self.roots {
            // The same real path reached twice within a root counts once.
            let mut seen = BTreeSet::new();
            for (dir, file) in walk(&root.path, &mut stamps) {
                stamps.push((file.clone(), stamp(&file)));
                let Ok(real) = std::fs::canonicalize(&file) else { continue };
                if !seen.insert(real) {
                    continue;
                }
                let dir_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                match read_entry(&dir, &file, dir_name, root.source) {
                    Ok(mut entry) => {
                        entry.enabled = !self.disabled.contains(&entry.name);
                        found.push(entry);
                    }
                    Err(reason) => tracing::warn!("skill {} skipped: {reason}", file.display()),
                }
            }
        }
        let mut names = BTreeSet::new();
        let (mut entries, mut shadowed) = (Vec::new(), Vec::new());
        // `found` is in order of precedence: the first skill with a name wins it.
        for entry in found {
            if names.insert(entry.name.clone()) {
                entries.push(entry);
            } else {
                shadowed.push(SkillEntry { shadowed: true, ..entry });
            }
        }
        entries.sort_by(|a, b| (a.source, &a.name).cmp(&(b.source, &b.name)));
        shadowed.sort_by(|a, b| (a.source, &a.name).cmp(&(b.source, &b.name)));
        (self.entries, self.shadowed, self.stamps) = (entries, shadowed, stamps);
    }
}

/// `<root>/<name>/SKILL.md`, and `<root>/<dir>/<name>/SKILL.md` where `<dir>` holds no
/// `SKILL.md` itself (a link to a folder of skills). Children in name order. Every
/// directory visited is stamped for the change check.
fn walk(root: &Path, stamps: &mut Vec<(PathBuf, Stamp)>) -> Vec<(PathBuf, PathBuf)> {
    stamps.push((root.to_path_buf(), stamp(root)));
    let mut out = Vec::new();
    for dir in children(root) {
        if !dir.is_dir() {
            continue;
        }
        stamps.push((dir.clone(), stamp(&dir)));
        let own = dir.join(SKILL_FILE);
        if own.is_file() {
            out.push((dir, own));
            continue;
        }
        for sub in children(&dir) {
            let file = sub.join(SKILL_FILE);
            if file.is_file() {
                out.push((sub, file));
            }
        }
    }
    out
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<std::ffi::OsString> =
        std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name()).collect()).unwrap_or_default();
    names.sort();
    names.into_iter().map(|n| dir.join(n)).collect()
}

fn read_entry(dir: &Path, file: &Path, dir_name: &str, source: SkillSource) -> Result<SkillEntry, String> {
    let size = std::fs::metadata(file).map_err(|e| format!("cannot stat it: {e}"))?.len();
    if size > MAX_FILE_BYTES {
        return Err(format!("{size} bytes, more than the {MAX_BODY_BYTES} byte body limit"));
    }
    let raw = std::fs::read_to_string(file).map_err(|e| format!("cannot read it as text: {e}"))?;
    let Some(doc) = frontmatter::parse(&raw) else {
        return Err("its frontmatter name is not a valid id".into());
    };
    let name = if doc.name.is_empty() { dir_name.to_string() } else { doc.name };
    if !frontmatter::valid_name(&name) {
        return Err(format!("name '{name}' is not a valid id"));
    }
    if doc.body.len() > MAX_BODY_BYTES {
        return Err(format!("body of {} bytes, more than {MAX_BODY_BYTES}", doc.body.len()));
    }
    Ok(SkillEntry {
        name,
        description: doc.description,
        when_to_use: doc.when_to_use,
        source,
        path: file.to_path_buf(),
        dir: dir.to_path_buf(),
        enabled: true,
        shadowed: false,
        size_bytes: doc.body.len() as u64,
        body: doc.body,
    })
}

/// The skill folders under `root`, as [`SkillLibrary::scan`] finds them: for a folder
/// of skills that is not a root (an import, skills.md section 7).
pub fn skill_dirs(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    walk(root, &mut Vec::new())
}

/// One skill folder read as the library reads it: the frontmatter name or else the
/// folder name, then the description and the body. What an import offers is what the
/// library loads. `Err` says why the library would skip the skill.
pub fn read_skill(dir: &Path, source: SkillSource) -> Result<SkillEntry, String> {
    let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    read_entry(dir, &dir.join(SKILL_FILE), folder, source)
}

/// What tells that a path changed: its modification time and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    /// `None` when the path is missing or cannot be read.
    mtime: Option<SystemTime>,
    /// A file: its size. A directory: its number of entries.
    size: u64,
}

fn stamp(path: &Path) -> Stamp {
    let Ok(meta) = std::fs::metadata(path) else {
        return Stamp { mtime: None, size: 0 };
    };
    Stamp {
        mtime: meta.modified().ok(),
        size: if meta.is_dir() { std::fs::read_dir(path).map(|rd| rd.count() as u64).unwrap_or(0) } else { meta.len() },
    }
}

/// Sorted and without repeats.
fn normalized(list: &[String]) -> Vec<String> {
    let mut list = list.to_vec();
    list.sort();
    list.dedup();
    list
}

/// One line: line breaks and runs of whitespace become single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
