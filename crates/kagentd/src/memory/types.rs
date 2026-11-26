//! The kinds of a memory (memory.md section 1): its type, who wrote it, a memory to
//! write, what the API shows of one (api.md section 6), and why a change was refused.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Memory, MAX_FILES};

/// The type of a memory (memory.md section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// About the owner: preferences, people, projects.
    #[default]
    User,
    /// A correction from the owner: do this, not that, and why.
    Feedback,
    /// What no tool can derive about this machine.
    Machine,
    Project,
    Reference,
}

impl MemoryType {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "user" => Some(Self::User),
            "feedback" => Some(Self::Feedback),
            "machine" => Some(Self::Machine),
            "project" => Some(Self::Project),
            "reference" => Some(Self::Reference),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Machine => "machine",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who wrote a memory (memory.md section 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// The owner, or the owner edited it.
    #[default]
    User,
    /// The agent; `reviewed` says whether the owner has seen it.
    Agent,
}

impl MemorySource {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "user" => Some(Self::User),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Agent => "agent",
        }
    }
}

impl std::fmt::Display for MemorySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A memory to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMemory {
    pub name: String,
    /// One line; empty = the first line of the body, cut to 140 characters.
    pub description: String,
    pub r#type: MemoryType,
    pub body: String,
    pub source: MemorySource,
}

/// Whether a save may take the name of a memory that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveMode {
    /// No: a memory of that name is an error (the API's POST).
    Create,
    /// Yes: the new memory replaces the file (the agent's `memory_save`).
    Overwrite,
}

/// Why a memory was not written, changed or removed. The text is what the tool answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryError {
    /// Not a valid memory (400 `invalid_input`).
    Invalid(String),
    /// The store has `MAX_FILES` files (409).
    Full,
    /// A memory of that name exists already (409).
    Exists(String),
    /// A core file where a topic memory is needed, or the other way round (409).
    Core(String),
    /// There is no memory of that name (404).
    Unknown(String),
    /// A file could not be written or removed (500).
    Io(String),
}

impl std::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(m) | Self::Core(m) | Self::Exists(m) | Self::Unknown(m) | Self::Io(m) => f.write_str(m),
            Self::Full => write!(f, "memory is full: it holds {MAX_FILES} files; forget or delete a memory first"),
        }
    }
}

impl std::error::Error for MemoryError {}

/// One memory as the API shows it (api.md section 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    pub name: String,
    pub description: String,
    pub r#type: MemoryType,
    pub path: PathBuf,
    pub bytes: u64,
    /// `YYYY-MM-DD`.
    pub created_at: String,
    pub updated_at: String,
    pub source: MemorySource,
    pub reviewed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// As stored: the list and the answers of writes carry it, a search does not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl MemoryView {
    pub fn of(m: &Memory, path: &Path, content: bool) -> Self {
        Self {
            name: m.name.clone(),
            description: m.description.clone(),
            r#type: m.r#type,
            path: path.to_path_buf(),
            bytes: m.bytes,
            created_at: m.created.to_string(),
            updated_at: m.updated.to_string(),
            source: m.source,
            reviewed: m.reviewed,
            valid_until: m.valid_until.map(|d| d.to_string()),
            superseded_by: m.superseded_by.clone(),
            content: content.then(|| m.body.clone()),
        }
    }
}
