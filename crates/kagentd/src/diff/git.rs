//! A git repository's diff (diff.md section 4): the refs are checked, git runs through the one exec helper (read-only),
//! and its unified output is parsed into the model.

use std::path::Path;
use std::time::Duration;

use kagent_core::tool::ToolContext;
use kagent_core::tools::exec::exec;

use super::lines::mark_ranges;
use super::{Diff, FileDiff, Hunk, Kind, Line, Status, MAX_FILES, MAX_FILE_BYTES};

/// What is read of git's output at most (the exec helper keeps the end of what it captures, four times its limit).
const READ_BYTES: usize = 8_000_000;

/// Why a diff could not be made: the HTTP status class and the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// 400: a ref or a project that is not accepted.
    Invalid(String),
    /// 409: the folder is not a git repository.
    NotARepo(String),
    /// 500: git itself failed.
    Failed(String),
}

/// The arguments before the subcommand: no pager, no optional locks, and nothing a repository's own configuration could
/// make git run (`core.fsmonitor`, an external diff driver).
fn base(project: &Path) -> Vec<String> {
    ["git", "--no-pager", "--no-optional-locks", "-c", "core.quotepath=false", "-c", "core.fsmonitor=false", "-C"].iter().map(|s| s.to_string()).chain([project.to_string_lossy().to_string()]).collect()
}

fn ctx() -> ToolContext {
    // The helper keeps the tail of at most four times this: 2 MB x 4 = READ_BYTES.
    ToolContext::standalone(Duration::from_secs(20), READ_BYTES / 4)
}

async fn git(project: &Path, args: &[&str]) -> anyhow::Result<kagent_core::tools::exec::ExecResult> {
    let mut argv = base(project);
    argv.extend(args.iter().map(|s| s.to_string()));
    exec(&ctx(), &argv).run().await
}

/// Is `project` inside a git work tree?
pub async fn is_repo(project: &Path) -> Result<(), GitError> {
    match git(project, &["rev-parse", "--is-inside-work-tree"]).await {
        Ok(r) if r.success() && r.stdout.trim() == "true" => Ok(()),
        Ok(_) => Err(GitError::NotARepo(format!("{} is not a git repository", project.display()))),
        Err(e) => Err(GitError::Failed(format!("git did not run: {e:#}"))),
    }
}

/// A ref that resolves to a commit.
async fn resolve(project: &Path, which: &str, r: &str) -> Result<(), GitError> {
    crate::gitref::check_ref(r).map_err(|e| GitError::Invalid(format!("{which}: {e}")))?;
    let spec = format!("{r}^{{commit}}");
    match git(project, &["rev-parse", "--verify", "--quiet", "--end-of-options", &spec]).await {
        Ok(res) if res.success() => Ok(()),
        Ok(_) => Err(GitError::Invalid(format!("{which}: unknown ref {r:?}"))),
        Err(e) => Err(GitError::Failed(format!("git did not run: {e:#}"))),
    }
}

/// The changes from `from` to `to`, or, with no `to`, of the working tree against `from` (untracked files are not part of
/// it). Read-only: `rev-parse` and `diff` only.
pub async fn diff(project: &Path, from: &str, to: Option<&str>) -> Result<Diff, GitError> {
    is_repo(project).await?;
    resolve(project, "from", from).await?;
    if let Some(to) = to {
        resolve(project, "to", to).await?;
    }
    let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color", "-U3"];
    args.push("-M");
    args.push(from);
    if let Some(to) = to {
        args.push(to);
    }
    args.push("--");
    let out = git(project, &args).await.map_err(|e| GitError::Failed(format!("git did not run: {e:#}")))?;
    if !out.success() {
        return Err(GitError::Failed(format!("git diff failed: {}", out.stderr.trim())));
    }
    Ok(parse(&out.stdout, out.stdout.len() >= READ_BYTES))
}

/// The path of a `diff --git a/X b/Y` line when both sides are the same file (no rename): the halves are equal.
fn same_path(rest: &str) -> Option<String> {
    let body = rest.strip_prefix("a/")?;
    let half = (body.len().checked_sub(3)?) / 2;
    let (left, right) = (body.get(..half)?, body.get(half + 3..)?);
    (body.get(half..half + 3) == Some(" b/") && left == right).then(|| left.to_string())
}

fn parse_range(s: &str) -> (usize, usize) {
    match s.split_once(',') {
        Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
        None => (s.parse().unwrap_or(0), 1),
    }
}

/// `@@ -a,b +c,d @@ header`.
fn hunk_head(line: &str) -> Option<Hunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, header) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let ((old_start, old_lines), (new_start, new_lines)) = (parse_range(old), parse_range(new));
    Some(Hunk { old_start, old_lines, new_start, new_lines, header: header.trim().to_string(), lines: Vec::new() })
}

/// Parse git's unified output. `cut`: the output was capped, so what comes before the first `diff --git` is dropped and
/// the answer says it is incomplete.
pub fn parse(output: &str, cut: bool) -> Diff {
    let start = if cut { output.find("\ndiff --git ").map_or(output.len(), |i| i + 1) } else { 0 };
    let mut sections: Vec<&str> = Vec::new();
    let mut at: Option<usize> = None;
    let body = &output[start..];
    for (i, _) in body.match_indices("diff --git ") {
        if i == 0 || body.as_bytes()[i - 1] == b'\n' {
            if let Some(a) = at {
                sections.push(&body[a..i]);
            }
            at = Some(i);
        }
    }
    if let Some(a) = at {
        sections.push(&body[a..]);
    }
    let mut truncated = cut;
    let mut files = Vec::new();
    for section in sections {
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        files.push(parse_file(section));
    }
    Diff::of(files, truncated)
}

fn parse_file(section: &str) -> FileDiff {
    let mut lines = section.lines();
    let first = lines.next().unwrap_or_default();
    let rest = first.strip_prefix("diff --git ").unwrap_or_default();
    let mut path = same_path(rest).unwrap_or_default();
    let (mut old_path, mut status, mut binary) = (None::<String>, Status::Modified, false);
    let (mut minus, mut plus) = (None::<String>, None::<String>);
    let mut rename_to = None::<String>;
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut old_no, mut new_no) = (0usize, 0usize);
    let (mut added, mut removed) = (0usize, 0usize);
    let too_large = section.len() > MAX_FILE_BYTES;
    for line in lines {
        if let Some(h) = line.strip_prefix("@@ ").and(hunk_head(line)) {
            (old_no, new_no) = (h.old_start, h.new_start);
            hunks.push(h);
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            // The header of the file.
            if line.starts_with("new file mode") {
                status = Status::Added;
            } else if line.starts_with("deleted file mode") {
                status = Status::Deleted;
            } else if let Some(p) = line.strip_prefix("rename from ") {
                old_path = Some(p.to_string());
                status = Status::Renamed;
            } else if let Some(p) = line.strip_prefix("rename to ") {
                rename_to = Some(p.to_string());
            } else if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
                binary = true;
            } else if let Some(p) = line.strip_prefix("--- ") {
                minus = Some(p.strip_prefix("a/").unwrap_or(p).to_string());
            } else if let Some(p) = line.strip_prefix("+++ ") {
                plus = Some(p.strip_prefix("b/").unwrap_or(p).to_string());
            }
            continue;
        };
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => (Kind::Add, &line[1..]),
            Some(b'-') => (Kind::Del, &line[1..]),
            Some(b' ') => (Kind::Ctx, &line[1..]),
            Some(b'\\') => continue, // "\ No newline at end of file"
            _ => (Kind::Ctx, line),
        };
        let text = text.strip_suffix('\r').unwrap_or(text).to_string();
        match kind {
            Kind::Add => added += 1,
            Kind::Del => removed += 1,
            Kind::Ctx => {}
        }
        if too_large {
            continue;
        }
        let row = match kind {
            Kind::Ctx => {
                let r = Line { kind, old: Some(old_no), new: Some(new_no), text, ranges: Vec::new() };
                old_no += 1;
                new_no += 1;
                r
            }
            Kind::Del => {
                let r = Line { kind, old: Some(old_no), new: None, text, ranges: Vec::new() };
                old_no += 1;
                r
            }
            Kind::Add => {
                let r = Line { kind, old: None, new: Some(new_no), text, ranges: Vec::new() };
                new_no += 1;
                r
            }
        };
        hunk.lines.push(row);
    }
    if let Some(to) = rename_to {
        path = to;
    } else if path.is_empty() {
        path = match status {
            Status::Deleted => minus.clone(),
            _ => plus.clone().or(minus.clone()),
        }
        .unwrap_or_default();
    }
    let mut file = FileDiff::summary(&path, status);
    file.old_path = old_path;
    if binary {
        file.binary = true;
        file.added = None;
        file.removed = None;
    } else {
        file.added = Some(added);
        file.removed = Some(removed);
        if too_large {
            file.too_large = true;
        } else {
            for h in &mut hunks {
                mark_ranges(&mut h.lines);
            }
            file.hunks = hunks;
        }
    }
    file
}
