//! The command lines of git for a source (skills.md section 8), word for word, and the
//! one way they run: no prompts, no hooks, output kept to what an error needs.

use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// How long one git command may take, a clone over the network included.
const GIT_TIMEOUT: Duration = Duration::from_secs(180);

// ------------------------------------------------------------------ git

/// Every git call starts with these: hooks are disabled, and a local transport stays
/// what the user's own configuration allows (never `always`).
fn git_base() -> Vec<String> {
    vec!["git".into(), "-c".into(), "core.hooksPath=/dev/null".into(), "-c".into(), "protocol.file.allow=user".into()]
}

/// The clone of a source (skills.md section 8), word for word. `ref` empty: the default branch.
pub fn clone_argv(url: &str, dir: &Path, r#ref: &str) -> Vec<String> {
    let mut argv = git_base();
    argv.extend(["clone", "--depth", "1"].map(String::from));
    if !r#ref.is_empty() {
        argv.extend(["--branch", r#ref].map(String::from));
    }
    argv.extend(["--", url, &dir.display().to_string()].map(String::from));
    argv
}

/// What a sync runs first: the tip of `ref` (of the default branch when empty) is fetched, shallow.
pub fn fetch_argv(dir: &Path, r#ref: &str) -> Vec<String> {
    let mut argv = git_base();
    argv.extend(["-C", &dir.display().to_string(), "fetch", "--depth", "1", "origin", if r#ref.is_empty() { "HEAD" } else { r#ref }].map(String::from));
    argv
}

/// Then that tip is checked out, whatever the clone held.
pub fn checkout_argv(dir: &Path) -> Vec<String> {
    let mut argv = git_base();
    argv.extend(["-C", &dir.display().to_string(), "checkout", "--force", "--detach", "FETCH_HEAD"].map(String::from));
    argv
}

/// The commit that is checked out.
pub fn head_argv(dir: &Path) -> Vec<String> {
    let mut argv = git_base();
    argv.extend(["-C", &dir.display().to_string(), "rev-parse", "HEAD"].map(String::from));
    argv
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// `git` is not installed.
    Missing,
    /// The command failed; the end of what it said says why.
    Failed(String),
    /// Still running when its time was up.
    TimedOut,
}

/// Run one git command line. Nothing it prints on success but the last line of its
/// output is kept; a failure keeps the end of its stderr. Output is never shown to
/// anything else, and nothing of the repository is executed (no hooks, no prompts).
pub async fn git(argv: &[String]) -> Result<String, GitError> {
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C.UTF-8")
        .kill_on_drop(true);
    // A process group of its own: a clone that runs out of time goes with its helpers.
    command.process_group(0);
    let child = command.spawn().map_err(|e| if e.kind() == io::ErrorKind::NotFound { GitError::Missing } else { GitError::Failed(e.to_string()) })?;
    let pid = child.id();
    match tokio::time::timeout(GIT_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).trim().to_string()),
        Ok(Ok(out)) => Err(GitError::Failed(tail(&out.stderr))),
        Ok(Err(e)) => Err(GitError::Failed(e.to_string())),
        Err(_) => {
            if let Some(pid) = pid {
                let _ = std::process::Command::new("kill").args(["-KILL", &format!("-{pid}")]).status();
            }
            Err(GitError::TimedOut)
        }
    }
}

/// The end of what git said, on one line.
pub fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).split_whitespace().collect::<Vec<_>>().join(" ");
    text.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect()
}

