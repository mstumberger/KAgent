//! settings.memory, what the prompt of a turn gets, and that the config source is
//! asked with the message of the turn.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use kagent_core::agent::{Agent, AgentConfig, AgentDeps, SendOutcome, TurnContext};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::{TurnStatus, Via};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::recall;
use super::testing::{hub, new};
use super::*;
use crate::prompt::MEMORY_RULE;
use crate::settings::Settings;

#[test]
fn the_memory_settings_section_has_defaults_ranges_and_a_typo_check() {
    let mut s = Settings::defaults();
    assert_eq!(s.memory_config(), MemorySection { enabled: true, recall_count: 3, recall_chars: 1500 }, "a missing section has the defaults");
    s.patch(json!({"memory": {"recallCount": 0, "recallChars": 8000}})).unwrap();
    assert_eq!(s.memory_config(), MemorySection { enabled: true, recall_count: 0, recall_chars: 8000 }, "a missing field takes the default; the ends of the ranges are taken");
    for bad in [json!({"memory": {"recallCount": 9}}), json!({"memory": {"recallChars": 8001}}), json!({"memory": {"recallCount": "three"}}), json!({"memory": {"enabel": false}}), json!({"memory": true})] {
        assert!(s.patch(bad.clone()).is_err(), "{bad} must be rejected");
    }
    assert_eq!(s.memory_config().recall_count, 0, "a rejected patch changes nothing");
    assert!(s.patch(json!({"memory": {"enable": false}})).unwrap_err().contains("did you mean \"enabled\"?"));

    // Refused on load as well, like every typed section; a file from before the section
    // has the defaults.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, json!({"memory": {"recallChars": 9000}}).to_string()).unwrap();
    assert!(Settings::load(&path).unwrap_err().to_string().contains("recallChars must be between 0 and 8000"));
    std::fs::write(&path, "{}").unwrap();
    assert_eq!(Settings::load(&path).unwrap().memory_config(), MemorySection::default());
    // `memory` is a default tool group, and one a settings file may name.
    assert!(crate::settings::DEFAULT_TOOL_GROUPS.contains(&"memory") && crate::settings::known_groups().contains(&"memory"));
}

/// The characters of the related memories in a block: what `memory.recallChars` caps.
fn related_chars(block: &str) -> usize {
    block.split("Related memories:\n").nth(1).map_or(0, |related| related.lines().filter(|l| l.starts_with("- ")).map(|l| l.split_once(": ").map_or(0, |(_, body)| body.chars().count())).sum())
}

fn related_names(block: &str) -> Vec<String> {
    block.split("Related memories:\n").nth(1).map_or_else(Vec::new, |related| related.lines().filter_map(|l| l.strip_prefix("- ")).filter_map(|l| l.split_once(':').map(|(name, _)| name.to_string())).collect())
}

/// `memory.recallCount` and `memory.recallChars` are read when the prompt is built.
#[test]
fn recall_count_and_recall_chars_decide_what_the_prompt_gets() {
    let (hub, _dir, _events) = hub();
    for i in 0..5 {
        hub.save(&new(&format!("printer-{i}"), MemoryType::Machine, &format!("printer number {i} {}", "detail of the note. ".repeat(32))), None, SaveMode::Overwrite).unwrap();
    }
    let section = |count: u8, chars: u32| MemorySection { recall_count: count, recall_chars: chars, ..MemorySection::default() };
    let (block, rule) = hub.prompt_parts(&section(0, 1500), "printer", 0, false);
    assert!(block.contains("About me: ") && !block.contains("Related memories") && !rule, "{block}", );
    let (block, _) = hub.prompt_parts(&section(2, 1500), "printer", 0, true);
    assert_eq!(related_names(&block).len(), 2, "{block}");
    // Five fit the count, 700 characters hold one whole body of 656 and the start of a second
    // (cut at its end; a space the cut leaves is trimmed).
    let (block, _) = hub.prompt_parts(&section(8, 700), "printer", 0, true);
    let chars = related_chars(&block);
    assert_eq!(related_names(&block).len(), 2, "{block}");
    assert!(chars > 690 && chars <= 700, "{chars}: {block}");
    // A window under 32k tokens: 600 together, whatever recallChars says.
    let (block, _) = hub.prompt_parts(&section(8, 8000), "printer", 16_000, true);
    assert_eq!((related_names(&block).len(), related_chars(&block)), (1, 600), "{block}");
    // The words of the message decide what is related.
    let (block, _) = hub.prompt_parts(&MemorySection::default(), "how is the weather", 0, true);
    assert!(!block.contains("Related memories"), "{block}");
    // A core file edited by the owner is always there; its comment is not.
    hub.update(USER, |m| {
        m.body = "Marko. Short answers.\n<!-- a note -->".into();
        Ok(())
    })
    .unwrap();
    let (block, _) = hub.prompt_parts(&MemorySection::default(), "", 0, true);
    assert!(block.contains("\nAbout me: Marko. Short answers.\n") && !block.contains("a note"), "{block}");
    assert_eq!(hub.core_text(9), "Marko. Sh", "cut to what the caller may send");
}

/// A model that answers nothing and keeps the system prompt of each request.
struct Capturing {
    systems: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ModelClient for Capturing {
    async fn complete(&self, request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        self.systems.lock().unwrap().push(request.system.clone());
        Ok(ModelResponse::default())
    }
}

/// The daemon's config closure recalls by the message of the turn: the config source is
/// asked with a `TurnContext` that carries it.
#[tokio::test]
async fn the_config_source_is_asked_with_the_message_of_the_turn() {
    let (hub, dir, _events) = hub();
    hub.save(&new("printer", MemoryType::Machine, "The printer is in the hallway."), None, SaveMode::Overwrite).unwrap();
    let (events, _) = broadcast::channel(16);
    let systems = Arc::new(Mutex::new(Vec::new()));
    let (memory, captured) = (hub.clone(), systems.clone());
    let config = Arc::new(move |turn: TurnContext| {
        let (block, rule) = memory.prompt_parts(&MemorySection::default(), &turn.user_text, 0, true);
        AgentConfig {
            system_prompt: format!("{block}{}", if rule { MEMORY_RULE } else { "" }),
            model_ref: "p::m".into(),
            max_iterations: 4,
            options: Default::default(),
            privilege: Default::default(),
            context_tokens: 0,
            fallback_model_ref: String::new(),
            fallback_context_tokens: 0,
            fallback_max_output: None,
        }
    });
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(Capturing { systems }),
        tools: Arc::new(ToolRegistry::new()),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
        store: SessionStore::new(dir.path().join("sessions")).unwrap(),
        events,
        config,
        on_rules_changed: None,
    });
    let session = agent.store().create("t").await.unwrap().id;
    for (text, recalled) in [("where is my printer?", true), ("how is the weather?", false)] {
        match agent.send(&session, text.into(), Via::Text).await.unwrap() {
            SendOutcome::Started { handle, .. } => assert_eq!(handle.wait().await.status, TurnStatus::Completed),
            SendOutcome::Queued => panic!("no turn is running"),
        }
        let systems = captured.lock().unwrap();
        let system = systems.last().unwrap();
        assert!(system.starts_with(recall::HEADING) && system.ends_with(MEMORY_RULE), "{system}");
        assert_eq!(system.contains("- printer: The printer is in the hallway."), recalled, "recalled by the words of the message: {system}");
    }
    assert_eq!(captured.lock().unwrap().len(), 2);
    // For no turn in particular (live voice, compaction) the source sees no message: the core only.
    let plain = agent.config().system_prompt;
    assert!(plain.contains("About me: ") && !plain.contains("Related memories"), "{plain}");
}
