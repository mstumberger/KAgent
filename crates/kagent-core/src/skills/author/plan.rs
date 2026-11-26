//! What is checked before a skill is written or removed, and the writing itself
//! (tools.md section 17; skills.md sections 5 and 10). Pure of the agent loop: the
//! skills API runs the same checks for the owner.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::super::frontmatter;
use super::super::library::{SkillLibrary, SkillSource};

/// Skills KAgent ships with whose names a new skill must not take.
pub const RESERVED: [&str; 2] = ["skill-creator", "widget-design"];
/// Longest body of a saved skill. The library reads up to 100 KB, so what these tools
/// write always loads.
pub const MAX_SAVE_BODY_BYTES: usize = 64 * 1024;
const SKILL_FILE: &str = "SKILL.md";

/// What the daemon does after a skill was written or removed: read the library again
/// and tell the UI.
pub type OnChange = Arc<dyn Fn() + Send + Sync>;

// ------------------------------------------------------------------ saving

/// What saving a skill does. Decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavePlan {
    pub name: String,
    /// `<user root>/<name>`.
    pub dir: PathBuf,
    /// A skill file is there already and gets replaced.
    pub overwrite: bool,
    /// A skill of another source has the same name: this one hides it (user wins).
    pub shadows: Option<SkillSource>,
    /// `skills.disabled` names it, so it stays out of the prompt until it is switched on.
    pub disabled: bool,
}

/// Check a skill before it is written under `user_root`: the name, the sizes and the
/// target directory (skills.md section 5, tools.md section 17). `Err` says why not.
pub fn plan_save(library: &SkillLibrary, user_root: &Path, name: &str, description: &str, body: &str) -> Result<SavePlan, String> {
    let name = name.trim();
    if RESERVED.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(format!("'{name}' is the name of a skill KAgent ships with; choose another name"));
    }
    if let Some(other) = library.all().find(|e| e.name.eq_ignore_ascii_case(name) && e.name != name) {
        return Err(format!("a skill named '{}' exists; a name that differs only in case is not allowed", other.name));
    }
    if !frontmatter::valid_name(name) {
        return Err(format!("'{name}' is not a valid skill name: lowercase letters, digits and hyphens only, starting with a letter or a digit, at most 64 characters"));
    }
    let chars = description.chars().count();
    if chars > frontmatter::MAX_DESCRIPTION_CHARS {
        return Err(format!("the description has {chars} characters, more than {}", frontmatter::MAX_DESCRIPTION_CHARS));
    }
    if body.trim().is_empty() {
        return Err("the body is empty; write what the agent should do".into());
    }
    if body.len() > MAX_SAVE_BODY_BYTES {
        return Err(format!("the body is {} bytes, more than {MAX_SAVE_BODY_BYTES}", body.len()));
    }
    let dir = user_root.join(name);
    let meta = std::fs::symlink_metadata(&dir).ok();
    if meta.as_ref().is_some_and(|m| m.file_type().is_symlink()) {
        return Err(format!("{} is a symlink; KAgent writes no skill through a link", dir.display()));
    }
    if meta.as_ref().is_some_and(|m| !m.is_dir()) {
        return Err(format!("{} is a file, not a skill directory", dir.display()));
    }
    Ok(SavePlan {
        overwrite: dir.join(SKILL_FILE).is_file(),
        shadows: library.all().find(|e| e.name == name && e.source != SkillSource::User).map(|e| e.source),
        disabled: library.all().any(|e| e.name == name && !e.enabled),
        name: name.to_string(),
        dir,
    })
}

/// Write the skill file of `plan`: a temporary file in its directory, then a rename,
/// so a reader never sees half a skill.
pub async fn write_skill(plan: &SavePlan, text: &str) -> std::io::Result<PathBuf> {
    tokio::fs::create_dir_all(&plan.dir).await?;
    let tmp = plan.dir.join(".SKILL.md.tmp");
    tokio::fs::write(&tmp, text).await?;
    let path = plan.dir.join(SKILL_FILE);
    tokio::fs::rename(&tmp, &path).await?;
    Ok(path)
}

// ------------------------------------------------------------------ deleting

/// What deleting a skill does. Decided before anything is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletePlan {
    pub name: String,
    pub dir: PathBuf,
    /// The skill is a symlink (an imported one may be): only the link is removed.
    pub link: bool,
    /// The files inside: path relative to the directory, and size.
    pub files: Vec<(String, u64)>,
    /// A skill of another source has the same name: it is in use again afterwards.
    pub unshadows: Option<SkillSource>,
}

/// Check `root/<name>` before it is removed (skills.md section 10). `link_ok` lets a
/// symlink go as a link; otherwise a link is refused. `Err` says why nothing is deleted.
pub fn plan_delete(library: &SkillLibrary, root: &Path, name: &str, link_ok: bool) -> Result<DeletePlan, String> {
    let name = name.trim();
    if !frontmatter::valid_name(name) {
        return Err(format!("'{name}' is not a valid skill name"));
    }
    let dir = root.join(name);
    let meta = std::fs::symlink_metadata(&dir).ok();
    match (library.get(name).map(|e| (e.source, e.dir.clone())), &meta) {
        // Not there, and known from another root: it is not this root's to delete.
        (Some((source, from)), None) if source != SkillSource::User => {
            let how = if source == SkillSource::Bundled { "switch it off in Settings -> Skills; it cannot be deleted" } else { "remove it in Settings -> Skills" };
            return Err(format!("skill '{name}' is {}. It comes from {}: {how}", source.label(), from.display()));
        }
        (None, None) => return Err(format!("unknown skill '{name}'")),
        _ => {}
    }
    let link = meta.as_ref().is_some_and(|m| m.file_type().is_symlink());
    if link && !link_ok {
        return Err(format!("{} is a symlink; KAgent deletes no skill through a link", dir.display()));
    }
    if !link {
        if meta.as_ref().is_some_and(|m| !m.is_dir()) {
            return Err(format!("{} is not a skill directory", dir.display()));
        }
        // However it is reached, the directory must be inside the root.
        match (std::fs::canonicalize(&dir), std::fs::canonicalize(root)) {
            (Ok(real), Ok(base)) if real.starts_with(&base) => {}
            _ => return Err(format!("{} does not resolve inside {}; nothing is deleted", dir.display(), root.display())),
        }
    }
    let files = if link { Vec::new() } else { list_files(&dir) };
    Ok(DeletePlan {
        unshadows: library.shadowed().iter().find(|e| e.name == name).map(|e| e.source),
        name: name.to_string(),
        dir,
        link,
        files,
    })
}

/// Files under `dir`, relative paths in name order. A link is an entry of its own and
/// is never followed.
fn list_files(dir: &Path) -> Vec<(String, u64)> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries
            .flatten()
            .map(|e| {
                let meta = e.metadata().ok();
                (e.file_name(), e.path(), meta.as_ref().map(|m| m.file_type()), meta.map(|m| m.len()).unwrap_or(0))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path, kind, len) in entries {
            let rel = format!("{prefix}{}", name.to_string_lossy());
            if kind.is_some_and(|k| k.is_dir()) {
                walk(&path, &format!("{rel}/"), out);
            } else {
                out.push((rel, len));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out
}

/// Remove what `plan` named: the directory with everything in it (links inside go as
/// links), or only the link.
pub async fn remove_skill(plan: &DeletePlan) -> std::io::Result<()> {
    if plan.link {
        tokio::fs::remove_file(&plan.dir).await
    } else {
        tokio::fs::remove_dir_all(&plan.dir).await
    }
}
