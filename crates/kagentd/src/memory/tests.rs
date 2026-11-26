//! The store (memory.md section 9): the files, their permissions, the limits and the index.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use kagent_core::protocol::AgentEvent;
use serde_json::json;

use super::file;
use super::testing::{hub, hub_with, new};
use super::*;
use crate::settings::Settings;

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn read(hub: &MemoryHub, name: &str) -> String {
    std::fs::read_to_string(hub.store().path(name)).unwrap()
}

#[test]
fn first_use_creates_the_core_files_and_the_folder_is_private() {
    let (hub, dir, _events) = hub();
    let folder = dir.path().join("memory");
    assert_eq!(mode(&folder), 0o700, "a folder of 0600 could not be entered to read its files");
    assert!(!folder.join("user.md").exists(), "nothing is written before the first use");
    let (block, rule) = hub.prompt_parts(&MemorySection::default(), "", 0, true);
    assert!(block.ends_with("\nAbout me: The owner writes Slovenian or English: answer in that language.\n"), "{block}");
    assert!(rule, "the rule belongs in the prompt while memory is on and the model has the tools");
    assert_eq!(read(&hub, MACHINE), format!("{MACHINE_SEED}\n"));
    assert_eq!((mode(&folder.join("user.md")), mode(&folder.join("machine.md"))), (0o600, 0o600));
    assert_eq!(hub.core_text(2000), "The owner writes Slovenian or English: answer in that language.", "the comment of machine.md is not sent");

    // The persona decides what a new `user.md` says.
    let settings = Arc::new(RwLock::new(Settings::defaults()));
    settings.write().unwrap().patch(json!({"persona": {"address": "Marko", "language": "sl"}})).unwrap();
    let (hub, _dir, _events) = hub_with(Arc::new(OnceLock::from(settings)));
    hub.all();
    assert_eq!(read(&hub, USER), "Address the owner as Marko.\nThe owner wants answers in Slovenian.\n");
    // A file written by hand with wider rights is 0600 after the next start.
    let hand = hub.store().path("router");
    std::fs::write(&hand, "---\ntype: machine\n---\nMikroTik.\n").unwrap();
    std::fs::set_permissions(&hand, std::fs::Permissions::from_mode(0o644)).unwrap();
    hub.store().secure().unwrap();
    assert_eq!(mode(&hand), 0o600);
    assert_eq!(hub.get("router").map(|m| m.r#type), Some(MemoryType::Machine));
}

/// memory.md section 9: an invalid name is skipped.
#[test]
fn the_store_loads_what_is_a_memory_and_skips_the_rest() {
    let (hub, dir, _events) = hub();
    let folder = hub.store().dir().to_path_buf();
    file::write_private(&folder.join("router.md"), "---\nname: router\ndescription: home router\ntype: machine\ncreated: 2026-11-24\nupdated: 2026-11-24\n---\nMikroTik hEX.\n").unwrap();
    for skipped in ["Bad Name.md", "with_underscore.md", "Upper.md", "memory.md", "notes.txt", "router.md.bak"] {
        std::fs::write(folder.join(skipped), "a memory\n").unwrap();
    }
    std::fs::write(folder.join(INDEX), "not a memory").unwrap();
    std::fs::write(folder.join("broken.md"), [0xff, 0xfe, 0xfd]).unwrap();
    std::fs::write(folder.join("no-id.md"), "---\nname: Bad Name\n---\nbody\n").unwrap();
    std::fs::create_dir(folder.join("folder.md")).unwrap();
    std::fs::write(dir.path().join("outside.md"), "not through a link\n").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.md"), folder.join("link.md")).unwrap();

    let names: Vec<String> = hub.all().into_iter().map(|m| m.name.clone()).collect();
    assert_eq!(names, ["machine", "router", "user"], "by name; the index, a bad name, a link, a folder and what is not text stay out");
    assert_eq!(hub.get("router").map(|m| (m.description, m.body)), Some(("home router".into(), "MikroTik hEX.".into())));
    for name in ["link", "memory", "Upper", "no-id", "nope"] {
        assert!(hub.get(name).is_none(), "{name}");
    }
    assert!(hub.all().iter().all(|m| m.modified.is_some() && m.bytes > 0), "the loader stamps when the file was written and how big it is");
}

#[test]
fn a_save_writes_the_file_says_so_and_indexes_it() {
    let (hub, _dir, events) = hub();
    let mut rx = events.subscribe();
    let (memory, old) = hub.save(&new("printer", MemoryType::Machine, "  The printer is in the hallway. "), None, SaveMode::Overwrite).unwrap();
    assert!(old.is_none());
    assert_eq!((memory.source, memory.reviewed, memory.description.as_str(), memory.body.as_str()), (MemorySource::Agent, false, "The printer is in the hallway.", "The printer is in the hallway."), "the first line stands in for a missing description; the body is trimmed");
    let text = read(&hub, "printer");
    assert!(text.ends_with("\nsource: agent\nreviewed: false\n---\nThe printer is in the hallway.\n"), "{text}");
    assert!(text.contains(&format!("created: {}\nupdated: {}\n", today(), today())), "{text}");
    assert_eq!(mode(&hub.store().path("printer")), 0o600);
    let AgentEvent::MemoryChanged { name, removed } = rx.try_recv().unwrap() else { panic!("the change is said with memoryChanged") };
    assert_eq!((name.as_str(), removed), ("printer", false));
    assert!(rx.try_recv().is_err(), "one event per change");
    assert_eq!(serde_json::to_value(AgentEvent::MemoryChanged { name: "router".into(), removed: true }).unwrap(), json!({"type": "memoryChanged", "name": "router", "removed": true}), "api.md section 7");

    // The owner's memory is reviewed from the start; a save under a name that exists keeps `created`.
    let owner = NewMemory { source: MemorySource::User, ..new("printer", MemoryType::Reference, "In the study now.") };
    let (again, _) = hub.save(&owner, None, SaveMode::Overwrite).unwrap();
    assert_eq!((again.created, again.source, again.reviewed, again.r#type), (memory.created, MemorySource::User, true, MemoryType::Reference));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::MemoryChanged { name, removed: false }) if name == "printer"));
    let leftovers: Vec<String> = std::fs::read_dir(hub.store().dir()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".tmp")).collect();
    assert!(leftovers.is_empty(), "nothing is left over from the writes: {leftovers:?}");
}

/// memory.md section 9: `MEMORY.md` is rewritten from the files.
#[test]
fn memory_md_is_rewritten_from_the_files() {
    let (hub, _dir, _events) = hub();
    let index = || std::fs::read_to_string(hub.store().dir().join(INDEX)).unwrap();
    hub.save(&new("printer", MemoryType::Machine, "The printer is in the hallway.\nNext to the stairs."), None, SaveMode::Overwrite).unwrap();
    hub.save(&new("router", MemoryType::Machine, "MikroTik hEX."), None, SaveMode::Overwrite).unwrap();
    let text = index();
    assert!(text.starts_with("# Memory\n\nGenerated by KAgent"), "{text}");
    assert!(text.ends_with("- [printer](printer.md) - The printer is in the hallway.\n- [router](router.md) - MikroTik hEX.\n- [user](user.md) - \n"), "{text}: the first line of the content stands in for a missing description");
    assert!(hub.save(&new("wrapped", MemoryType::Machine, "The NAS is at\n192.168.88.10."), None, SaveMode::Overwrite).is_ok());
    assert!(index().contains("- [wrapped](wrapped.md) - The NAS is at\n"), "{}", index());
    hub.remove("wrapped").unwrap();
    // Editing it has no effect: the next change writes it again, without what was forgotten.
    std::fs::write(hub.store().dir().join(INDEX), "edited by hand").unwrap();
    hub.forget("printer").unwrap();
    let text = index();
    assert!(!text.contains("printer") && text.contains("- [router](router.md) - MikroTik hEX.\n"), "{text}");
    // A file added by hand is in it after the next change.
    std::fs::write(hub.store().dir().join("nas.md"), "---\ndescription: the NAS\n---\nAt 192.168.88.10.\n").unwrap();
    hub.remove("router").unwrap();
    assert!(index().contains("- [machine](machine.md) - \n- [nas](nas.md) - the NAS\n"), "{}", index());
    assert_eq!(mode(&hub.store().dir().join(INDEX)), 0o600);
}

#[test]
fn the_limits_and_the_reserved_names_are_enforced() {
    let (hub, _dir, _events) = hub();
    let save = |name: &str, description: &str, body: &str| {
        hub.save(&NewMemory { description: description.into(), ..new(name, MemoryType::User, body) }, None, SaveMode::Overwrite)
    };
    for (name, why) in [("Printer", "is not valid"), ("", "is not valid"), ("a_b", "is not valid"), (USER, "core file"), (MACHINE, "core file"), (INDEX_NAME, "would overwrite the index")] {
        assert!(save(name, "", "body").unwrap_err().to_string().contains(why), "{name}");
    }
    assert_eq!(save("p", "", "   ").unwrap_err(), MemoryError::Invalid("the content is empty: write the fact to remember".into()));
    let long: String = "č".repeat(MAX_BODY_CHARS + 1);
    assert!(save("p", "", &long).unwrap_err().to_string().contains(&format!("{} characters", MAX_BODY_CHARS + 1)), "characters, not bytes");
    let fits = format!("{}c", "ab ".repeat(1333));
    assert_eq!(fits.chars().count(), MAX_BODY_CHARS);
    assert!(save("fits", "", &fits).is_ok(), "the limit itself is kept");
    hub.remove("fits").unwrap();
    assert!(save("p", &"d".repeat(MAX_DESCRIPTION_CHARS + 1), "body").unwrap_err().to_string().contains("the description has 1025 characters"));
    assert_eq!(save("p", "", "sk-0123456789abcdef").unwrap_err(), MemoryError::Invalid("memory 'p': do not store secrets in memory (it looks like an API key)".into()));
    assert!(save("p", "", "password: 0123456789abcdef").is_err(), "a secret in the content is refused");
    assert!(hub.save(&NewMemory { description: "token: 0123456789abcdef".into(), ..new("p", MemoryType::User, "body") }, None, SaveMode::Overwrite).is_err(), "and in the description");
    for (replaces, why) in [("nope", "there is no memory named 'nope' to replace"), ("p", "cannot replace itself"), (USER, "'user' is a core file and is not replaced"), (MACHINE, "is a core file")] {
        let err = hub.save(&new("p", MemoryType::User, "body"), Some(replaces), SaveMode::Overwrite).unwrap_err().to_string();
        assert!(err.contains(why), "{replaces:?}: {err}");
    }
    assert!(hub.all().len() == 2 && !hub.store().path("p").exists(), "nothing of the refused saves was written");

    // POST may not take a name that exists; the agent's save replaces the file.
    assert!(hub.save(&new("p", MemoryType::User, "body"), None, SaveMode::Create).is_ok());
    assert_eq!(hub.save(&new("p", MemoryType::User, "body"), None, SaveMode::Create).unwrap_err(), MemoryError::Exists("a memory named 'p' exists already; change it instead".into()));
    // The limit of 200 files counts the two core files.
    for i in 0..196 {
        std::fs::write(hub.store().dir().join(format!("filler-{i}.md")), "body\n").unwrap();
    }
    assert!(hub.save(&new("last", MemoryType::User, "body"), None, SaveMode::Overwrite).is_ok(), "the 200th file");
    assert_eq!(hub.save(&new("one-too-many", MemoryType::User, "body"), None, SaveMode::Overwrite).unwrap_err(), MemoryError::Full);
    assert!(hub.save(&new("p", MemoryType::User, "body again"), None, SaveMode::Overwrite).is_ok(), "a name that exists is still written");
    assert_eq!(hub.forget(USER).unwrap_err(), MemoryError::Core("the core files 'user' and 'machine' are not forgotten: edit them in Settings -> Memory".into()));
    assert_eq!(hub.remove(MACHINE).unwrap_err(), MemoryError::Core("'machine' is a core file and is not deleted: clear its text in Settings -> Memory instead".into()));
    assert_eq!(hub.remove("nope").unwrap_err(), MemoryError::Unknown("there is no memory named 'nope'".into()));
    assert_eq!(hub.forget("nope").unwrap_err(), MemoryError::Unknown("there is no memory named 'nope'".into()));
}
