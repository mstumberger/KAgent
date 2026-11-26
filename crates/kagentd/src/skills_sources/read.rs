//! Reading a source: where its `subdir` is, the limits on what is read, and the skills
//! it holds with the hash of each.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use kagent_core::skills::SkillSource;
use kagent_core::skills::library::read_skill;

use crate::skills_import::{bytes_of, hash_of, skill_files, MAX_SKILL_BYTES, TOO_LARGE, Skipped};


/// A clone may hold at most this many bytes of files.
pub const MAX_CLONE_BYTES: u64 = 200 * 1024 * 1024;
/// ... and this many files.
pub const MAX_CLONE_FILES: usize = 2000;
/// At most this many skills are installed from one source.
pub const MAX_SYNC_SKILLS: usize = 100;

// ------------------------------------------------------------------ reading a source

/// The `subdir` of a source, folder by folder: each must be a real folder, so a link of
/// the repository never leads out of it.
pub fn subdir_path(root: &Path, subdir: &str) -> Result<PathBuf, String> {
    let mut dir = root.to_path_buf();
    for part in subdir.split('/').filter(|p| !p.is_empty()) {
        dir.push(part);
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(format!("{subdir} is not a folder of the source"));
        }
    }
    Ok(dir)
}

/// The limits on what is read (skills.md section 8): bytes and files under `dir`.
pub fn check_limits(dir: &Path) -> Result<(), String> {
    let (bytes, files) = measure(dir).map_err(|e| format!("{} cannot be read: {e}", dir.display()))?;
    if bytes > MAX_CLONE_BYTES {
        return Err(format!("the source is {} MB, more than the 200 MB limit", bytes.div_ceil(1_048_576)));
    }
    if files > MAX_CLONE_FILES {
        return Err(format!("the source has {files} files, more than the 2000 file limit"));
    }
    Ok(())
}

fn measure(dir: &Path) -> io::Result<(u64, usize)> {
    let (mut bytes, mut files) = (0u64, 0usize);
    for entry in std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()? {
        let kind = entry.file_type()?;
        if kind.is_dir() {
            let (b, f) = measure(&entry.path())?;
            (bytes, files) = (bytes + b, files + f);
        } else if kind.is_file() {
            bytes += entry.metadata()?.len();
            files += 1;
        }
    }
    Ok((bytes, files))
}

/// One skill a source holds now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub dir: PathBuf,
    /// SHA-256 of its files.
    pub hash: String,
}

/// The skills under the `subdir` of a source, by name: two levels deep (skills.md
/// section 1), in real folders only. A skill the library would not load, or that is
/// over the copy limit, is in the second list with why. More than [`MAX_SYNC_SKILLS`]
/// skills is an error.
pub fn scan(root: &Path, subdir: &str) -> Result<(Vec<Found>, Vec<Skipped>), String> {
    let folder = subdir_path(root, subdir)?;
    let mut dirs = Vec::new();
    for dir in real_children(&folder) {
        if is_real_file(&dir.join("SKILL.md")) {
            dirs.push(dir);
            continue;
        }
        dirs.extend(real_children(&dir).into_iter().filter(|sub| is_real_file(&sub.join("SKILL.md"))));
    }
    if dirs.len() > MAX_SYNC_SKILLS {
        return Err(format!("the source holds {} skills; at most {MAX_SYNC_SKILLS} are installed from one source: choose a smaller subdir", dirs.len()));
    }
    let (mut found, mut skipped, mut seen) = (Vec::new(), Vec::new(), BTreeSet::new());
    for dir in dirs {
        let skill = match read_skill(&dir, SkillSource::Imported) {
            Ok(skill) => skill,
            Err(reason) => {
                skipped.push(Skipped { name: folder_name(&dir), reason });
                continue;
            }
        };
        match hash_of_skill(&dir) {
            Ok(hash) if seen.insert(skill.name.clone()) => found.push(Found { name: skill.name, dir, hash }),
            Ok(_) => skipped.push(Skipped { name: skill.name, reason: "listed twice in the source".into() }),
            Err(reason) => skipped.push(Skipped { name: skill.name, reason }),
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((found, skipped))
}

/// The hash of a skill folder; `Err` when it is over the copy limit (such a skill is
/// not installed) or cannot be read.
pub(super) fn hash_of_skill(dir: &Path) -> Result<String, String> {
    let files = skill_files(dir).map_err(|e| format!("cannot be read: {e}"))?;
    if bytes_of(&files) > MAX_SKILL_BYTES {
        return Err(TOO_LARGE.into());
    }
    hash_of(&files, dir).map_err(|e| format!("cannot be read: {e}"))
}

fn folder_name(dir: &Path) -> String {
    dir.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

fn real_children(dir: &Path) -> Vec<PathBuf> {
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    names.retain(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()));
    names.sort();
    names
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

