use super::*;
use kagent_core::skills::SkillRoot;
use crate::skills_api::quarantine_all;
use kagent_core::tool::{Tool, ToolContext};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

const INVALID: &str = "invalid_input";

fn skill(root: &Path, name: &str, description: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n\nBody of {name}.\n")).unwrap();
    dir
}

/// SHA-256 of a skill folder.
fn tree_hash(dir: &Path) -> String {
    skills_import::hash_of(&skills_import::skill_files(dir).unwrap(), dir).unwrap()
}

/// A hub over a bundled folder, the imported folder and the owner's folder, with its
/// store under `<home>/config`: what the routes work with, without the server's state.
struct Rig {
    home: tempfile::TempDir,
    hub: SkillHub,
    store: Arc<SourceStore>,
}

impl Rig {
    fn imported_root(&self) -> PathBuf {
        self.hub.root(SkillSource::Imported)
    }

    /// The library reads `disabled`, as the config closure does before a turn.
    fn library_with(&self, disabled: &[String]) {
        self.hub.library().write().unwrap().refresh_if_changed(&quarantine_all(&[], disabled));
    }
}

fn rig() -> Rig {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config");
    skill(&config.join("bundled"), "shipped", "KAgent ships with it");
    skill(&config.join("skills"), "mine", "written by the owner");
    std::fs::create_dir_all(config.join("skills-imported")).unwrap();
    let roots = [
        SkillRoot { path: config.join("bundled"), source: SkillSource::Bundled },
        SkillRoot { path: config.join("skills-imported"), source: SkillSource::Imported },
        SkillRoot { path: config.join("skills"), source: SkillSource::User },
    ];
    let store = Arc::new(SourceStore::load(config.join("skill-sources.json"), home.path().join("data/skill-sources")).unwrap());
    let hub = SkillHub::new(&roots, &[], broadcast::channel(16).0, home.path().join("data/skill-usage.json")).with_sources(store.clone());
    Rig { home, hub, store }
}

/// What the `import` handler does, in its order: plan, switch the new skills off, apply,
/// then the library reads the list a turn would.
async fn run_import(rig: &Rig, body: Value) -> Result<(Value, Vec<String>), Refusal> {
    let writers = rig.store.lock().await;
    let step = import_plan(&rig.hub, &body)?;
    let disabled = step.to_disable();
    let outcome = step.apply(&writers, &rig.hub, &rig.store, rig.home.path()).await?;
    let off = outcome.imported.clone();
    rig.library_with(&disabled);
    Ok((json!({ "imported": outcome.imported, "skipped": outcome.skipped, "disabled": off }), disabled))
}

#[tokio::test]
async fn import_copies_or_links_and_says_what_was_skipped() {
    // Scenario S21 items 2 and 3, the part of the daemon; skills.md sections 7 and 10.
    let rig = rig();
    let tool = rig.home.path().join("tool/skills");
    skill(&tool, "one", "the first");
    skill(&tool, "two", "the second");
    skill(&tool, "mine", "a name the owner has");
    skill(&tool, "big", "over the limit");
    std::fs::File::create(tool.join("big/blob.bin")).unwrap().set_len(skills_import::MAX_SKILL_BYTES + 1).unwrap();
    let root = tool.display().to_string();

    // What is refused is a 400 and writes nothing.
    for (body, said) in [
        (json!({ "names": ["one"] }), "missing field"),
        (json!({ "path": "tool/skills", "names": ["one"] }), "absolute path"),
        (json!({ "path": root }), "names"),
        (json!({ "path": root, "names": ["one"], "mode": "move" }), "copy or symlink"),
        (json!({ "path": root, "names": ["../one"] }), "not a valid skill name"),
        (json!({ "path": root, "names": ["one"], "extra": 1 }), "expected an object"),
        (json!({ "path": root, "names": "one" }), "expected an object"),
    ] {
        let refusal = import_plan(&rig.hub, &body).err().unwrap_or_else(|| panic!("{body} must be refused"));
        assert_eq!((refusal.0, refusal.1), (StatusCode::BAD_REQUEST, INVALID), "{body}: {}", refusal.2);
        assert!(refusal.2.contains(said), "{body}: {}", refusal.2);
    }
    assert!(std::fs::read_dir(rig.imported_root()).unwrap().next().is_none(), "a refused request writes nothing");

    // Copy: two arrive, switched off before the first file is written; three are skipped with why.
    let (answer, disabled) = run_import(&rig, json!({ "path": root, "names": ["one", "two", "mine", "big", "nope"] })).await.unwrap();
    assert_eq!(disabled, ["one", "two"]);
    assert_eq!(
        answer,
        json!({
            "imported": ["one", "two"],
            "disabled": ["one", "two"],
            "skipped": [
                { "name": "mine", "reason": "name exists" },
                { "name": "big", "reason": "more than 20 MB" },
                { "name": "nope", "reason": "not found" }
            ]
        })
    );
    assert!(rig.imported_root().join("one/SKILL.md").is_file() && rig.imported_root().join("two/SKILL.md").is_file());
    assert!(!rig.imported_root().join("mine").exists() && !rig.imported_root().join("big").exists());

    // Arrived off: out of the prompt index, refused by `load_skill`. The detail says where they came from.
    let one = rig.hub.view("one", false).unwrap();
    assert!((one.source, one.enabled, one.symlink) == (SkillSource::Imported, false, false));
    let from = one.imported_from.clone().unwrap();
    assert_eq!(from.source, root, "a folder that is no known tool's is named by its path");
    assert_eq!(from.path, std::fs::canonicalize(tool.join("one")).unwrap());
    assert_eq!(from.hash, tree_hash(&tool.join("one")));
    let index = rig.hub.index(3000);
    assert!(!index.contains("\n- one:") && !index.contains("\n- two:") && index.contains("\n- mine:"), "{index}");
    let refused = rig.hub.tool().call(json!({ "name": "one" }), ToolContext::standalone(Duration::from_secs(5), 1 << 20)).await.unwrap();
    assert!(refused.is_error && refused.content.contains("disabled"), "{}", refused.content);
    assert!(rig.hub.view("mine", false).unwrap().imported_from.is_none(), "a skill of the owner does not say it was imported");

    // Link: the folder of the other tool is not copied; deleting the skill removes the link only.
    skill(&tool, "linked", "to be linked");
    let (answer, disabled) = run_import(&rig, json!({ "path": root, "names": ["linked"], "mode": "symlink" })).await.unwrap();
    assert_eq!((answer["imported"].clone(), disabled), (json!(["linked"]), vec!["linked".to_string()]));
    let linked = rig.hub.view("linked", false).unwrap();
    assert!(linked.symlink && linked.real_path == std::fs::canonicalize(tool.join("linked")).unwrap());
    crate::skills_api::delete(&rig.hub, "linked").await.unwrap();
    assert!(!std::fs::symlink_metadata(rig.imported_root().join("linked")).is_ok() && tool.join("linked/SKILL.md").is_file());

    // What is installed is not imported again, whatever the mode.
    let (answer, disabled) = run_import(&rig, json!({ "path": root, "names": ["one", "two"], "mode": "symlink" })).await.unwrap();
    assert!(disabled.is_empty() && answer["imported"] == json!([]) && answer["skipped"].as_array().unwrap().len() == 2, "{answer}");
}

#[tokio::test]
async fn a_local_source_is_added_synced_after_a_change_and_removed() {
    // Scenario S21 items 4 (a local folder: a clone needs the network) and 5; skills.md sections 8 and 10.
    let rig = rig();
    let repo = rig.home.path().join("repo");
    for name in ["alpha", "beta"] {
        skill(&repo.join("skills"), name, "first");
    }
    skill(&repo.join("skills"), "mine", "a name the owner has");
    skill(&repo.join("skills"), "shipped", "a name a skill of KAgent has");
    std::fs::write(repo.join("README.md"), "not a skill").unwrap();
    let writers = rig.store.lock().await;
    let skipped = |name: &str| Skipped { name: name.into(), reason: "name exists".into() };
    let untouched = vec![skipped("mine"), skipped("shipped")];

    // Add: read in place (no clone), and everything new is switched off before it is installed.
    let step = add_step(&rig.hub, &rig.store, &writers, Some(&format!("{}/", repo.display())), None, Some("/skills/"), None).await.unwrap();
    let (id, disabled) = (step.id().to_string(), step.to_disable());
    assert_eq!(disabled, ["alpha", "beta"]);
    let (saved, report) = step.apply(&writers, &rig.hub, &rig.store).await.unwrap();
    assert_eq!(report, SyncReport { added: vec!["alpha".into(), "beta".into()], updated: vec![], removed: vec![], skipped: untouched.clone() });
    let real = std::fs::canonicalize(&repo).unwrap().display().to_string();
    assert_eq!((saved.id.as_str(), saved.name.as_str(), saved.subdir.as_str(), saved.url.as_str()), (id.as_str(), "repo", "skills", real.as_str()));
    assert!(saved.commit.is_none() && saved.last_error.is_none() && saved.last_sync.is_some(), "a local path has no commit");
    assert_eq!(rig.store.list(), [saved.clone()], "saved");
    assert!(!rig.home.path().join("data").exists(), "a local path is read in place: nothing is cloned");
    rig.library_with(&disabled);
    assert!(!rig.hub.view("alpha", false).unwrap().enabled && !rig.hub.view("beta", false).unwrap().enabled);
    let alpha = rig.hub.view("alpha", false).unwrap();
    assert_eq!(alpha.imported_from, Some(sources::ImportedFrom { source: "repo".into(), path: std::fs::canonicalize(repo.join("skills/alpha")).unwrap(), hash: saved.skills[0].hash.clone() }));

    // The same folder again is a 409; a sync with nothing changed does nothing.
    let again = add_step(&rig.hub, &rig.store, &writers, Some(&repo.display().to_string()), None, Some("skills"), None).await.err().unwrap();
    assert_eq!((again.0, again.1), (StatusCode::CONFLICT, "conflict"));
    let step = sync_step(&rig.hub, &rig.store, &writers, &id).await.unwrap();
    assert!(step.to_disable().is_empty());
    let (_, quiet) = step.apply(&writers, &rig.hub, &rig.store).await.unwrap();
    assert_eq!(quiet, SyncReport { added: vec![], updated: vec![], removed: vec![], skipped: untouched.clone() });

    // The owner switches alpha on. The source changes: a sync updates alpha (off again), removes beta, adds gamma (off).
    let on = crate::skills_api::disabled_after(&disabled, "alpha", true);
    rig.library_with(&on);
    assert!(rig.hub.view("alpha", false).unwrap().enabled, "the owner switched it on");
    std::fs::write(repo.join("skills/alpha/SKILL.md"), "---\nname: alpha\ndescription: second\n---\nSecond body.\n").unwrap();
    std::fs::remove_dir_all(repo.join("skills/beta")).unwrap();
    skill(&repo.join("skills"), "gamma", "new in the source");
    let step = sync_step(&rig.hub, &rig.store, &writers, &id).await.unwrap();
    let disabled = step.to_disable();
    assert_eq!(disabled, ["gamma", "alpha"], "the new and the changed one: added first, as planned");
    let (saved, report) = step.apply(&writers, &rig.hub, &rig.store).await.unwrap();
    assert_eq!(report, SyncReport { added: vec!["gamma".into()], updated: vec!["alpha".into()], removed: vec!["beta".into()], skipped: untouched.clone() });
    assert_eq!(std::fs::read_to_string(rig.imported_root().join("alpha/SKILL.md")).unwrap(), "---\nname: alpha\ndescription: second\n---\nSecond body.\n");
    assert!(!rig.imported_root().join("beta").exists() && rig.imported_root().join("gamma/SKILL.md").is_file());
    let mut names: Vec<&str> = saved.skills.iter().map(|s| s.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["alpha", "gamma"]);
    rig.library_with(&quarantine_all(&on, &disabled));
    assert!(!rig.hub.view("alpha", false).unwrap().enabled, "changed content is new content: off again");
    assert!(!rig.hub.view("gamma", false).unwrap().enabled);

    // Remove, keeping the skills: the copies stay; no source says any more where they came from.
    remove_step(&rig.hub, &rig.store, &writers, &id, false).await.unwrap();
    assert!(rig.store.list().is_empty());
    assert!(rig.hub.view("alpha", false).unwrap().imported_from.is_none() && rig.imported_root().join("alpha/SKILL.md").is_file());

    // Added again, the copies that are there with the same content are kept (not reported as new).
    let step = add_step(&rig.hub, &rig.store, &writers, Some(&repo.display().to_string()), None, Some("skills"), None).await.unwrap();
    assert!(step.to_disable().is_empty(), "nothing new to switch off");
    let (_, report) = step.apply(&writers, &rig.hub, &rig.store).await.unwrap();
    assert!(report.added.is_empty() && report.removed.is_empty(), "{report:?}");
    assert_eq!(rig.store.list()[0].skills.len(), 2, "alpha and gamma are recorded");

    // Removed with its skills: only the copies go. The local folder is never touched.
    remove_step(&rig.hub, &rig.store, &writers, &id, true).await.unwrap();
    assert!(rig.store.list().is_empty());
    assert!(!rig.imported_root().join("alpha").exists() && !rig.imported_root().join("gamma").exists());
    assert!(repo.join("skills/alpha/SKILL.md").is_file() && repo.join("skills/gamma/SKILL.md").is_file() && repo.join("README.md").is_file(), "the local folder is not removed");
    assert!(rig.hub.view("mine", false).unwrap().source == SkillSource::User && rig.hub.view("shipped", false).unwrap().source == SkillSource::Bundled);
}

#[tokio::test]
async fn removing_a_source_with_its_skills_leaves_a_skill_of_the_owner_that_took_a_name() {
    let rig = rig();
    let repo = rig.home.path().join("repo");
    skill(&repo, "alpha", "first");
    let writers = rig.store.lock().await;
    let id = add_step(&rig.hub, &rig.store, &writers, Some(&repo.display().to_string()), None, None, None).await.unwrap().apply(&writers, &rig.hub, &rig.store).await.unwrap().0.id;
    assert_eq!(rig.store.list()[0].skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["alpha"], "installed at the root of the folder");

    // The owner deletes the copy and writes a skill of their own with the name.
    crate::skills_api::delete(&rig.hub, "alpha").await.unwrap();
    skill(&rig.home.path().join("config/skills"), "alpha", "written by the owner");
    rig.hub.reload();
    assert_eq!(rig.hub.view("alpha", false).unwrap().source, SkillSource::User);
    remove_step(&rig.hub, &rig.store, &writers, &id, true).await.unwrap();
    assert!(rig.hub.view("alpha", false).unwrap().source == SkillSource::User && rig.home.path().join("config/skills/alpha/SKILL.md").is_file(), "the owner's skill stays");
    assert!(rig.store.list().is_empty());
}

#[tokio::test]
async fn refused_source_requests_change_nothing_and_an_unknown_source_is_404() {
    // Scenario S21 item 6 on the route; skills.md section 8.
    let rig = rig();
    let writers = rig.store.lock().await;
    for (url, r#ref, subdir) in [
        ("ftp://x/y", None, None),
        ("https://github.com/obra/superpowers", Some("--upload-pack=touch /tmp/x"), None),
        ("https://github.com/x/y", None, Some("../etc")),
        ("/no/such/folder", None, None),
    ] {
        let refusal = add_step(&rig.hub, &rig.store, &writers, Some(url), r#ref, subdir, None).await.err().unwrap_or_else(|| panic!("{url} must be refused"));
        assert_eq!((refusal.0, refusal.1), (StatusCode::BAD_REQUEST, INVALID), "{refusal:?}");
    }
    assert!(rig.store.list().is_empty() && !rig.home.path().join("data").exists(), "no record and no clone folder: refused before any command ran");
    for id in ["nope", "../etc"] {
        let missing = sync_step(&rig.hub, &rig.store, &writers, id).await.err().unwrap();
        assert_eq!((missing.0, missing.1), (StatusCode::NOT_FOUND, "not_found"));
        let missing = remove_step(&rig.hub, &rig.store, &writers, id, true).await.err().unwrap();
        assert_eq!((missing.0, missing.1), (StatusCode::NOT_FOUND, "not_found"));
    }
    note_failure(&rig.store, "nope", "git failed");
    assert!(rig.store.list().is_empty(), "a failure is recorded for a source that exists only");
}

#[tokio::test]
async fn a_missing_git_is_named_and_a_failed_one_is_upstream() {
    // skills.md section 8: git missing -> "install git".
    let refusal = sources::git(&["/nonexistent/kagent-git".to_string(), "--version".to_string()]).await.map_err(git_refusal).err().unwrap();
    assert_eq!((refusal.0, refusal.1), (StatusCode::INTERNAL_SERVER_ERROR, "internal"));
    assert!(refusal.2.starts_with("install git"), "{}", refusal.2);
    let failed = git_refusal(GitError::Failed("fatal: repository not found".into()));
    assert_eq!((failed.0, failed.1), (StatusCode::BAD_GATEWAY, UPSTREAM));
    assert!(failed.2.contains("repository not found"), "{}", failed.2);
    assert_eq!(git_refusal(GitError::TimedOut).1, UPSTREAM);
    assert_eq!(sources::tail(&[b'x'; 1000]).len(), 300, "the end of what git said is what is kept");
}

#[test]
fn a_failed_write_is_reported_and_not_counted_as_done() {
    let found = |name: &str| sources::Found { name: name.into(), dir: PathBuf::from("/source").join(name), hash: "h".repeat(64) };
    let plan = SyncPlan { added: vec![found("a")], updated: vec![], removed: vec!["c".into()], adopted: vec![], skipped: vec![] };
    let failed = Skipped { name: "c".into(), reason: "cannot be removed: busy".into() };
    let report = SyncReport::of(&plan, vec![failed.clone()]);
    assert_eq!(serde_json::to_value(&report).unwrap(), json!({"added": ["a"], "updated": [], "removed": [], "skipped": [{"name": "c", "reason": "cannot be removed: busy"}]}));
    assert_eq!((report.added, report.removed), (vec!["a".to_string()], Vec::<String>::new()));
    assert_eq!(report.skipped, [failed]);
}

#[test]
fn quarantine_adds_each_name_once_and_keeps_the_rest() {
    // skills.md section 11: skills of a sync land in skills.disabled.
    assert_eq!(quarantine_all(&[], &["a".to_string()]), ["a"]);
    assert_eq!(quarantine_all(&["x".into(), "a".into()], &["a".to_string(), "b".into(), "b".to_string()]), ["x", "a", "b"]);
    assert_eq!(quarantine_all(&["x".into()], &[]), ["x"]);
}
