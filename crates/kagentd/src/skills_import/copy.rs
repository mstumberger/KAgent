//! The copy rule an import and a source share (skills.md sections 7 and 8): files only,
//! links left out, at most 20 MB per skill, landed in one step, never over anything.
//! Also the listing of a skill's files and its hash.

use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{MAX_SKILL_BYTES, NAME_EXISTS, TOO_LARGE};

/// Linux `O_NOFOLLOW`: a file is read as what it is, never through a link.
const O_NOFOLLOW: i32 = 0o400000;

/// Put a copy of the skill folder `from` at `dest` as one step: the copy is made beside
/// the imported folder and renamed into place, so the library never reads half a skill.
/// With `replace`, what is at `dest` goes first (a copy of a source whose content
/// changed). Nothing is written over anything: `dest` must be free.
pub async fn stage(from: &Path, dest: &Path, imported_root: &Path, replace: bool) -> io::Result<()> {
    let staging = staging_dir(imported_root);
    tokio::fs::create_dir_all(&staging).await?;
    let tag: String = format!("{:08x}", rand::random::<u32>());
    let (fresh, old) = (staging.join(format!("{tag}-new")), staging.join(format!("{tag}-old")));
    copy_skill(from, &fresh).await?;
    let moved = if replace {
        match tokio::fs::rename(dest, &old).await {
            Ok(()) => true,
            // The copy was removed by hand: nothing to move aside.
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => {
                let _ = tokio::fs::remove_dir_all(&fresh).await;
                return Err(e);
            }
        }
    } else {
        false
    };
    let result = move_in(&fresh, dest).await;
    match &result {
        Ok(()) if moved => {
            let _ = tokio::fs::remove_dir_all(&old).await;
        }
        Ok(()) => {}
        Err(_) if moved => {
            let _ = tokio::fs::rename(&old, dest).await;
            let _ = tokio::fs::remove_dir_all(&fresh).await;
        }
        Err(_) => {
            let _ = tokio::fs::remove_dir_all(&fresh).await;
        }
    }
    // The staging folder goes when nothing is left in it.
    let _ = tokio::fs::remove_dir(&staging).await;
    result
}

/// Rename `from` onto `dest`, which must be free: an empty folder is made there first
/// (that fails when the name is taken) and takes the copy in one step.
async fn move_in(from: &Path, dest: &Path) -> io::Result<()> {
    if let Err(e) = tokio::fs::create_dir(dest).await {
        return Err(if e.kind() == io::ErrorKind::AlreadyExists { exists() } else { e });
    }
    let result = tokio::fs::rename(from, dest).await;
    if result.is_err() {
        let _ = tokio::fs::remove_dir(dest).await;
    }
    result
}

/// `dest` as a link to the skill folder `target`. The link is made in one step, so it
/// fails when something has the name: nothing is overwritten, and the target is never written.
pub async fn link(target: &Path, dest: &Path) -> io::Result<()> {
    tokio::fs::symlink(target, dest).await.map_err(|e| if e.kind() == io::ErrorKind::AlreadyExists { exists() } else { e })
}

/// Remove what an interrupted import or sync left beside the imported folder.
pub async fn clear_staging(imported_root: &Path) {
    let _ = tokio::fs::remove_dir_all(staging_dir(imported_root)).await;
}

/// Copies are made here and renamed into the imported folder: beside it, where the
/// library does not look.
pub(super) fn staging_dir(imported_root: &Path) -> PathBuf {
    let name = imported_root.file_name().and_then(|n| n.to_str()).unwrap_or("skills-imported");
    imported_root.with_file_name(format!("{name}.staging"))
}

pub(super) fn exists() -> io::Error {
    io::Error::new(io::ErrorKind::AlreadyExists, NAME_EXISTS)
}

pub(super) fn too_large() -> io::Error {
    io::Error::new(io::ErrorKind::FileTooLarge, TOO_LARGE)
}

/// Copy the skill folder `from` to `to`, which must not exist: files only, links left
/// out, at most [`MAX_SKILL_BYTES`]. What was written is removed when the copy fails.
pub async fn copy_skill(from: &Path, to: &Path) -> io::Result<()> {
    let result = match skill_files(from) {
        Ok(files) => copy_files(from, to, &files).await,
        Err(e) => Err(e),
    };
    if result.is_err() {
        let _ = tokio::fs::remove_dir_all(to).await;
    }
    result
}

async fn copy_files(from: &Path, to: &Path, files: &[SkillFile]) -> io::Result<()> {
    tokio::fs::create_dir_all(to).await?;
    let mut budget = MAX_SKILL_BYTES;
    for file in files.iter().filter(|f| !f.link) {
        let (source, dest) = (from.join(&file.rel), to.join(&file.rel));
        if let Some(dir) = dest.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let mut input = tokio::fs::File::from_std(open_nofollow(&source)?);
        let meta = input.metadata().await?;
        budget = budget.checked_sub(meta.len()).ok_or_else(too_large)?;
        let mut output = tokio::fs::File::create(&dest).await?;
        // Bounded at the size the file had: one that grows is not followed to the end.
        let written = tokio::io::copy(&mut (&mut input).take(meta.len() + 1), &mut output).await?;
        if written != meta.len() {
            return Err(io::Error::other(format!("{} changed while it was copied", source.display())));
        }
        output.set_permissions(std::fs::Permissions::from_mode(meta.permissions().mode() & 0o777)).await?;
    }
    Ok(())
}

/// One file of a skill folder: `link` when it is a symbolic link (its size is 0).
/// Folders, and anything else (pipes, devices), are not listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    /// Path from the skill folder.
    pub rel: PathBuf,
    pub size: u64,
    pub link: bool,
}

/// The files under a skill folder, by relative path in name order. Links are entries of
/// their own and are never followed.
pub fn skill_files(dir: &Path) -> io::Result<Vec<SkillFile>> {
    let mut out = Vec::new();
    list(dir, Path::new(""), &mut out)?;
    Ok(out)
}

fn list(dir: &Path, prefix: &Path, out: &mut Vec<SkillFile>) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let rel = prefix.join(entry.file_name());
        if kind.is_dir() {
            list(&entry.path(), &rel, out)?;
        } else if kind.is_symlink() {
            out.push(SkillFile { rel, size: 0, link: true });
        } else if kind.is_file() {
            out.push(SkillFile { rel, size: entry.metadata()?.len(), link: false });
        }
    }
    Ok(())
}

/// Bytes of the files of a skill (links do not count).
pub fn bytes_of(files: &[SkillFile]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

/// SHA-256 over files that were listed already (`skill_files`).
pub fn hash_of(files: &[SkillFile], dir: &Path) -> io::Result<String> {
    let mut hash = Sha256::new();
    for file in files {
        hash.update(file.rel.as_os_str().as_encoded_bytes());
        hash.update([0]);
        if file.link {
            hash.update(std::fs::read_link(dir.join(&file.rel))?.as_os_str().as_encoded_bytes());
        } else {
            let mut input = open_nofollow(&dir.join(&file.rel))?;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
        }
        hash.update([0]);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn open_nofollow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW).open(path)
}
