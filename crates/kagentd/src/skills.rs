//! The daemon's skills library: one copy behind a lock, read for the prompt of every
//! turn, by `load_skill` and by the skills API, read again when a skill changed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Utc};
use kagent_core::agent::TurnOpts;
use kagent_core::protocol::AgentEvent;
use kagent_core::skills::{frontmatter, LoadSkill, SkillCounter, SkillEntry, SkillLibrary, SkillRoot, SkillSource, tool::expand_dirs};
use kagent_core::tool::Tool;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::sections::SkillsSection;
use crate::skills_sources::{ImportedFrom, SourceStore};

/// A skill the library holds, as an import or a source checks against it: its name,
/// where it comes from, and its folder with every link resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub source: SkillSource,
    pub real_dir: PathBuf,
}

#[derive(Clone)]
pub struct SkillHub {
    library: Arc<RwLock<SkillLibrary>>,
    events: broadcast::Sender<AgentEvent>,
    counter: Arc<SkillCounter>,
    /// The store of skill sources: it says where an imported skill came from.
    sources: Option<Arc<SourceStore>>,
}

/// One skill as the API shows it (api.md section 6).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when_to_use: String,
    pub source: SkillSource,
    /// The skill's folder as found; `real_path` is it with every link resolved.
    pub path: PathBuf,
    pub real_path: PathBuf,
    pub enabled: bool,
    pub shadowed: bool,
    pub size_bytes: u64,
    /// Only where the API sends it (`GET /api/skills/:name`, a write): as stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub uses: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_used: String,
    pub symlink: bool,
    /// When `SKILL.md` was last written, RFC 3339; empty when unknown.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    /// An imported skill: where it came from, when KAgent recorded it (S21).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<ImportedFrom>,
}

impl SkillHub {
    /// Scan every root once. `roots` in any order. Uses are counted in `usage_path`.
    pub fn new(roots: &[SkillRoot], disabled: &[String], events: broadcast::Sender<AgentEvent>, usage_path: impl Into<PathBuf>) -> Self {
        let library = SkillLibrary::scan(roots, disabled);
        tracing::info!(
            "skills: {} enabled of {} found, roots {:?}",
            library.available_count(),
            library.all().count(),
            library.roots().iter().map(|r| r.path.display().to_string()).collect::<Vec<_>>()
        );
        Self { library: Arc::new(RwLock::new(library)), events, counter: Arc::new(SkillCounter::load(usage_path)), sources: None }
    }

    /// With the store of skill sources: an imported skill then says where it came from.
    pub fn with_sources(mut self, sources: Arc<SourceStore>) -> Self {
        self.sources = Some(sources);
        self
    }

    /// Every skill the library holds, the shadowed ones included: what "installed"
    /// and "name exists" mean for an import (skills.md sections 7 and 8).
    pub fn installed(&self) -> Vec<Installed> {
        self.library.read().unwrap().all().map(|e| Installed { name: e.name.clone(), source: e.source, real_dir: real_dir_of(&e.dir) }).collect()
    }

    /// The `load_skill` tool over this library.
    pub fn tool(&self) -> LoadSkill {
        LoadSkill::new(self.library.clone(), self.counter.clone())
    }

    pub fn library(&self) -> &Arc<RwLock<SkillLibrary>> {
        &self.library
    }

    /// How often each skill was loaded: read by the API, written by `load_skill` and by
    /// a message that names a skill.
    pub fn counter(&self) -> &Arc<SkillCounter> {
        &self.counter
    }

    /// The folder of `source`, as the library scans it.
    pub fn root(&self, source: SkillSource) -> PathBuf {
        self.library.read().unwrap().roots().iter().find(|r| r.source == source).map(|r| r.path.clone()).unwrap_or_default()
    }

    /// First thing the config closure does (skills.md section 3): rescan when a skill
    /// was added, removed or edited, or `skills.disabled` changed, and tell the UI.
    pub fn refresh(&self, skills: &SkillsSection) -> bool {
        let mut library = self.library.write().unwrap();
        if !library.refresh_if_changed(&skills.disabled) {
            return false;
        }
        let count = library.available_count();
        tracing::info!("skills changed: {count} enabled");
        let _ = self.events.send(AgentEvent::SkillsChanged { count });
        true
    }

    /// The prompt index, within `skills.indexBudgetChars`.
    pub fn index(&self, budget_chars: u32) -> String {
        self.library.read().unwrap().index(budget_chars as usize)
    }

    /// Read every root again now, and tell the UI: after a skill was written or removed
    /// by `skill_save`, `skill_delete` or the API. Answers with the enabled count.
    pub fn reload(&self) -> usize {
        self.notifier()();
        self.available_count()
    }

    pub fn available_count(&self) -> usize {
        self.library.read().unwrap().available_count()
    }

    /// Every skill found, winners first (user, imported, bundled, each alphabetical),
    /// then those they hide. No bodies.
    pub fn views(&self) -> Vec<SkillView> {
        let library = self.library.read().unwrap();
        library.all().map(|e| self.view_of(e, false)).collect()
    }

    /// The skill that owns `name`, enabled or not; `None` for an unknown name.
    pub fn view(&self, name: &str, with_body: bool) -> Option<SkillView> {
        let library = self.library.read().unwrap();
        let found = library.all().find(|e| e.name == name).map(|e| self.view_of(e, with_body));
        found
    }

    fn view_of(&self, entry: &SkillEntry, with_body: bool) -> SkillView {
        let use_ = self.counter.get(&entry.name);
        let symlink = std::fs::symlink_metadata(&entry.dir).is_ok_and(|m| m.file_type().is_symlink());
        SkillView {
            real_path: real_dir_of(&entry.dir),
            // A skill of the owner never says it was imported, whatever a stale record names.
            imported_from: (entry.source == SkillSource::Imported).then(|| self.sources.as_ref()?.provenance(&entry.name)).flatten(),
            body: with_body.then(|| entry.body.clone()),
            uses: use_.as_ref().map(|u| u.uses).unwrap_or(0),
            last_used: use_.map(|u| u.last_used).unwrap_or_default(),
            updated_at: std::fs::metadata(&entry.path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| DateTime::<Utc>::from(t).to_rfc3339())
                .unwrap_or_default(),
            symlink,
            name: entry.name.clone(),
            description: entry.description.clone(),
            when_to_use: entry.when_to_use.clone(),
            source: entry.source,
            path: entry.dir.clone(),
            enabled: entry.enabled,
            shadowed: entry.shadowed,
            size_bytes: entry.size_bytes,
        }
    }

    /// The skill a message names with `/<name>` (skills.md section 6): an enabled one
    /// only. Its name and its body with the directory variables expanded.
    pub fn named(&self, text: &str) -> Option<(String, String)> {
        let name = named_skill(text)?;
        let library = self.library.read().unwrap();
        let entry = library.get(name).filter(|e| e.available())?;
        Some((entry.name.clone(), expand_dirs(&entry.body, &entry.dir)))
    }

    /// What the turn of that message needs: the skill's body as its prompt block, and
    /// one use counted. Any other message gets the defaults.
    pub fn turn_opts(&self, text: &str) -> TurnOpts {
        let Some((name, body)) = self.named(text) else { return TurnOpts::default() };
        self.counter.record(&name);
        TurnOpts { system_suffix: Some(crate::prompt::skill_block(&name, &body)), ..Default::default() }
    }

    /// `skill_save` and `skill_delete` over the owner's root. After each write or
    /// removal the library is read again, so `load_skill` and the next prompt see it
    /// without waiting for the next turn.
    pub fn author_tools(&self) -> Vec<Arc<dyn Tool>> {
        let library = self.library.clone();
        let user_root = self.root(SkillSource::User);
        let save = kagent_core::skills::author::SkillSave::new(library.clone(), user_root.clone(), self.notifier());
        let delete = kagent_core::skills::author::SkillDelete::new(library, user_root, self.notifier());
        vec![Arc::new(save), Arc::new(delete)]
    }

    fn notifier(&self) -> kagent_core::skills::author::OnChange {
        let (library, events) = (self.library.clone(), self.events.clone());
        Arc::new(move || {
            let count = {
                let mut library = library.write().unwrap();
                library.reload();
                library.available_count()
            };
            tracing::info!("skills changed: {count} enabled");
            let _ = events.send(AgentEvent::SkillsChanged { count });
        })
    }
}

/// The folder with every link resolved; itself when it cannot be resolved.
fn real_dir_of(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// The skill a message names: the text starts with `/<name>` and the name ends at the
/// first whitespace. `None` for any other text, a `/<name>` that is not a valid name
/// included.
pub fn named_skill(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..end];
    frontmatter::valid_name(name).then_some(name)
}


#[cfg(test)]
mod tests;
