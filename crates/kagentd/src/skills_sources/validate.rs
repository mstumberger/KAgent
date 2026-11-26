//! What a new source may be (skills.md section 8), checked before any command runs: the
//! address, the `ref`, the `subdir` and the name, and the id a source gets.

use std::path::Path;

use sha2::{Digest, Sha256};

// ------------------------------------------------------------------ a new source

/// `POST /api/skills/sources`, checked: nothing of it can be an option to git, or lead out of the clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSource {
    /// Canonical: an `https://` address without a trailing slash, or the real path of a folder.
    pub url: String,
    pub r#ref: String,
    pub subdir: String,
    pub name: String,
}

/// An `https://` address is cloned; anything else (an absolute path) is read where it is.
pub fn is_local(url: &str) -> bool {
    !url.starts_with("https://")
}

/// Check what adding a source sends, before any command runs. `Err` is the 400.
pub fn validate(url: Option<&str>, r#ref: Option<&str>, subdir: Option<&str>, name: Option<&str>) -> Result<NewSource, String> {
    let given = url.unwrap_or("").trim().trim_end_matches('/');
    if given.is_empty() {
        return Err("url is required: an https:// address, or the absolute path of a folder".into());
    }
    let bad_url = "the URL must start with https:// and name a repository (with no password in it), or be the absolute path of a folder";
    let url = if Path::new(given).is_absolute() {
        let dir = std::fs::canonicalize(given).map_err(|_| format!("{given} is not a folder"))?;
        if !dir.is_dir() {
            return Err(format!("{given} is not a folder"));
        }
        dir.display().to_string()
    } else {
        let uri = given.parse::<axum::http::Uri>().map_err(|_| bad_url.to_string())?;
        let names_repository = !uri.path().trim_end_matches('/').rsplit('/').next().unwrap_or("").is_empty();
        if uri.scheme_str() != Some("https") || uri.host().is_none() || uri.authority().is_some_and(|a| a.as_str().contains('@')) || !names_repository {
            return Err(bad_url.into());
        }
        given.to_string()
    };
    let r#ref = r#ref.unwrap_or("").trim();
    if !r#ref.is_empty() && !valid_ref(r#ref) {
        return Err(format!("ref must be a branch, a tag or a commit (letters, digits, '.', '_', '/' and '-'), not '{ref}'"));
    }
    let subdir = subdir.unwrap_or("").trim().trim_matches('/').to_string();
    if !subdir.is_empty() && !valid_subdir(&subdir) {
        return Err(format!("subdir must be a folder inside the repository, written from its root (as skills or skills/.curated), not '{subdir}'"));
    }
    let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        None => repo_name(&url),
        Some(n) if n.chars().count() <= 80 && !n.chars().any(char::is_control) => n.to_string(),
        Some(_) => return Err("name has at most 80 characters, on one line".into()),
    };
    Ok(NewSource { url, r#ref: r#ref.to_string(), subdir, name })
}

/// Letters, digits, `.`, `_`, `/` and `-`: a branch, a tag or a commit. So a `ref` can
/// never be an option of git (`--upload-pack=<command>` would run a command).
fn valid_ref(r: &str) -> bool {
    // Parts as git names them (`git check-ref-format`), and none starts with a dash: a
    // part starts with a letter or a digit, ends neither in `.lock` nor in a dot, and is not empty.
    let part = |p: &str| p.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && !p.ends_with(".lock") && !p.ends_with('.');
    !r.is_empty() && r.len() <= 128 && !r.contains("..") && !r.contains("//") && r.split('/').all(part)
        && r.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

fn valid_subdir(s: &str) -> bool {
    !s.contains(['\\', '\0']) && !s.chars().any(char::is_control) && s.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The last folder of the address, without `.git`: the default name of a source.
pub fn repo_name(url: &str) -> String {
    let name = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").trim_end_matches(".git");
    if name.is_empty() { "source".into() } else { name.to_string() }
}

/// The id of a source: its repository's name and 8 signs of its address, so the same
/// address is always the same id (`https://github.com/obra/superpowers` gives
/// `superpowers-8c6014a3`), and an id is a safe folder name and URL segment.
pub fn id_for(url: &str) -> String {
    let url = url.trim_end_matches('/');
    let slug: String = repo_name(url).chars().flat_map(char::to_lowercase).map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.trim_matches('-');
    let slug: String = slug.chars().take(32).collect();
    let digest = Sha256::digest(url.as_bytes());
    let signs: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() { format!("source-{signs}") } else { format!("{slug}-{signs}") }
}

