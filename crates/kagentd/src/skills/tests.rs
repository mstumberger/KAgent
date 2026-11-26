use super::*;
use std::path::Path;
use std::time::Duration;

use kagent_core::agent::TurnOpts;

use kagent_core::skills::SkillSource;
use kagent_core::tool::{Tool, ToolContext};
use serde_json::json;

use crate::settings::Settings;

fn write_skill(root: &Path, name: &str, description: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\nBody of {name}.")).unwrap();
}

/// What the config closure does with the library, turn after turn.
fn prompt_index(hub: &SkillHub, settings: &Settings) -> String {
    hub.refresh(&settings.skills_config());
    hub.index(settings.skills_config().index_budget_chars)
}

#[tokio::test]
async fn a_skill_added_between_turns_is_in_the_next_prompt_and_an_event_is_sent() {
    // Scenario S18 items 2 and 3.
    let home = tempfile::tempdir().unwrap();
    let (config, bundled) = (home.path().join("config"), home.path().join("bundled"));
    write_skill(&bundled, "slow-machine", "why is it slow");
    write_skill(&config.join("skills"), "alpha", "written by the owner");
    let roots = [
        SkillRoot { path: bundled, source: SkillSource::Bundled },
        SkillRoot { path: config.join("skills"), source: SkillSource::User },
    ];
    let (events, mut rx) = broadcast::channel(16);
    let hub = SkillHub::new(&roots, &[], events.clone(), home.path().join("data/skill-usage.json"));
    let settings = Settings::defaults();
    let index = prompt_index(&hub, &settings);
    assert!(index.contains("- alpha: written by the owner") && index.contains("- slow-machine: why is it slow"), "{index}");
    assert!(rx.try_recv().is_err(), "the first scan is not a change");

    // A skill added between two turns is in the next prompt, without a restart.
    write_skill(&config.join("skills"), "beta", "added later");
    let index = prompt_index(&hub, &settings);
    assert!(index.contains("- beta: added later"), "{index}");
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::SkillsChanged { count: 3 })));
    assert!(rx.try_recv().is_err(), "nothing else changed");
    assert_eq!(serde_json::to_value(AgentEvent::SkillsChanged { count: 12 }).unwrap(), json!({"type": "skillsChanged", "count": 12}));

    // skills.disabled hides a skill from the index and load_skill refuses it.
    let mut settings = Settings::defaults();
    settings.patch(json!({"skills": {"disabled": ["slow-machine"], "indexBudgetChars": 500}})).unwrap();
    let index = prompt_index(&hub, &settings);
    assert!(!index.contains("slow-machine") && index.contains("- beta: added later"), "{index}");
    let tool = hub.tool();
    let ctx = ToolContext::standalone(Duration::from_secs(5), 1 << 20);
    let refused = tool.call(json!({"name": "slow-machine"}), ctx.clone()).await.unwrap();
    assert!(refused.is_error && refused.content.contains("is disabled"), "{}", refused.content);
    let loaded = tool.call(json!({"name": "beta"}), ctx).await.unwrap();
    assert!(!loaded.is_error && loaded.content == "Body of beta.", "{}", loaded.content);
    assert!(home.path().join("data/skill-usage.json").is_file(), "the use is counted");
}
/// A hub over a bundled root and the owner's root under `<home>/config/skills`.
fn hub_in(home: &Path, disabled: &[&str]) -> (SkillHub, PathBuf, broadcast::Receiver<AgentEvent>) {
    let (bundled, user) = (home.join("bundled"), home.join("config/skills"));
    std::fs::create_dir_all(&user).unwrap();
    let roots = [
        SkillRoot { path: bundled.clone(), source: SkillSource::Bundled },
        SkillRoot { path: user, source: SkillSource::User },
    ];
    let disabled: Vec<String> = disabled.iter().map(|d| d.to_string()).collect();
    let (events, rx) = broadcast::channel(16);
    let hub = SkillHub::new(&roots, &disabled, events, home.join("data/skill-usage.json"));
    (hub, bundled, rx)
}

#[tokio::test]
async fn a_message_that_names_a_skill_gives_its_turn_the_body_and_counts_a_use() {
    // Scenario S20 item 5, the daemon's part.
    let home = tempfile::tempdir().unwrap();
    let (hub, bundled, _rx) = hub_in(home.path(), &["off"]);
    write_skill(&bundled, "slow-machine", "why is it slow");
    std::fs::write(bundled.join("slow-machine/SKILL.md"), "---\nname: slow-machine\ndescription: why is it slow\n---\nRun ${KAGENT_SKILL_DIR}/check.sh").unwrap();
    write_skill(&bundled, "off", "switched off");
    hub.reload();

    // What `post_message` does with a message that starts with the skill's name.
    let opts = hub.turn_opts("/slow-machine it lags after login");
    let block = opts.system_suffix.expect("the turn gets the skill body");
    assert!(block.starts_with("\nSkill 'slow-machine' (the user asked for it with /slow-machine)"), "{block}");
    assert!(block.ends_with(&format!("Run {}/check.sh\n", bundled.join("slow-machine").display())), "{block}");
    assert_eq!(hub.counter().get("slow-machine").map(|u| u.uses), Some(1), "loaded this way counts as a use");

    // Unknown, switched off, or not a skill at all: plain text, nothing counted.
    for text in ["/nope do it", "/off do it", "/etc/passwd is readable?", "what is /slow-machine?", "slow-machine please"] {
        assert_eq!(hub.turn_opts(text), TurnOpts::default(), "{text}");
    }
    assert_eq!(hub.counter().get("off"), None);
}

#[test]
fn views_carry_source_state_usage_and_the_time_of_the_file() {
    let home = tempfile::tempdir().unwrap();
    let (hub, bundled, _rx) = hub_in(home.path(), &["alpha"]);
    write_skill(&bundled, "alpha", "first");
    write_skill(&home.path().join("config/skills"), "zeta", "written by the owner");
    hub.reload();
    hub.counter().record("zeta");

    let names: Vec<(String, bool)> = hub.views().into_iter().map(|v| (v.name, v.enabled)).collect();
    assert_eq!(names, [("zeta".to_string(), true), ("alpha".to_string(), false)], "user first, then bundled; a disabled one is listed too");
    let zeta = hub.view("zeta", true).expect("known");
    assert_eq!((zeta.uses, zeta.body.as_deref(), zeta.source), (1, Some("Body of zeta."), SkillSource::User));
    assert_eq!(zeta.path, home.path().join("config/skills/zeta"));
    assert!(zeta.real_path.ends_with("config/skills/zeta") && !zeta.symlink);
    assert!(chrono::DateTime::parse_from_rfc3339(&zeta.updated_at).is_ok(), "updatedAt is RFC 3339: {}", zeta.updated_at);
    assert!(zeta.last_used.starts_with("20") && zeta.size_bytes > 0);
    assert!(hub.views()[0].body.is_none(), "the list sends no bodies");

    // The JSON has the keys of api.md `SkillView`, and no empty ones.
    let json = serde_json::to_value(&zeta).unwrap();
    let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(
        keys,
        ["body", "description", "enabled", "lastUsed", "name", "path", "realPath", "shadowed", "sizeBytes", "source", "symlink", "updatedAt", "uses"]
    );
    assert_eq!((json["source"].as_str(), json["enabled"].as_bool(), json["shadowed"].as_bool()), (Some("user"), Some(true), Some(false)));
    assert!(hub.view("nope", true).is_none());
}

#[tokio::test]
async fn the_author_tools_update_the_library_and_tell_the_ui_at_once() {
    // Scenario S20 item 3, the file part: after the save the file is there and `load_skill` returns it.
    let home = tempfile::tempdir().unwrap();
    let (hub, bundled, mut rx) = hub_in(home.path(), &[]);
    write_skill(&bundled, "alpha", "first");
    hub.reload();
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::SkillsChanged { count: 1 })), "the rescan is the first event");
    let tools = hub.author_tools();
    let (save, delete) = (&*tools[0], &*tools[1]);
    assert_eq!((save.contract().name.as_str(), delete.contract().name.as_str()), ("skill_save", "skill_delete"));
    let ctx = ToolContext::standalone(Duration::from_secs(5), 1 << 20);
    let out = save.call(json!({"name": "bluetooth-fix", "description": "no sound after reconnect", "body": "1. `bluetooth` info."}), ctx.clone()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(home.path().join("config/skills/bluetooth-fix/SKILL.md").is_file(), "under <config>/skills");
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::SkillsChanged { count: 2 })), "the UI hears of it at once");
    let out = hub.tool().call(json!({"name": "bluetooth-fix"}), ctx.clone()).await.unwrap();
    assert_eq!(out.content, "1. `bluetooth` info.", "load_skill returns it, in the same turn");
    assert!(hub.views().iter().any(|v| v.name == "bluetooth-fix" && v.source == SkillSource::User));

    let out = delete.call(json!({"name": "bluetooth-fix"}), ctx).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(!home.path().join("config/skills/bluetooth-fix").exists());
    assert!(hub.view("bluetooth-fix", false).is_none());
}
