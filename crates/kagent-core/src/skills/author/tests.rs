//! skills.md section 11: the `skill_save` and `skill_delete` rows.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::skills::frontmatter;
use crate::skills::library::SkillRoot;
use crate::skills::SkillSource;
use crate::skills::{LoadSkill, SkillCounter};

struct Rig {
    dir: tempfile::TempDir,
    library: Arc<RwLock<SkillLibrary>>,
    save: SkillSave,
    delete: SkillDelete,
    /// How often the daemon was told that the library changed.
    changes: Arc<Mutex<usize>>,
}

impl Rig {
    fn root(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

fn skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n{body}")).unwrap();
}

/// Bundled `alpha`, `hidden` (disabled by name) and `skill-creator`; imported `gamma`;
/// the owner's `zeta`, with a second file in its folder.
fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let (bundled, imported, user) = (dir.path().join("bundled"), dir.path().join("skills-imported"), dir.path().join("skills"));
    skill(&bundled, "alpha", "first", "Alpha body");
    skill(&bundled, "hidden", "not for the model", "Hidden body");
    skill(&bundled, "skill-creator", "how to write a skill", "Creator body");
    skill(&imported, "gamma", "from elsewhere", "Gamma body");
    skill(&user, "zeta", "written by the owner", "Zeta body");
    std::fs::write(user.join("zeta/notes.md"), "more").unwrap();
    let roots = [
        SkillRoot { path: bundled, source: SkillSource::Bundled },
        SkillRoot { path: imported, source: SkillSource::Imported },
        SkillRoot { path: user.clone(), source: SkillSource::User },
    ];
    let library = Arc::new(RwLock::new(SkillLibrary::scan(&roots, &["hidden".to_string()])));
    let changes = Arc::new(Mutex::new(0));
    let on_change = {
        let (library, changes) = (library.clone(), changes.clone());
        Arc::new(move || {
            *changes.lock().unwrap() += 1;
            library.write().unwrap().reload();
        })
    };
    Rig {
        save: SkillSave::new(library.clone(), &user, on_change.clone()),
        delete: SkillDelete::new(library.clone(), &user, on_change),
        library,
        changes,
        dir,
    }
}

/// What the model would see: the tool's text, or the error of a call that could not
/// be parsed (the loop turns that into an error result too).
async fn call(tool: &dyn Tool, input: serde_json::Value) -> String {
    let out = match tool.call(input, ToolContext::standalone(Duration::from_secs(5), 1 << 20)).await {
        Ok(out) => out,
        Err(e) => return format!("ERR {e:#}"),
    };
    if out.is_error {
        format!("ERR {}", out.content)
    } else {
        format!("OK {}", out.content)
    }
}

async fn preview(tool: &dyn Tool, input: serde_json::Value) -> String {
    let ctx = ToolContext::standalone(Duration::from_secs(5), 1 << 20);
    tool.preview(&input, &ctx).await.expect("a preview")
}

fn exists(path: &Path) -> bool {
    path.exists()
}

#[tokio::test]
async fn save_refuses_reserved_case_clashing_oversized_and_invalid_input() {
    let r = rig();
    let long = "x".repeat(MAX_SAVE_BODY_BYTES + 1);
    let description = "d".repeat(frontmatter::MAX_DESCRIPTION_CHARS + 1);
    for (input, said) in [
        (json!({"name": "skill-creator", "description": "d", "body": "b"}), "KAgent ships with"),
        (json!({"name": "ALPHA", "description": "d", "body": "b"}), "differs only in case"),
        (json!({"name": "slow_machine", "description": "d", "body": "b"}), "not a valid skill name"),
        (json!({"name": "", "description": "d", "body": "b"}), "not a valid skill name"),
        (json!({"name": "big", "description": "d", "body": long}), "bytes, more than"),
        (json!({"name": "empty", "description": "d", "body": "  \n"}), "the body is empty"),
        (json!({"name": "wordy", "description": description, "body": "b"}), "characters, more than"),
    ] {
        let out = call(&r.save, input.clone()).await;
        assert!(out.starts_with("ERR") && out.contains(said), "{input}: {out}");
        let card = preview(&r.save, input.clone()).await;
        assert!(card.starts_with("Not possible:"), "{input}: {card}");
    }
    // `zeta` is the only folder of the owner's root: a refused save wrote nothing.
    assert_eq!(std::fs::read_dir(r.root("skills")).unwrap().count(), 1, "a refused save writes nothing");
    assert_eq!(*r.changes.lock().unwrap(), 0, "and does not count as a change");
    // The limits themselves pass.
    let at_limit = "x".repeat(MAX_SAVE_BODY_BYTES);
    assert!(call(&r.save, json!({"name": "big", "description": "d", "body": at_limit})).await.starts_with("OK"));
}

#[tokio::test]
async fn save_writes_a_skill_that_parses_back_and_loads_at_once() {
    let r = rig();
    let description = "Fix bluetooth: no sound after reconnect.\nSecond line of the description";
    let when = "when the headphones pair but stay silent";
    let body = "1. `bluetooth` info <mac>.\n2. Restart bluetooth.\n\nGood answer: names the device.";
    let out = call(&r.save, json!({"name": "bluetooth-fix", "description": description, "when_to_use": when, "body": body})).await;
    assert!(out.starts_with("OK Saved skill 'bluetooth-fix'") && out.contains("(new skill). The agent loads it later with load_skill, or the owner starts a message with /bluetooth-fix."), "{out}");

    let file = r.root("skills/bluetooth-fix/SKILL.md");
    let raw = std::fs::read_to_string(&file).unwrap();
    assert!(raw.starts_with("---\nname: bluetooth-fix\ndescription: |-\n  Fix bluetooth: no sound after reconnect.\n  Second line of the description\nwhen_to_use: when the headphones pair but stay silent\n---\n"), "{raw}");
    let doc = frontmatter::parse(&raw).unwrap();
    assert_eq!((doc.name.as_str(), doc.description.as_str(), doc.when_to_use.as_str(), doc.body.as_str()), ("bluetooth-fix", description, when, body));

    // The library was read again: the new skill is the owner's, and `load_skill` finds it in the same turn.
    assert_eq!(*r.changes.lock().unwrap(), 1, "the daemon is told once");
    let entry = r.library.read().unwrap().get("bluetooth-fix").cloned().expect("in the library");
    assert_eq!((entry.source, entry.enabled), (SkillSource::User, true));
    let load = LoadSkill::new(r.library.clone(), Arc::new(SkillCounter::load(r.root("data/skill-usage.json"))));
    assert_eq!(call(&load, json!({"name": "bluetooth-fix"})).await, format!("OK {body}"));
}

#[tokio::test]
async fn save_overwrites_only_a_user_skill_and_says_what_it_hides() {
    let r = rig();
    // A user skill named like a bundled one hides the bundled one; the bundled file is not touched.
    let out = call(&r.save, json!({"name": "alpha", "description": "mine now", "body": "My alpha"})).await;
    assert!(out.contains("hides the bundled skill of the same name"), "{out}");
    assert_eq!(std::fs::read_to_string(r.root("bundled/alpha/SKILL.md")).unwrap().contains("Alpha body"), true);
    assert!(matches!(r.library.read().unwrap().get("alpha"), Some(e) if e.source == SkillSource::User));
    assert!(r.library.read().unwrap().shadowed().iter().any(|e| e.name == "alpha" && e.source == SkillSource::Bundled));

    // Overwriting it says so, and keeps the folder's other file.
    let out = call(&r.save, json!({"name": "zeta", "description": "edited", "body": "New zeta"})).await;
    assert!(out.contains("replaced the skill that was there") && !out.contains("hides"), "{out}");
    assert!(exists(&r.root("skills/zeta/notes.md")));

    // A disabled name stays disabled.
    let out = call(&r.save, json!({"name": "hidden", "description": "mine", "body": "My hidden"})).await;
    assert!(out.contains("switched off in settings"), "{out}");
    assert!(r.library.read().unwrap().get("hidden").is_some_and(|e| !e.enabled));
    assert_eq!(*r.changes.lock().unwrap(), 3);

    // A skill without a description is saved, with a warning in the result.
    let out = call(&r.save, json!({"name": "bare", "description": "", "body": "b"})).await;
    assert!(out.contains("no description"), "{out}");
}

#[cfg(unix)]
#[tokio::test]
async fn save_writes_nothing_through_a_symlink() {
    let r = rig();
    std::fs::create_dir_all(r.root("skills")).unwrap();
    std::os::unix::fs::symlink(r.root("bundled/alpha"), r.root("skills/linked")).unwrap();
    let out = call(&r.save, json!({"name": "linked", "description": "d", "body": "through the link"})).await;
    assert!(out.contains("is a symlink"), "{out}");
    assert!(!std::fs::read_to_string(r.root("bundled/alpha/SKILL.md")).unwrap().contains("through the link"));
}

#[tokio::test]
async fn delete_refuses_bundled_imported_unknown_and_symlinked_names() {
    let r = rig();
    // Scenario S20 item 4, first half.
    let out = call(&r.delete, json!({"name": "alpha"})).await;
    assert!(out.starts_with("ERR skill 'alpha' is bundled. It comes from ") && out.contains("bundled/alpha") && out.contains("switch it off"), "{out}");
    let out = call(&r.delete, json!({"name": "gamma"})).await;
    assert!(out.starts_with("ERR skill 'gamma' is imported. It comes from ") && out.contains("remove it in Settings"), "{out}");
    assert!(call(&r.delete, json!({"name": "nope"})).await.starts_with("ERR unknown skill 'nope'"));
    assert!(call(&r.delete, json!({"name": "Bad_Name"})).await.contains("not a valid skill name"));

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(r.root("skills-imported/gamma"), r.root("skills/gamma-link")).unwrap();
        assert!(call(&r.delete, json!({"name": "gamma-link"})).await.contains("is a symlink"));
        assert!(exists(&r.root("skills-imported/gamma/SKILL.md")), "the link's target is untouched");
    }
    for name in ["alpha", "gamma", "nope"] {
        assert!(preview(&r.delete, json!({"name": name})).await.starts_with("Not possible:"), "{name}");
    }
    assert_eq!(*r.changes.lock().unwrap(), 0);
}

#[tokio::test]
async fn delete_removes_a_user_directory_with_everything_in_it() {
    let r = rig();
    // Scenario S20 item 4, second half.
    std::fs::create_dir_all(r.root("skills/zeta/scripts")).unwrap();
    std::fs::write(r.root("skills/zeta/scripts/run.sh"), "echo hi").unwrap();
    let card = preview(&r.delete, json!({"name": "zeta"})).await;
    assert!(card.starts_with("Delete skill 'zeta' and its folder") && card.contains("\n  SKILL.md (") && card.contains("\n  notes.md (") && card.contains("\n  scripts/run.sh (7 bytes)"), "{card}");

    let out = call(&r.delete, json!({"name": "zeta"})).await;
    assert!(out.starts_with(&format!("OK Deleted skill 'zeta' ({}).", r.root("skills/zeta").display())), "{out}");
    assert!(!exists(&r.root("skills/zeta")), "the directory is gone");
    assert!(r.library.read().unwrap().get("zeta").is_none());
    assert_eq!(*r.changes.lock().unwrap(), 1);
    assert!(preview(&r.delete, json!({"name": "zeta"})).await.starts_with("Not possible: unknown skill"));

    // Deleting the owner's copy of a bundled name brings the bundled skill back.
    call(&r.save, json!({"name": "alpha", "description": "mine", "body": "My alpha"})).await;
    let out = call(&r.delete, json!({"name": "alpha"})).await;
    assert!(out.ends_with("The bundled skill of that name is in use again."), "{out}");
    assert!(matches!(r.library.read().unwrap().get("alpha"), Some(e) if e.source == SkillSource::Bundled && e.body == "Alpha body"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_may_be_removed_as_a_link_and_leaves_its_target() {
    let r = rig();
    std::os::unix::fs::symlink(r.root("bundled/alpha"), r.root("skills-imported/alpha-link")).unwrap();
    let library = r.library.read().unwrap();
    let refused = plan_delete(&library, &r.root("skills-imported"), "alpha-link", false).unwrap_err();
    assert!(refused.contains("is a symlink"), "{refused}");
    let plan = plan_delete(&library, &r.root("skills-imported"), "alpha-link", true).unwrap();
    drop(library);
    assert!(plan.link && plan.files.is_empty());
    remove_skill(&plan).await.unwrap();
    assert!(!exists(&r.root("skills-imported/alpha-link")), "the link is gone");
    assert!(exists(&r.root("bundled/alpha/SKILL.md")), "the target is not");
}

#[tokio::test]
async fn the_body_may_come_as_one_text_or_as_its_lines() {
    // tools.md section 17: an array of lines is joined; small models send it that way.
    let r = rig();
    let lines = json!(["## Fix bluetooth", "", "1. `bluetooth` info <mac>.", "2. Restart bluetooth."]);
    let out = call(&r.save, json!({"name": "as-lines", "description": "d", "body": lines})).await;
    assert!(out.starts_with("OK Saved skill 'as-lines'"), "{out}");
    let raw = std::fs::read_to_string(r.root("skills/as-lines/SKILL.md")).unwrap();
    assert!(raw.ends_with("---\n\n## Fix bluetooth\n\n1. `bluetooth` info <mac>.\n2. Restart bluetooth.\n"), "{raw}");
    let one = json!("## Fix bluetooth\n\n1. `bluetooth` info <mac>.\n2. Restart bluetooth.");
    call(&r.save, json!({"name": "as-lines", "description": "d", "body": one})).await;
    assert_eq!(std::fs::read_to_string(r.root("skills/as-lines/SKILL.md")).unwrap(), raw, "one text and its lines give the same file");

    // The card shows the same for both, and anything else is refused, not a crash.
    assert_eq!(preview(&r.save, json!({"name": "x", "description": "d", "body": lines})).await, preview(&r.save, json!({"name": "x", "description": "d", "body": one})).await);
    let schema = &r.save.contract().input_schema["properties"]["body"];
    assert!(schema.get("anyOf").is_some() && schema.get("type").is_none(), "the schema takes either form: {schema}");
}

#[tokio::test]
async fn a_body_that_is_neither_text_nor_lines_is_an_error_of_the_call() {
    let r = rig();
    let out = call(&r.save, json!({"name": "bad", "description": "d", "body": [1, 2]})).await;
    assert!(out.contains("every line must be text") || out.contains("invalid input"), "{out}");
    let out = call(&r.save, json!({"name": "bad", "description": "d", "body": 7})).await;
    assert!(out.starts_with("ERR") && out.contains("body"), "{out}");
    assert!(!exists(&r.root("skills/bad")));
}

#[tokio::test]
async fn the_save_card_shows_the_text() {
    let r = rig();
    let body: String = (1..=45).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
    let card = preview(&r.save, json!({"name": "bluetooth-fix", "description": "Fix bluetooth: no sound", "when_to_use": "headphones pair but stay silent", "body": body})).await;
    assert!(card.starts_with("New skill 'bluetooth-fix'\ndescription: Fix bluetooth: no sound\nwhen_to_use: headphones pair but stay silent\n\nline 1\n"), "{card}");
    assert!(card.contains("\nline 30\n") && !card.contains("line 31") && card.ends_with("... 15 more lines\n"), "{card}");
}
