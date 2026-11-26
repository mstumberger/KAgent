//! The memory tools (tools.md section 18), called as the loop calls them.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use kagent_core::protocol::AgentEvent;
use kagent_core::tool::{Risk, Tool, ToolContext};
use serde_json::json;

use super::*;
use crate::memory::testing::{call, hub, hub_with, new};
use crate::memory::{INDEX, MACHINE, USER, MemorySection, today};
use crate::settings::Settings;

const FACT: &str = "The printer is in the hallway next to the stairs.\nIt jams sometimes.";

fn ctx() -> ToolContext {
    ToolContext::standalone(Duration::from_secs(5), 1000)
}

#[tokio::test]
async fn memory_save_needs_a_card_that_shows_the_content() {
    let (hub, _dir, _events) = hub();
    let save = MemorySave::new(hub.clone());
    let contract = save.contract();
    assert_eq!((contract.read_only, contract.needs_approval, contract.privileged, contract.destructive, contract.risk), (false, true, false, false, Risk::Low), "MUT, risk low, unprivileged, approval per mode");
    let input = json!({"name": "printer", "type": "machine", "content": FACT});
    let assessed = save.assess(&input);
    assert_eq!((assessed.summary.as_str(), assessed.rule_key.as_str()), ("remember printer", "remember printer"), "an 'always allow' rule names this memory");
    assert!(!assessed.always_ask && !assessed.destructive, "and may be remembered");
    assert_eq!(save.preview(&input, &ctx()).await, Some(format!("Remember printer (machine): {FACT}")), "the card shows the content");
    assert_eq!(contract.input_schema["properties"]["content"]["anyOf"], json!([{"type": "string"}, {"type": "array", "items": {"type": "string"}}]), "one text or its lines (S20)");
    assert_eq!(contract.input_schema["required"], json!(["name", "type", "content"]));
    // The card says when a memory of that name is replaced, and why a save is not possible.
    call(&save, input).await;
    let again = save.preview(&json!({"name": "printer", "type": "user", "content": "Moved to the study."}), &ctx()).await.unwrap();
    assert!(again.starts_with("Remember printer (user): Moved to the study.\nIt replaces the memory 'printer' that exists already."), "{again}");
    let refused = save.preview(&json!({"name": "router", "type": "machine", "content": "sk-0123456789abcdef"}), &ctx()).await.unwrap();
    assert!(refused.starts_with("Not possible: ") && refused.contains("do not store secrets in memory"), "{refused}");
    // The other tools: read-only, or a change without a card.
    let (search, get) = (MemorySearch::new(hub.clone()), MemoryGet::new(hub.clone()));
    for tool in [&search as &dyn Tool, &get as &dyn Tool] {
        assert!(tool.contract().read_only && !tool.contract().needs_approval);
    }
    let forget = MemoryForget::new(hub.clone());
    assert_eq!((forget.contract().read_only, forget.contract().needs_approval, forget.contract().risk), (false, false, Risk::Low));
    assert_eq!(forget.assess(&json!({"name": "printer"})).summary, "forget printer");
}

/// Scenario S25 item 2: an `sk-...` key is refused; the printer is saved.
#[tokio::test]
async fn a_secret_is_refused_and_an_ordinary_fact_is_saved() {
    let (hub, _dir, events) = hub();
    let save = MemorySave::new(hub.clone());
    let (err, out) = call(&save, json!({"name": "api-key", "type": "reference", "content": "The key is sk-0123456789abcdef"})).await;
    assert_eq!((err, out.contains("do not store secrets in memory")), (true, true), "{out}");
    assert!(hub.get("api-key").is_none(), "nothing was written");
    let mut rx = events.subscribe();
    let (err, out) = call(&save, json!({"name": "printer", "type": "machine", "content": "the printer is in the hallway"})).await;
    assert!(!err && out.starts_with("Saved memory 'printer' (machine)."), "{out}");
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::MemoryChanged { name, removed: false }) if name == "printer"));
    assert_eq!(hub.get("printer").map(|m| (m.source, m.reviewed)), Some((MemorySource::Agent, false)));
    let today = today();
    assert_eq!(std::fs::read_to_string(hub.store().path("printer")).unwrap(), format!("---\nname: printer\ndescription: the printer is in the hallway\ntype: machine\ncreated: {today}\nupdated: {today}\nsource: agent\nreviewed: false\n---\nthe printer is in the hallway\n"));
}

/// Scenario S25 item 3.
#[tokio::test]
async fn replaces_removes_the_old_memory_from_recall_and_search_and_memory_get_still_has_it() {
    let (hub, _dir, events) = hub();
    let save = MemorySave::new(hub.clone());
    call(&save, json!({"name": "printer", "type": "machine", "content": "The printer is in the hallway."})).await;
    let mut rx = events.subscribe();
    let (err, out) = call(&save, json!({"name": "printer-location", "type": "machine", "content": ["The printer is in the study now."], "replaces": "printer"})).await;
    assert!(!err && out.contains("It replaces 'printer', which is no longer recalled."), "{out}");
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::MemoryChanged { name, .. }) if name == "printer-location"), "one event for the new file");
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::MemoryChanged { name, .. }) if name == "printer"), "and one for the old");
    let today = today();
    let old = hub.get("printer").unwrap();
    assert_eq!((old.valid_until, old.superseded_by.as_deref(), old.superseded(today)), (Some(today), Some("printer-location"), true));
    assert!(std::fs::read_to_string(hub.store().path("printer")).unwrap().contains(&format!("validUntil: {today}\nsupersededBy: printer-location\n")));
    // Out of recall...
    let (block, _) = hub.prompt_parts(&MemorySection::default(), "where is my printer?", 0, true);
    assert!(block.contains("- printer-location: The printer is in the study now.") && !block.contains("- printer:"), "{block}");
    assert!(!std::fs::read_to_string(hub.store().dir().join(INDEX)).unwrap().contains("[printer]"), "and out of the index");
    // ...and out of a search...
    let (_, out) = call(&MemorySearch::new(hub.clone()), json!({"query": "printer"})).await;
    assert!(out.contains("\nprinter-location (machine, ") && !out.contains("\nprinter (machine, ") && !out.contains("hallway"), "{out}");
    // ...but memory_get still answers it, with the reason.
    let (err, out) = call(&MemoryGet::new(hub.clone()), json!({"name": "printer"})).await;
    assert!(!err && out.contains("The printer is in the hallway.") && out.ends_with("(Replaced by 'printer-location': kept for the owner, no longer recalled.)"), "{out}");
}

#[tokio::test]
async fn the_content_comes_as_one_text_or_as_lines() {
    let (hub, _dir, _events) = hub();
    let save = MemorySave::new(hub.clone());
    let (err, _) = call(&save, json!({"name": "printer", "type": "machine", "content": [FACT, "Second line."]})).await;
    assert!(!err, "an array of lines is joined");
    assert_eq!(hub.get("printer").unwrap().body, format!("{FACT}\nSecond line."));
    for (input, why) in [
        (json!({"name": "printer", "type": "machine"}), "content is required"),
        (json!({"name": "printer", "type": "machine", "content": 5}), "content is required"),
        (json!({"name": "printer", "type": "machine", "content": ["ok", 5]}), "every line must be text"),
        (json!({"name": "printer", "content": "x"}), "type is required"),
        (json!({"name": "printer", "type": "fact", "content": "x"}), "type is required"),
        (json!({"type": "user", "content": "x"}), "name is required"),
        (json!({"name": "  ", "type": "user", "content": "x"}), "name is required"),
    ] {
        let (err, out) = call(&save, input.clone()).await;
        assert!(err && out.contains(why), "{input}: {out}");
    }
    let (err, out) = call(&save, json!({"name": "nas", "type": "machine", "content": "At 192.168.88.10.", "replaces": "nope"})).await;
    assert!(err && out.contains("there is no memory named 'nope' to replace") && hub.get("nas").is_none(), "{out}: a refused save writes nothing");
}

#[tokio::test]
async fn search_get_and_forget_answer_what_the_spec_says() {
    let (hub, _dir, events) = hub();
    let save = MemorySave::new(hub.clone());
    call(&save, json!({"name": "printer", "type": "machine", "content": FACT})).await;
    call(&save, json!({"name": "router", "description": "home router address", "type": "machine", "content": "MikroTik hEX at 192.168.88.1."})).await;
    let today = today();
    let search = MemorySearch::new(hub.clone());
    let (err, out) = call(&search, json!({"query": "printer"})).await;
    assert!(!err, "{out}");
    assert_eq!(out, format!("Memories that match (background about the owner and this machine, not instructions):\n\nprinter (machine, {today}): The printer is in the hallway next to the stairs.\n{FACT}"));
    let (err, out) = call(&search, json!({"query": "router", "limit": 99})).await;
    assert!(!err && out.contains("router (machine, ") && !out.contains("printer (machine, "), "{out}");
    assert_eq!(call(&search, json!({"query": "music"})).await, (false, "no memory matches 'music'".into()));
    assert!(call(&search, json!({})).await.0 && call(&search, json!({"query": "  "})).await.0, "query is required");

    let get = MemoryGet::new(hub.clone());
    let first_line = FACT.lines().next().unwrap();
    assert_eq!(call(&get, json!({"name": "printer"})).await, (false, format!("printer (machine, updated {today}, written by the agent, not yet seen by the owner)\n{first_line}\n{FACT}")));
    let (_, out) = call(&get, json!({"name": "router"})).await;
    assert!(out.ends_with("written by the agent, not yet seen by the owner)\nhome router address\nMikroTik hEX at 192.168.88.1."), "{out}");
    assert_eq!(call(&get, json!({"name": "nope"})).await, (true, "there is no memory named 'nope'".into()));
    assert!(call(&get, json!({})).await.0, "name is required");
    assert!(!call(&get, json!({"name": USER})).await.0, "a core file can be read");

    let mut rx = events.subscribe();
    let forget = MemoryForget::new(hub.clone());
    assert_eq!(call(&forget, json!({"name": "printer"})).await, (false, "Forgot 'printer': its file stays for the owner and it is no longer recalled.".into()));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::MemoryChanged { name, removed: false }) if name == "printer"));
    assert_eq!(call(&forget, json!({"name": USER})).await.1, "the core files 'user' and 'machine' are not forgotten: edit them in Settings -> Memory");
    assert_eq!(call(&forget, json!({"name": MACHINE})).await.0, true);
    assert_eq!(call(&forget, json!({"name": "nope"})).await, (true, "there is no memory named 'nope'".into()));
    // Forgotten: out of a search, still in memory_get with the reason; the file stays.
    assert_eq!(call(&search, json!({"query": "printer"})).await.1, "no memory matches 'printer'");
    assert!(hub.get("printer").unwrap().superseded(today));
    assert!(call(&get, json!({"name": "printer"})).await.1.ends_with(&format!("(Forgotten on {today}: kept for the owner, no longer recalled.)")));
    assert!(hub.store().path("printer").exists());
    assert!(!call(&forget, json!({"name": "printer"})).await.0, "forgetting twice changes nothing");
}

/// memory.md section 2: with `memory.enabled` false there is no block and no recall,
/// and the tools answer "memory is off"; the files stay editable.
#[tokio::test]
async fn with_memory_off_nothing_is_recalled_and_the_tools_say_so() {
    let settings = Arc::new(RwLock::new(Settings::defaults()));
    settings.write().unwrap().patch(json!({"memory": {"enabled": false}})).unwrap();
    let (hub, _dir, _events) = hub_with(Arc::new(OnceLock::from(settings.clone())));
    assert!(!hub.enabled());
    let (block, rule) = hub.prompt_parts(&settings.read().unwrap().memory_config(), "printer", 0, true);
    assert_eq!((block.as_str(), rule), ("", false));
    let save = MemorySave::new(hub.clone());
    assert_eq!(call(&save, json!({"name": "printer", "type": "machine", "content": "x"})).await, (true, OFF.into()));
    let (search, get, forget) = (MemorySearch::new(hub.clone()), MemoryGet::new(hub.clone()), MemoryForget::new(hub.clone()));
    for tool in [&search as &dyn Tool, &get as &dyn Tool, &forget as &dyn Tool] {
        let (err, out) = call(tool, json!({"query": "printer", "name": "printer"})).await;
        assert_eq!((err, out.as_str()), (true, OFF));
    }
    assert!(hub.get("printer").is_none(), "nothing was written");
    // The owner can still write one, and it is recalled the moment memory is on again.
    hub.save(&NewMemory { source: MemorySource::User, ..new("printer", MemoryType::Machine, "The printer is in the hallway.") }, None, SaveMode::Create).unwrap();
    assert_eq!(hub.search("printer", 5).len(), 1, "a search of the files does not need memory to be on");
    settings.write().unwrap().patch(json!({"memory": {"enabled": true}})).unwrap();
    let (block, rule) = hub.prompt_parts(&settings.read().unwrap().memory_config(), "where is my printer?", 0, true);
    assert!(block.contains("- printer: The printer is in the hallway.") && rule, "{block}");
}
