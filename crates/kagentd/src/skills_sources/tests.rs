use super::*;
use crate::skills::Installed;
use kagent_core::skills::SkillSource;
use super::read::{MAX_CLONE_BYTES, MAX_CLONE_FILES, MAX_SYNC_SKILLS};
use crate::skills_import::{skill_files, Skipped};
use super::validate::{repo_name, NewSource};
use std::os::unix::fs::PermissionsExt;

/// SHA-256 of a skill folder.
fn tree_hash(dir: &Path) -> String {
    crate::skills_import::hash_of(&skill_files(dir).unwrap(), dir).unwrap()
}

fn skill(root: &Path, name: &str, body: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: skill {name}\n---\n\n{body}\n")).unwrap();
    dir
}

fn names_of(items: &[Skipped]) -> Vec<&str> {
    items.iter().map(|s| s.name.as_str()).collect()
}

fn found_names(list: &[Found]) -> Vec<&str> {
    list.iter().map(|f| f.name.as_str()).collect()
}

const DIR: &str = "/data/skill-sources/superpowers-8c6014a3";
const BASE: [&str; 5] = ["git", "-c", "core.hooksPath=/dev/null", "-c", "protocol.file.allow=user"];

fn record(id: &str, url: &str, skills: Vec<(&str, String)>) -> SourceRecord {
    SourceRecord {
        id: id.into(),
        name: "repo".into(),
        url: url.into(),
        r#ref: String::new(),
        subdir: String::new(),
        commit: None,
        last_sync: None,
        last_error: None,
        skills: skills.into_iter().map(|(name, hash)| SkillRecord { name: name.into(), path: PathBuf::from("/source").join(name), hash, synced_at: "t0".into() }).collect(),
    }
}

#[test]
fn the_file_round_trips_and_a_damaged_file_is_an_error() {
    // skills.md section 11: parse of skill-sources.json round trip.
    let home = tempfile::tempdir().unwrap();
    let (path, clones) = (home.path().join("config/skill-sources.json"), home.path().join("data/skill-sources"));
    let store = SourceStore::load(&path, &clones).unwrap();
    assert!(store.list().is_empty() && store.provenance("x").is_none(), "a missing file is an empty one");

    let superpowers = SourceRecord {
        id: "superpowers-8c6014a3".into(),
        name: "Superpowers".into(),
        url: "https://github.com/obra/superpowers".into(),
        r#ref: "main".into(),
        subdir: "skills".into(),
        commit: Some("0f3a5c9e".into()),
        last_sync: Some("2026-11-25T10:00:00+00:00".into()),
        last_error: None,
        skills: vec![SkillRecord { name: "brainstorming".into(), path: clones.join("superpowers-8c6014a3/skills/brainstorming"), hash: "ab".repeat(32), synced_at: "2026-11-25T10:00:00+00:00".into() }],
    };
    store.put(&superpowers).unwrap();
    let zeta = ImportRecord { name: "zeta".into(), source: "ZCode".into(), path: PathBuf::from("/home/me/.zcode/skills/zeta"), hash: "cd".repeat(32) };
    store.note_import(&[zeta.clone()]).unwrap();

    let store = SourceStore::load(&path, &clones).unwrap();
    assert_eq!(store.list(), [superpowers.clone()], "what was saved is what loads");
    assert_eq!(store.get("superpowers-8c6014a3"), Some(superpowers.clone()));
    let from = |source: &str, path: PathBuf, hash: String| Some(ImportedFrom { source: source.into(), path, hash });
    assert_eq!(store.provenance("brainstorming"), from("Superpowers", superpowers.skills[0].path.clone(), "ab".repeat(32)));
    assert_eq!(store.provenance("zeta"), from("ZCode", zeta.path.clone(), "cd".repeat(32)));
    assert_eq!(store.clone_dir("superpowers-8c6014a3"), clones.join("superpowers-8c6014a3"));
    assert!(store.imported_by_hand("zeta") && !store.imported_by_hand("brainstorming"));

    // camelCase in the file, and the mode of a config file.
    let raw = std::fs::read_to_string(&path).unwrap();
    for key in ["\"schemaVersion\": 1", "\"lastSync\":", "\"syncedAt\":", "\"ref\": \"main\""] {
        assert!(raw.contains(key), "{key} missing in {raw}");
    }
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "0600");
    let view = serde_json::to_value(view_of(&superpowers)).unwrap();
    assert_eq!(view["skills"][0], serde_json::json!({"name": "brainstorming", "hash": "ab".repeat(32), "syncedAt": "2026-11-25T10:00:00+00:00"}));
    assert_eq!((view["ref"].as_str(), view["commit"].as_str()), (Some("main"), Some("0f3a5c9e")));
    assert!(view.get("lastError").is_none(), "an empty optional field is left out");

    // A source that installs a name owns it from then on: the note of the hand-made import goes.
    let takes = record("other-1a2b3c4d", "/tmp/repo", vec![("zeta", "00".repeat(32))]);
    store.put(&takes).unwrap();
    assert!(!store.imported_by_hand("zeta"));
    assert_eq!(store.provenance("zeta").map(|p| p.source), Some("repo".into()));
    store.remove(&takes.id).unwrap();
    assert_eq!(store.list(), [superpowers.clone()]);

    // Damaged: an error that names the file, never an empty store over the owner's sources.
    std::fs::write(&path, "{not json").unwrap();
    let error = SourceStore::load(&path, &clones).unwrap_err();
    assert!(error.contains("skill-sources.json"), "{error}");
    std::fs::write(&path, "{\"sources\": [{\"id\": \"../escape\", \"name\": \"x\", \"url\": \"https://x/y\"}]}").unwrap();
    assert!(SourceStore::load(&path, &clones).unwrap_err().contains("not a valid id"));
    std::fs::write(&path, "{\"sorces\": []}").unwrap();
    assert!(SourceStore::load(&path, &clones).is_err(), "a misspelt key is refused");
}

#[test]
fn git_runs_the_command_lines_of_the_spec_word_for_word() {
    // skills.md sections 8 and 11: argv built for clone, fetch and checkout.
    let dir = Path::new(DIR);
    let mut with_branch = BASE.to_vec();
    with_branch.extend(["clone", "--depth", "1", "--branch", "main", "--", "https://github.com/obra/superpowers", DIR]);
    assert_eq!(clone_argv("https://github.com/obra/superpowers", dir, "main"), with_branch);
    let mut default_branch = BASE.to_vec();
    default_branch.extend(["clone", "--depth", "1", "--", "https://github.com/anthropics/skills", DIR]);
    assert_eq!(clone_argv("https://github.com/anthropics/skills", dir, ""), default_branch, "no --branch for the default branch");
    let mut fetch = BASE.to_vec();
    fetch.extend(["-C", DIR, "fetch", "--depth", "1", "origin", "main"]);
    assert_eq!(fetch_argv(dir, "main"), fetch);
    let mut fetch_default = BASE.to_vec();
    fetch_default.extend(["-C", DIR, "fetch", "--depth", "1", "origin", "HEAD"]);
    assert_eq!(fetch_argv(dir, ""), fetch_default, "no ref: the default branch");
    let mut checkout = BASE.to_vec();
    checkout.extend(["-C", DIR, "checkout", "--force", "--detach", "FETCH_HEAD"]);
    assert_eq!(checkout_argv(dir), checkout);
    let mut head = BASE.to_vec();
    head.extend(["-C", DIR, "rev-parse", "HEAD"]);
    assert_eq!(head_argv(dir), head);
}

#[test]
fn invalid_urls_and_refs_are_refused_before_any_command_runs() {
    // Scenario S21 item 6; skills.md section 11.
    for bad in [
        "",
        "ftp://example.com/skills",
        "http://github.com/obra/superpowers",
        "github.com/obra/superpowers",
        "skills",
        "relative/../skills",
        "https://user:secret@github.com/obra/superpowers",
        "https://github.com/obra/skills --upload-pack=evil",
        "https://github.com/",
        "ssh://git@github.com/obra/superpowers",
        "file:///tmp/repo",
        "/home/nobody/no-such-folder",
    ] {
        assert!(validate(Some(bad), None, None, None).is_err(), "{bad} must be refused");
    }
    for bad in ["--upload-pack=touch /tmp/x", "-x", "a..b", "a//b", "/main", "main/", ".hidden", "a/.b", "x y", "a.lock", "v1."] {
        let refused = validate(Some("https://github.com/obra/superpowers"), Some(bad), None, None).unwrap_err();
        assert!(refused.contains("ref must be"), "{bad:?}: {refused}");
    }
    for bad in ["../skills", "skills/../../etc", "skills//x", "a/./b", "skills\\x", "."] {
        let refused = validate(Some("https://github.com/obra/superpowers"), None, Some(bad), None).unwrap_err();
        assert!(refused.contains("subdir must be"), "{bad:?}: {refused}");
    }
    assert!(validate(Some("https://github.com/obra/superpowers"), None, None, Some(&"n".repeat(81))).is_err(), "a name of at most 80 characters");

    // What is accepted, and what it is read as.
    let ok = validate(Some(" https://github.com/obra/superpowers/ "), Some(" main "), Some("/skills/ "), Some(" Superpowers ")).unwrap();
    assert_eq!(ok, NewSource { url: "https://github.com/obra/superpowers".into(), r#ref: "main".into(), subdir: "skills".into(), name: "Superpowers".into() });
    assert_eq!(
        validate(Some("https://github.com/anthropics/skills.git"), None, None, None).unwrap(),
        NewSource { url: "https://github.com/anthropics/skills.git".into(), r#ref: String::new(), subdir: String::new(), name: "skills".into() },
        "the name defaults to the repository's"
    );
    assert!(validate(Some("https://github.com/openai/skills"), Some("0f3a5c9e1b2d4a6f8c0e2b4d6a8f0c1e3b5d7a9f"), Some("skills/.curated"), None).is_ok(), "a commit, and a subfolder that starts with a dot");
    assert_eq!(validate(Some("https://github.com/x/y"), None, Some("/etc"), None).unwrap().subdir, "etc", "a subfolder is always inside the repository");
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("my skills");
    std::fs::create_dir_all(&folder).unwrap();
    let local = validate(Some(&folder.display().to_string()), None, None, None).unwrap();
    assert_eq!((local.url.clone(), local.name.as_str()), (std::fs::canonicalize(&folder).unwrap().display().to_string(), "my skills"));
    assert!(!is_local(&ok.url) && is_local(&local.url));

    // The id of an address never changes, is that of no other address, and is a safe name.
    assert_eq!(id_for("https://github.com/obra/superpowers"), "superpowers-8c6014a3");
    assert_eq!(id_for(&format!("{}/", "https://github.com/obra/superpowers")), "superpowers-8c6014a3", "a trailing slash is the same address");
    let (anthropic, openai) = (id_for("https://github.com/anthropics/skills"), id_for("https://github.com/openai/skills"));
    assert!(anthropic.starts_with("skills-") && openai.starts_with("skills-") && anthropic != openai, "{anthropic} vs {openai}");
    assert!(id_for(&local.url).starts_with("my-skills-"));
    assert!(id_for("https://github.com/.git").starts_with("source-"), "an address that names no folder");
    for url in ["https://github.com/-/-", "https://github.com/obra/superpowers", "https://github.com/Anthropics/Skills", "/tmp/a b"] {
        let id = id_for(url);
        assert!(valid_name(&id) && id.len() <= 41, "{url}: {id}");
    }
}

#[tokio::test]
async fn the_sync_plan_comes_from_the_hashes_and_install_makes_the_folder_match() {
    // Scenario S21 item 5, the plan; skills.md section 11.
    let home = tempfile::tempdir().unwrap();
    let (repo, imported_root) = (home.path().join("repo"), home.path().join("config/skills-imported"));
    for name in ["alpha", "beta", "delta", "epsilon", "zeta", "eta"] {
        skill(&repo, name, "first");
    }
    // Copies as an earlier sync left them: alpha (since changed in the source), beta (unchanged), gamma (since gone from the source).
    std::fs::create_dir_all(&imported_root).unwrap();
    for name in ["alpha", "beta", "gamma"] {
        std::fs::create_dir_all(imported_root.join(name)).unwrap();
        std::fs::write(imported_root.join(name).join("SKILL.md"), format!("---\nname: {name}\ndescription: old\n---\nOld copy of {name}.\n")).unwrap();
    }
    let old = record("repo-1a2b3c4d", &repo.display().to_string(), vec![("alpha", "0".repeat(64)), ("beta", tree_hash(&repo.join("beta"))), ("gamma", "1".repeat(64))]);
    std::fs::write(repo.join("alpha/SKILL.md"), "---\nname: alpha\ndescription: second\n---\nSecond body.\n").unwrap();
    // zeta: an interrupted sync left a copy with the content of the source; eta: a copy of something else has the name.
    std::fs::create_dir_all(imported_root.join("zeta")).unwrap();
    std::fs::copy(repo.join("zeta/SKILL.md"), imported_root.join("zeta/SKILL.md")).unwrap();
    std::fs::create_dir_all(imported_root.join("eta")).unwrap();
    std::fs::write(imported_root.join("eta/SKILL.md"), "---\nname: eta\ndescription: elsewhere\n---\nSomeone else's eta.\n").unwrap();

    let (found, unreadable) = scan(&repo, "").unwrap();
    assert!(unreadable.is_empty());
    assert_eq!(found_names(&found), ["alpha", "beta", "delta", "epsilon", "eta", "zeta"], "by name");
    let mine = [Installed { name: "epsilon".into(), source: SkillSource::User, real_dir: PathBuf::from("/config/skills/epsilon") }];
    let taken = |name: &str| mine.iter().any(|s| s.name == name);
    let plan = plan_sync(&old, &found, &imported_root, &taken);
    assert_eq!(found_names(&plan.added), ["delta"], "new in the source");
    assert_eq!(found_names(&plan.updated), ["alpha"], "its hash changed");
    assert_eq!(plan.removed, ["gamma"], "gone from the source");
    assert_eq!(found_names(&plan.adopted), ["zeta"], "the same content is in place already: only recorded");
    assert_eq!(names_of(&plan.skipped), ["epsilon", "eta"], "a name a skill has, and a name another copy has");
    assert!(plan.skipped.iter().all(|s| s.reason == "name exists"));

    // A copy the owner removed by hand comes back.
    std::fs::remove_dir_all(imported_root.join("beta")).unwrap();
    let back = plan_sync(&old, &found, &imported_root, &taken);
    assert_eq!(found_names(&back.added), ["beta", "delta"]);
    std::fs::create_dir_all(imported_root.join("beta")).unwrap();
    std::fs::copy(repo.join("beta/SKILL.md"), imported_root.join("beta/SKILL.md")).unwrap();

    let mut now = old.clone();
    assert!(install(&plan, &mut now, &imported_root).await.is_empty(), "everything was done");
    assert_eq!(std::fs::read_to_string(imported_root.join("alpha/SKILL.md")).unwrap(), "---\nname: alpha\ndescription: second\n---\nSecond body.\n", "updated: the new content");
    assert!(imported_root.join("delta/SKILL.md").is_file(), "added");
    assert!(!imported_root.join("gamma").exists(), "removed");
    assert_eq!(std::fs::read_to_string(imported_root.join("eta/SKILL.md")).unwrap(), "---\nname: eta\ndescription: elsewhere\n---\nSomeone else's eta.\n", "never overwritten");
    let mut recorded: Vec<(&str, &str)> = now.skills.iter().map(|s| (s.name.as_str(), s.path.file_name().unwrap().to_str().unwrap())).collect();
    recorded.sort();
    assert_eq!(recorded, [("alpha", "alpha"), ("beta", "beta"), ("delta", "delta"), ("zeta", "zeta")], "gamma is out, delta and zeta are in");
    assert_eq!(now.skills.iter().find(|s| s.name == "alpha").unwrap().hash, found.iter().find(|f| f.name == "alpha").unwrap().hash, "the record holds the new hash");
    assert!(["alpha", "delta", "zeta"].iter().all(|name| now.skills.iter().find(|s| s.name == *name).unwrap().synced_at.starts_with("20")), "what was installed now says when");
    assert_eq!(now.skills.iter().find(|s| s.name == "beta").unwrap().synced_at, "t0", "an unchanged skill keeps when it was installed");

    // Again: nothing left to do.
    let again = plan_sync(&now, &found, &imported_root, &taken);
    assert!(again.added.is_empty() && again.updated.is_empty() && again.removed.is_empty() && again.adopted.is_empty(), "stable");
    assert_eq!(names_of(&again.skipped), ["epsilon", "eta"]);
}

#[test]
fn a_source_is_read_strictly_and_within_its_limits() {
    // skills.md sections 1, 8 and 10: two levels deep, real folders only, at most 100 skills, a link never leads out.
    let home = tempfile::tempdir().unwrap();
    let (repo, outside) = (home.path().join("repo"), home.path().join("outside"));
    skill(&repo.join("docs"), "top-level", "a category folder that is a skill itself");
    skill(&repo.join("docs/guides"), "nested", "two levels deep");
    skill(&repo.join("docs/guides/deeper/still"), "too-deep", "three levels deep: not found");
    skill(&outside, "outside", "elsewhere on the disk");
    skill(&repo.join("docs/again"), "nested", "the same name as another skill of the source");
    std::fs::create_dir_all(repo.join("docs/linked-file")).unwrap();
    std::fs::create_dir_all(repo.join("docs/broken")).unwrap();
    std::fs::write(repo.join("docs/broken/SKILL.md"), "---\nname: Not_Valid\ndescription: d\n---\n\nb\n").unwrap();
    skill(&repo.join("docs"), "big", "b");
    std::fs::File::create(repo.join("docs/big/blob.bin")).unwrap().set_len(MAX_CLONE_BYTES + 1).unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("docs/link-to-outside")).unwrap();
    std::os::unix::fs::symlink(outside.join("SKILL.md"), repo.join("docs/linked-file/SKILL.md")).unwrap();
    std::os::unix::fs::symlink(outside, repo.join("linked-subdir")).unwrap();

    let (found, skipped) = scan(&repo, "docs").unwrap();
    assert_eq!(found_names(&found), ["nested", "top-level"], "a linked folder, a linked file and a third level are not read");
    let mut why: Vec<(&str, &str)> = skipped.iter().map(|s| (s.name.as_str(), s.reason.as_str())).collect();
    why.sort();
    assert_eq!(why, [("big", "more than 20 MB"), ("broken", "its frontmatter name is not a valid id"), ("nested", "listed twice in the source")], "{why:?}");
    assert!(subdir_path(&repo, "linked-subdir/x").is_err(), "a linked folder is not a folder of the source");
    assert!(subdir_path(&repo, "nope").is_err() && subdir_path(&repo, "").unwrap() == repo);

    // More than 100 skills: an error, none is picked.
    for i in 0..=MAX_SYNC_SKILLS {
        skill(&repo.join("many"), &format!("s{i:03}"), "b");
    }
    let error = scan(&repo, "many").unwrap_err();
    assert!(error.contains("101 skills") && error.contains("100"), "{error}");

    // The limits on what is read: 200 MB and 2000 files (sparse files: no disk is used).
    assert_eq!(check_limits(&repo.join("docs")).unwrap_err(), "the source is 201 MB, more than the 200 MB limit");
    assert!(check_limits(&repo.join("many")).is_ok(), "101 small folders");
    std::fs::create_dir_all(repo.join("lots")).unwrap();
    for i in 0..=MAX_CLONE_FILES {
        std::fs::write(repo.join("lots").join(format!("f{i:05}")), "").unwrap();
    }
    assert_eq!(check_limits(&repo.join("lots")).unwrap_err(), "the source has 2001 files, more than the 2000 file limit");
}

#[test]
fn the_catalog_holds_the_three_sources_of_the_spec() {
    // skills.md section 8.
    let rows: Vec<(&str, &str, &str)> = catalog().iter().map(|c| (c.name.as_str(), c.url.as_str(), c.subdir.as_str())).collect();
    assert_eq!(
        rows,
        [
            ("Anthropic skills", "https://github.com/anthropics/skills", "skills"),
            ("Superpowers", "https://github.com/obra/superpowers", "skills"),
            ("OpenAI skills", "https://github.com/openai/skills", "skills/.curated"),
        ]
    );
    for entry in catalog() {
        let checked = validate(Some(&entry.url), None, Some(&entry.subdir), None).unwrap_or_else(|e| panic!("{}: {e}", entry.name));
        assert_eq!(checked.name, repo_name(&entry.url), "{}: its default name", entry.name);
        assert!(!entry.note.is_empty());
    }
}

/// Only ever a throwaway repository under `/tmp/kagent-s21`.
fn git_in(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-C", &dir.display().to_string(), "-c", "user.name=kagent-test", "-c", "user.email=kagent-test@example.invalid", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "init.defaultBranch=main", "-c", "advice.defaultBranchName=false"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} in {}", dir.display());
}

#[tokio::test]
#[ignore = "needs the git program and a throwaway repository under /tmp/kagent-s21: run with --ignored"]
async fn real_git_clones_fetches_and_checks_out_with_the_command_lines_of_the_spec() {
    std::fs::create_dir_all("/tmp/kagent-s21").unwrap();
    let home = tempfile::Builder::new().prefix("git-it-").tempdir_in("/tmp/kagent-s21").unwrap();
    let repo = home.path().join("repo");
    skill(&repo.join("skills"), "alpha", "First body.");
    skill(&repo.join("skills"), "beta", "Beta body.");
    git_in(&repo, &["init", "-q"]);
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "first"]);
    let clones = home.path().join("data/skill-sources");
    std::fs::create_dir_all(&clones).unwrap();
    let store = SourceStore::load(home.path().join("config/skill-sources.json"), &clones).unwrap();
    let dir = store.clone_dir(&id_for("https://example.invalid/obra/superpowers"));

    // The clone line, over the file transport: the one way to clone without the network.
    git(&clone_argv(&format!("file://{}", repo.display()), &dir, "main")).await.unwrap();
    assert!(dir.join(".git/shallow").is_file(), "a shallow clone");
    assert_eq!(git(&head_argv(&dir)).await.unwrap().len(), 40);
    assert_eq!(found_names(&scan(&dir, "skills").unwrap().0), ["alpha", "beta"]);

    // A hook planted in the clone never runs.
    let hook = dir.join(".git/hooks/post-checkout");
    std::fs::write(&hook, "#!/bin/sh\ntouch hook-ran\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

    // A change upstream, then the sync lines.
    std::fs::write(repo.join("skills/alpha/SKILL.md"), "---\nname: alpha\ndescription: second\n---\nSecond body.\n").unwrap();
    std::fs::remove_dir_all(repo.join("skills/beta")).unwrap();
    git_in(&repo, &["add", "-A"]);
    git_in(&repo, &["commit", "-q", "-m", "second"]);
    git(&fetch_argv(&dir, "main")).await.unwrap();
    git(&checkout_argv(&dir)).await.unwrap();
    assert!(!dir.join("hook-ran").exists(), "hooks are disabled");
    assert!(std::fs::read_to_string(dir.join("skills/alpha/SKILL.md")).unwrap().contains("Second body."));
    assert!(!dir.join("skills/beta").exists(), "a file gone upstream is gone after the checkout");
    assert!(dir.join(".git/shallow").is_file(), "still shallow after the fetch");
    assert_eq!(git(&head_argv(&dir)).await.unwrap(), String::from_utf8_lossy(&std::process::Command::new("git").args(["-C", &repo.display().to_string(), "rev-parse", "HEAD"]).output().unwrap().stdout).trim().to_string(), "the commit of the source");

    // Without the option, git would run the hook: the test above proves something.
    std::process::Command::new("git").args(["-C", &dir.display().to_string(), "checkout", "--force", "--detach", "HEAD"]).status().unwrap();
    assert!(dir.join("hook-ran").exists(), "the hook works when hooks are not disabled");
}
