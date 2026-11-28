//! The diff the reviewer is shown (workflows.md section 3.3): `git diff HEAD` of the task's files, read-only, no pager, no
//! configured helper programs.

use std::path::Path;
use std::process::Command;

const MAX_DIFF_BYTES: usize = 40_000;

fn git(project: &str) -> Command {
    let mut c = Command::new("git");
    c.env("GIT_OPTIONAL_LOCKS", "0")
        .args(["-C", project, "--no-pager", "--no-optional-locks", "-c", "core.quotepath=false", "-c", "core.fsmonitor=false"]);
    c
}

/// Paths relative to the project: an absolute one inside it is made relative, one outside is dropped.
pub fn relative(project: &str, files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter_map(|f| {
            let p = Path::new(f);
            if p.is_absolute() {
                p.strip_prefix(project).ok().map(|r| r.to_string_lossy().to_string())
            } else {
                Some(f.clone())
            }
        })
        .filter(|f| !f.is_empty() && !f.starts_with('-'))
        .collect()
}

/// The diff of `files` (all changes when empty) against the last commit, with the untracked files named; text for a reviewer.
pub fn diff(project: &str, files: &[String]) -> String {
    let inside = git(project).args(["rev-parse", "--is-inside-work-tree"]).output();
    if !inside.is_ok_and(|o| o.status.success()) {
        return "no git diff: the project is not a git repository; read the files".into();
    }
    let rel = relative(project, files);
    let mut cmd = git(project);
    cmd.args(["diff", "--no-ext-diff", "--no-textconv", "--no-color", "-M", "HEAD", "--"]);
    cmd.args(&rel);
    let mut out = match cmd.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        Ok(o) => format!("git diff failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
        Err(e) => format!("git could not run: {e}"),
    };
    let mut untracked = git(project);
    untracked.args(["ls-files", "--others", "--exclude-standard", "--"]);
    untracked.args(&rel);
    if let Ok(o) = untracked.output() {
        let names = String::from_utf8_lossy(&o.stdout).lines().take(30).collect::<Vec<_>>().join(", ");
        if !names.is_empty() {
            out.push_str(&format!("\nNew files not yet tracked by git (read them): {names}\n"));
        }
    }
    if out.len() > MAX_DIFF_BYTES {
        out = format!("{}\n(cut: the diff is longer than {MAX_DIFF_BYTES} bytes; read the files)", crate::util::truncate_str(&out, MAX_DIFF_BYTES));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_relative_and_outside_ones_go() {
        let out = relative("/p", &["/p/src/a.rs".into(), "/other/b.rs".into(), "c.rs".into(), "-rf".into()]);
        assert_eq!(out, ["src/a.rs", "c.rs"]);
    }

    #[test]
    fn a_folder_that_is_no_repository_says_so() {
        let dir = tempfile::tempdir().unwrap();
        assert!(diff(dir.path().to_str().unwrap(), &[]).starts_with("no git diff"));
    }
}
