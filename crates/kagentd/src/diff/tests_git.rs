//! The git side on temporary repositories the tests make (no network, nothing outside the temporary folders).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use super::git::{diff, GitError};
use super::http::{known_project, repo_diff};
use super::Status;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// A repository with two commits: the second modifies, deletes, renames, adds, changes a binary and adds a big file.
fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = std::fs::canonicalize(dir.path()).unwrap();
    git(&p, &["init", "-q", "-b", "main"]);
    let w = |name: &str, text: &str| std::fs::write(p.join(name), text).unwrap();
    w("a.txt", "one\ntwo\nthree\n");
    w("b.txt", "bye\n");
    w("c.txt", "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n");
    std::fs::write(p.join("pic.bin"), b"\x00\x01\x02first").unwrap();
    git(&p, &["add", "."]);
    git(&p, &["commit", "-q", "-m", "first"]);
    w("a.txt", "one\n2\nthree\n");
    std::fs::remove_file(p.join("b.txt")).unwrap();
    git(&p, &["mv", "c.txt", "d.txt"]);
    w("d.txt", "line 1\nline 2\nline 3\nline 4\nline 5\nline SIX\n");
    w("e.txt", "brand new\n");
    std::fs::write(p.join("pic.bin"), b"\x00\x01\x02second").unwrap();
    w("big.txt", &"a line of text that is long enough\n".repeat(40_000));
    git(&p, &["add", "-A"]);
    git(&p, &["commit", "-q", "-m", "second"]);
    (dir, p)
}

fn index_state(p: &Path) -> (Vec<u8>, String, String) {
    (std::fs::read(p.join(".git/index")).unwrap(), git(p, &["rev-parse", "HEAD"]), git(p, &["status", "--porcelain"]))
}

#[tokio::test]
async fn two_refs_give_every_status_and_the_counts() {
    let (_d, p) = repo();
    let before = index_state(&p);
    let d = diff(&p, "HEAD~1", Some("HEAD")).await.unwrap();
    let by = |name: &str| d.files.iter().find(|f| f.path == name).unwrap_or_else(|| panic!("{name} in {:?}", d.files.iter().map(|f| &f.path).collect::<Vec<_>>()));
    let a = by("a.txt");
    assert_eq!((a.status, a.added, a.removed), (Status::Modified, Some(1), Some(1)));
    assert_eq!(a.hunks[0].lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one", "two", "2", "three"]);
    let b = by("b.txt");
    assert_eq!((b.status, b.removed), (Status::Deleted, Some(1)));
    let r = by("d.txt");
    assert_eq!((r.status, r.old_path.as_deref(), r.added, r.removed), (Status::Renamed, Some("c.txt"), Some(1), Some(1)));
    assert_eq!(by("e.txt").status, Status::Added);
    let bin = by("pic.bin");
    assert!(bin.binary && bin.hunks.is_empty() && bin.added.is_none());
    let big = by("big.txt");
    assert!(big.too_large && big.hunks.is_empty() && big.added == Some(40_000), "{:?} {:?}", big.too_large, big.added);
    assert_eq!(d.files.len(), 6);
    assert_eq!(index_state(&p), before, "a diff changes nothing in the repository");
}

#[tokio::test]
async fn the_working_tree_against_a_ref() {
    let (_d, p) = repo();
    std::fs::write(p.join("a.txt"), "one\n2\nthree\nfour\n").unwrap();
    let before = index_state(&p);
    let d = diff(&p, "HEAD", None).await.unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!((d.files[0].path.as_str(), d.files[0].added, d.files[0].removed), ("a.txt", Some(1), Some(0)));
    let d = diff(&p, "HEAD~1", None).await.unwrap();
    assert!(d.files.iter().any(|f| f.path == "a.txt"), "against an older commit: everything since");
    assert_eq!(index_state(&p).1, before.1);
    assert_eq!(index_state(&p).0, before.0, "the index is not even refreshed");
}

#[tokio::test]
async fn a_ref_that_could_be_an_option_or_is_unknown_is_refused_and_git_never_sees_it() {
    let (_d, p) = repo();
    let target = p.join("pwned.txt");
    for bad in ["--output=pwned.txt", "-x", "HEAD..HEAD~1", "a b", "$(touch pwned.txt)", "", "main;touch pwned.txt"] {
        match diff(&p, bad, None).await {
            Err(GitError::Invalid(_)) => {}
            other => panic!("{bad:?}: {other:?}"),
        }
        assert!(matches!(diff(&p, "HEAD", Some(bad)).await, Err(GitError::Invalid(_))), "to {bad:?}");
    }
    assert!(!target.exists());
    match diff(&p, "no-such-branch", None).await {
        Err(GitError::Invalid(m)) => assert!(m.contains("unknown ref"), "{m}"),
        other => panic!("{other:?}"),
    }
    match diff(&p, "HEAD", Some("v9.9")).await {
        Err(GitError::Invalid(m)) => assert!(m.starts_with("to:"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_folder_that_is_not_a_repository_is_a_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let p = std::fs::canonicalize(dir.path()).unwrap();
    assert!(matches!(diff(&p, "HEAD", None).await, Err(GitError::NotARepo(_))));
    let known = vec![p.clone()];
    let e = repo_diff(&json!({"project": p, "from": "HEAD"}), &known).await.unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_project_must_be_one_of_the_list() {
    let (_d, p) = repo();
    let (_d2, other) = repo();
    let known = vec![p.clone()];
    let e = repo_diff(&json!({"project": other, "from": "HEAD~1", "to": "HEAD"}), &known).await.unwrap_err();
    assert_eq!(e.0, axum::http::StatusCode::BAD_REQUEST);
    assert!(e.2.contains("not a known project"), "{}", e.2);
    assert!(known_project("relative/path", &known).is_err());
    assert!(known_project("/no/such/folder", &known).is_err());
    let v = repo_diff(&json!({"project": p, "from": "HEAD~1", "to": "HEAD"}), &known).await.unwrap();
    assert_eq!(v["diff"]["files"].as_array().unwrap().len(), 6);
    assert_eq!(v["from"], "HEAD~1");
    // A link to the folder is the same folder.
    let link = p.parent().unwrap().join("link-to-repo");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert!(known_project(&link.to_string_lossy(), &known).is_ok());
    std::fs::remove_file(link).unwrap();
    let e = repo_diff(&json!({"project": p, "from": "HEAD", "extra": 1}), &known).await.unwrap_err();
    assert!(e.2.contains("unknown field"), "{}", e.2);
}

#[tokio::test]
async fn a_repositorys_own_configuration_runs_nothing() {
    let (_d, p) = repo();
    let marker = p.join("ran.txt");
    git(&p, &["config", "core.fsmonitor", &format!("touch {}; echo", marker.display())]);
    git(&p, &["config", "diff.external", &format!("touch {}", marker.display())]);
    std::fs::write(p.join("a.txt"), "changed\n").unwrap();
    diff(&p, "HEAD", None).await.unwrap();
    assert!(!marker.exists(), "no external diff and no fsmonitor hook");
}
