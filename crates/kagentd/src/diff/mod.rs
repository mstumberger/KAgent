//! The diff the daemon computes (diff.md): for two texts (a journal change) and for a git repository. The web part only
//! renders it. `lines.rs` is the line diff, `git.rs` the repository, `http.rs` the routes.

pub mod git;
pub mod http;
pub mod lines;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_git;

use serde::Serialize;

/// A file over this many bytes is summarised, not diffed (diff.md section 1).
pub const MAX_FILE_BYTES: usize = 1_000_000;
/// Files in one answer; the rest set `truncated`.
pub const MAX_FILES: usize = 500;
/// Lines of context around a change.
pub const CONTEXT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Ctx,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    pub kind: Kind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
    /// `[start, end)` in characters of `text` that differ from the paired line.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<[usize; 2]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub header: String,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub status: Status,
    pub binary: bool,
    pub too_large: bool,
    /// `None` for a binary file and for a text too large to count.
    pub added: Option<usize>,
    pub removed: Option<usize>,
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    pub fn summary(path: &str, status: Status) -> Self {
        Self { path: path.into(), old_path: None, status, binary: false, too_large: false, added: Some(0), removed: Some(0), hunks: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diff {
    pub files: Vec<FileDiff>,
    pub added: usize,
    pub removed: usize,
    pub truncated: bool,
}

impl Diff {
    pub fn of(files: Vec<FileDiff>, truncated: bool) -> Self {
        let added = files.iter().filter_map(|f| f.added).sum();
        let removed = files.iter().filter_map(|f| f.removed).sum();
        Self { files, added, removed, truncated }
    }
}
