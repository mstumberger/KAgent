//! Memory (memory.md): what a memory is, the typed `memory` settings section, and the hub
//! the prompt, the tools and the API share. The kinds of a memory, a memory to write and
//! what the API shows of one are `types.rs`; the store is `store.rs`, the file format
//! `file.rs`, the secret guard `guard.rs`, recall `recall.rs`, the tools `tools.rs` and
//! `session_search.rs`, the routes `http.rs`.

mod file;
mod guard;
pub(crate) mod http;
pub mod recall;
mod session_search;
mod store;
mod tools;
mod types;

pub use store::MemoryStore;
pub use types::{MemoryError, MemorySource, MemoryType, MemoryView, NewMemory, SaveMode};

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use kagent_core::protocol::AgentEvent;

use crate::sections::SectionError;
use crate::settings::Settings;

#[cfg(test)]
mod hub_tests;
#[cfg(test)]
mod tests;

/// At most this many memory files, the two core files included (memory.md section 1).
pub const MAX_FILES: usize = 200;
/// At most this many characters in the body of one memory.
pub const MAX_BODY_CHARS: usize = 4000;
/// At most this many characters in the description of one memory.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;
/// The first line of the content stands in for a missing description, cut here.
const LINE_DESCRIPTION_CHARS: usize = 140;
/// The core files (memory.md section 1): who the owner is, and what no tool can derive
/// about this machine.
pub const USER: &str = "user";
pub const MACHINE: &str = "machine";
/// The first line of a new `machine.md`: a comment that says what belongs there.
const MACHINE_SEED: &str = "<!-- What belongs here: what no tool can derive. Devices, addresses, services you run, pinned versions and why, past fixes and decisions. -->";
/// The generated index, and the name a topic memory must not take (it would be that
/// file on a case-insensitive disk).
const INDEX: &str = "MEMORY.md";
const INDEX_NAME: &str = "memory";
/// The five types of a topic memory, as a message lists them.
pub const TYPE_NAMES: &str = "user, feedback, machine, project or reference";
/// What every memory tool answers while `memory.enabled` is false.
pub const OFF: &str = "memory is off";

/// settings.memory (config.md section 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MemorySection {
    /// false: no memory block in the prompt, no recall, and the memory tools answer
    /// "memory is off". The files stay editable in the UI.
    pub enabled: bool,
    /// 0..=8: how many memories related to the message go into the prompt.
    pub recall_count: u8,
    /// 0..=8000: their size together, in characters.
    pub recall_chars: u32,
}

impl Default for MemorySection {
    fn default() -> Self {
        Self { enabled: true, recall_count: 3, recall_chars: 1500 }
    }
}

impl MemorySection {
    /// A count or a size outside its range makes the settings file invalid.
    pub fn validate(&self) -> Result<(), SectionError> {
        if self.recall_count > 8 {
            return Err(SectionError::Invalid { key: "memory".into(), message: "recallCount must be between 0 and 8".into() });
        }
        if self.recall_chars > 8000 {
            return Err(SectionError::Invalid { key: "memory".into(), message: "recallChars must be between 0 and 8000".into() });
        }
        Ok(())
    }
}

/// A memory as it is stored (memory.md section 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    /// The file name without `.md`: `^[a-z0-9][a-z0-9-]{0,63}$`.
    pub name: String,
    pub description: String,
    /// The body, without the frontmatter.
    pub body: String,
    pub r#type: MemoryType,
    pub created: NaiveDate,
    pub updated: NaiveDate,
    pub source: MemorySource,
    /// The owner has seen it (a memory of the agent's starts unreviewed).
    pub reviewed: bool,
    /// The last day it counts: `memory_forget` and `replaces` set it.
    pub valid_until: Option<NaiveDate>,
    /// The memory that replaced this one.
    pub superseded_by: Option<String>,
    /// When the file was last written. Among memories updated the same day, the one
    /// written last is the newer (memory.md section 2).
    pub modified: Option<DateTime<Utc>>,
    pub bytes: u64,
}

/// Whether `name` is one of the two core files.
pub fn is_core(name: &str) -> bool {
    name == USER || name == MACHINE
}

/// Today, as the dates of a memory file are written: the local date.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

impl Memory {
    pub fn is_core(&self) -> bool {
        is_core(&self.name)
    }

    /// Superseded (memory.md section 2): replaced by another memory, forgotten, or past
    /// its last day. Not recalled and not found by a search; `memory_get` still answers.
    pub fn superseded(&self, today: NaiveDate) -> bool {
        self.superseded_by.as_deref().is_some_and(|s| !s.is_empty()) || self.valid_until.is_some_and(|d| d <= today)
    }

    /// Written by the agent and not seen by the owner.
    pub fn unreviewed(&self) -> bool {
        self.source == MemorySource::Agent && !self.reviewed
    }
}

#[cfg(test)]
impl Memory {
    /// A memory of the owner's, as the tests of the scorer and the API use them.
    pub(super) fn sample(name: &str, r#type: MemoryType, updated: &str, description: &str, body: &str) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            body: body.into(),
            r#type,
            created: NaiveDate::parse_from_str("2026-01-01", "%Y-%m-%d").unwrap(),
            updated: NaiveDate::parse_from_str(updated, "%Y-%m-%d").unwrap(),
            source: MemorySource::User,
            reviewed: true,
            valid_until: None,
            superseded_by: None,
            modified: None,
            bytes: body.len() as u64,
        }
    }
}

/// A hub over a temporary folder, for the tests of the module.
#[cfg(test)]
pub(super) mod testing {
    use super::*;

    pub fn hub_with(settings: Arc<OnceLock<Arc<RwLock<Settings>>>>) -> (Arc<MemoryHub>, tempfile::TempDir, broadcast::Sender<AgentEvent>) {
        let dir = tempfile::tempdir().unwrap();
        let (events, _) = broadcast::channel(64);
        let hub = Arc::new(MemoryHub::new(MemoryStore::new(dir.path().join("memory")), dir.path().join("sessions"), settings, events.clone()));
        (hub, dir, events)
    }

    /// With the default settings.
    pub fn hub() -> (Arc<MemoryHub>, tempfile::TempDir, broadcast::Sender<AgentEvent>) {
        hub_with(Arc::new(OnceLock::new()))
    }

    /// A hub whose settings say memory is off.
    pub fn hub_off() -> Arc<MemoryHub> {
        let settings = Arc::new(RwLock::new(Settings::defaults()));
        settings.write().unwrap().patch(serde_json::json!({"memory": {"enabled": false}})).unwrap();
        hub_with(Arc::new(OnceLock::from(settings))).0
    }

    /// A memory the agent writes.
    pub fn new(name: &str, r#type: MemoryType, body: &str) -> NewMemory {
        NewMemory { name: name.into(), description: String::new(), r#type, body: body.into(), source: MemorySource::Agent }
    }

    /// What a tool answers: whether it is an error, and the text.
    pub async fn call(tool: &dyn kagent_core::tool::Tool, input: serde_json::Value) -> (bool, String) {
        let out = tool.call(input, kagent_core::tool::ToolContext::standalone(std::time::Duration::from_secs(5), 200_000)).await.expect("the tool runs");
        (out.is_error, out.content)
    }
}

/// What a new `user.md` says (memory.md section 1): the address and the language of the persona.
fn seed_text(address: &str, language: &str) -> String {
    let address = address.trim();
    let lang = match language {
        "sl" => "The owner wants answers in Slovenian.",
        "en" => "The owner wants answers in English.",
        _ => "The owner writes Slovenian or English: answer in that language.",
    };
    format!("Address the owner as {}.\n{lang}", if address.is_empty() { "the owner" } else { address })
}

/// The memory of one daemon: the store, the settings the tools read, the sessions
/// `session_search` looks at, and the event every change sends.
#[derive(Clone)]
pub struct MemoryHub {
    store: Arc<MemoryStore>,
    sessions: PathBuf,
    /// The daemon's settings, set once they are loaded (as for the screen tool); until
    /// then, and in tests, the defaults.
    settings: Arc<OnceLock<Arc<RwLock<Settings>>>>,
    events: broadcast::Sender<AgentEvent>,
}

impl MemoryHub {
    /// Makes the folder, readable by the owner only. `sessions` is `<data>/sessions`.
    pub fn new(store: MemoryStore, sessions: impl Into<PathBuf>, settings: Arc<OnceLock<Arc<RwLock<Settings>>>>, events: broadcast::Sender<AgentEvent>) -> Self {
        if let Err(e) = store.secure() {
            tracing::warn!("memory folder {}: {e}", store.dir().display());
        }
        Self { store: Arc::new(store), sessions: sessions.into(), settings, events }
    }

    pub fn store(&self) -> &Arc<MemoryStore> {
        &self.store
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions
    }

    /// settings.memory; the defaults until the daemon's settings are loaded.
    pub fn section(&self) -> MemorySection {
        self.settings.get().and_then(|s| s.read().ok().map(|s| s.memory_config())).unwrap_or_default()
    }

    pub fn enabled(&self) -> bool {
        self.section().enabled
    }

    fn changed(&self, name: &str, removed: bool) {
        let _ = self.events.send(AgentEvent::MemoryChanged { name: name.to_string(), removed });
    }

    /// Every memory file by name, the superseded ones included. The two core files
    /// exist afterwards (first use creates them).
    pub fn all(&self) -> Vec<Memory> {
        if let Some(settings) = self.settings.get().and_then(|s| s.read().ok()) {
            let persona = &settings.persona;
            self.store.set_seed(seed_text(&persona.address, &persona.language));
        }
        if let Err(e) = self.store.ensure_core() {
            tracing::warn!("memory: {e}");
        }
        self.store.load()
    }

    pub fn get(&self, name: &str) -> Option<Memory> {
        self.store.get(name)
    }

    /// Check a save without writing: what the card of `memory_save` shows.
    pub fn plan(&self, new: &NewMemory, replaces: Option<&str>) -> Result<store::Planned, MemoryError> {
        self.store.plan(new, replaces)
    }

    /// The current memories that match `query`, best first (the recall scorer). A search
    /// does not need memory to be on: the owner may look through the files.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Memory> {
        let all = self.all();
        recall::rank(&all, query, limit, today()).into_iter().map(|hit| hit.memory.clone()).collect()
    }

    /// Write a memory; with `replaces`, the memory it supersedes is marked and returned.
    pub fn save(&self, new: &NewMemory, replaces: Option<&str>, mode: SaveMode) -> Result<(Memory, Option<Memory>), MemoryError> {
        let (memory, old) = self.store.save(new, replaces, mode)?;
        self.changed(&memory.name, false);
        if let Some(old) = &old {
            self.changed(&old.name, false);
        }
        Ok((memory, old))
    }

    /// Change one memory under the store's lock; `edit` sees its current state.
    pub fn update(&self, name: &str, edit: impl FnMut(&mut Memory) -> Result<(), MemoryError>) -> Result<Memory, MemoryError> {
        let memory = self.store.update(name, edit)?;
        self.changed(&memory.name, false);
        Ok(memory)
    }

    /// Mark a memory `validUntil: today`: it stays on disk and is no longer recalled.
    pub fn forget(&self, name: &str) -> Result<Memory, MemoryError> {
        let memory = self.store.forget(name)?;
        self.changed(&memory.name, false);
        Ok(memory)
    }

    /// Delete the file of a topic memory.
    pub fn remove(&self, name: &str) -> Result<(), MemoryError> {
        self.store.remove(name)?;
        self.changed(name, true);
        Ok(())
    }

    /// What the prompt of a turn needs (memory.md section 2): the block after the skills
    /// index, and whether the rule about saving memories belongs in the working rules
    /// (memory is on and the model has the memory tools: `memory_tools`).
    pub fn prompt_parts(&self, section: &MemorySection, user_text: &str, context_tokens: usize, memory_tools: bool) -> (String, bool) {
        if !section.enabled {
            return (String::new(), false);
        }
        let all = self.all();
        let caps = recall::Caps::new(context_tokens, section.recall_chars);
        let (user, machine) = core_parts(&all, caps.core);
        let topics = all.iter().filter(|m| !m.is_core());
        let ranked = recall::rank(topics, user_text, usize::from(section.recall_count), today());
        (recall::prompt_block(&user, &machine, &recall::fit_related(ranked, caps.related)), memory_tools)
    }

    /// The bodies of the two core files, for the context message of a live voice session
    /// (voice.md section 4): together at most `cap_chars`.
    pub fn core_text(&self, cap_chars: usize) -> String {
        let all = self.all();
        let (user, machine) = core_parts(&all, cap_chars);
        [user, machine].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n")
    }
}

impl Settings {
    /// settings.memory: whether KAgent remembers across sessions, and how much it
    /// recalls for a message.
    pub fn memory_config(&self) -> MemorySection {
        self.section::<MemorySection>("memory").unwrap_or_default()
    }
}

/// The bodies of `user.md` and `machine.md`, without their comments, together within
/// `cap` characters.
fn core_parts(all: &[Memory], cap: usize) -> (String, String) {
    let body = |name: &str| all.iter().find(|m| m.name == name).map_or_else(String::new, |m| recall::strip_comments(&m.body));
    recall::core_texts(&body(USER), &body(MACHINE), cap)
}
