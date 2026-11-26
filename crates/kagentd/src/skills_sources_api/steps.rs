//! The work of the import and sources routes (S21), in steps that take the hubs and the
//! store, never the server's state, so they are tested without one: a step is planned,
//! the caller switches the new skills off (`quarantine` in the parent, before the first
//! file is written), then the step is applied.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use chrono::Utc;
use kagent_core::skills::SkillSource;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::HttpError;
use crate::skills::{Installed, SkillHub};
use crate::skills_api::{broken, conflict, invalid, Refusal};
use crate::skills_import::{self, ImportMode, ImportOutcome, ImportPlan, Skipped};
use crate::skills_sources::{self as sources, GitError, ImportRecord, SourceRecord, SourceStore, SyncPlan};

/// The lock every change to the imported skills holds (see [`SourceStore::lock`]).
type Writers<'a> = tokio::sync::MutexGuard<'a, ()>;

const UPSTREAM: &str = "provider_error";

fn upstream(reason: String) -> Refusal {
    Refusal(StatusCode::BAD_GATEWAY, UPSTREAM, reason)
}

/// git could not do it: the words for the owner.
fn git_refusal(e: GitError) -> Refusal {
    match e {
        GitError::Missing => broken("install git: cloning a source needs the git command".into()),
        GitError::TimedOut => upstream("git did not finish in time".into()),
        GitError::Failed(reason) => upstream(format!("git failed: {reason}")),
    }
}

/// A skill of the owner, or one KAgent ships with, has the name: an import never takes
/// it. (The imported folder is looked at on the disk.)
fn taken(installed: &[Installed]) -> impl Fn(&str) -> bool + '_ {
    |name| installed.iter().any(|s| s.name == name && s.source != SkillSource::Imported)
}

fn unknown_source(id: &str) -> Refusal {
    Refusal(StatusCode::NOT_FOUND, "not_found", format!("unknown source '{id}'"))
}

pub(super) fn internal(e: impl std::fmt::Display) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string())
}

// ------------------------------------------------------------------ import

/// POST `/api/skills/import`, before anything is written: what the request asks for and what it will do.
pub struct ImportStep {
    root: PathBuf,
    plan: ImportPlan,
    mode: ImportMode,
}

/// Read the body `{path, names, mode}` and plan the import. `Err` is the 400; nothing is written.
pub fn import_plan(hub: &SkillHub, body: &Value) -> Result<ImportStep, Refusal> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Body {
        path: String,
        #[serde(default)]
        names: Vec<String>,
        #[serde(default)]
        mode: Option<String>,
    }
    let body: Body = serde_json::from_value(body.clone()).map_err(|e| invalid(format!("expected an object with path, names and mode: {e}")))?;
    if !Path::new(&body.path).is_absolute() {
        return Err(invalid("path must be the absolute path of a folder that holds skills".into()));
    }
    if body.names.is_empty() {
        return Err(invalid("names (the skills to import) must not be empty".into()));
    }
    if let Some(bad) = body.names.iter().find(|n| !kagent_core::skills::frontmatter::valid_name(n)) {
        return Err(invalid(format!("'{bad}' is not a valid skill name")));
    }
    let mode = ImportMode::parse(body.mode.as_deref()).map_err(invalid)?;
    let plan = skills_import::plan_import(Path::new(&body.path), &body.names, mode, &hub.installed(), &hub.root(SkillSource::Imported)).map_err(invalid)?;
    Ok(ImportStep { root: PathBuf::from(body.path), plan, mode })
}

impl ImportStep {
    /// The skills that must be in `skills.disabled` before the first file is written.
    pub fn to_disable(&self) -> Vec<String> {
        self.plan.items.iter().map(|i| i.name.clone()).collect()
    }

    /// Copy or link the skills, remember where each came from, and read the library again.
    pub async fn apply(self, _writers: &Writers<'_>, hub: &SkillHub, store: &SourceStore, home: &Path) -> Result<ImportOutcome, Refusal> {
        let (imported_root, mode, root) = (hub.root(SkillSource::Imported), self.mode, self.root);
        let outcome = skills_import::apply_import(&self.plan, mode, &imported_root).await;
        let records: Vec<ImportRecord> = self
            .plan
            .items
            .iter()
            .filter(|i| outcome.imported.contains(&i.name))
            .map(|i| ImportRecord { name: i.name.clone(), source: skills_import::origin_label(&root, home), path: i.real_dir.clone(), hash: i.hash.clone() })
            .collect();
        // The skills are there; where they came from is a note, not a reason to fail.
        if let Err(e) = store.note_import(&records) {
            tracing::warn!("import: where the skills came from is not recorded: {e}");
        }
        hub.reload();
        Ok(outcome)
    }
}

// ------------------------------------------------------------------ sources

/// POST `/api/skills/sources` and `.../sync`, between reading the source and
/// installing: what will change. Nothing is installed and the record is not saved yet.
pub struct SourceStep {
    record: SourceRecord,
    plan: SyncPlan,
}

/// Clone (when `first`), or fetch and check out, and read the commit that is checked out.
async fn fetch_clone(record: &SourceRecord, dir: &Path, first: bool) -> Result<SourceRecord, Refusal> {
    let mut record = record.clone();
    if first {
        if let Some(parent) = dir.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| broken(format!("cannot create the clones folder: {e}")))?;
        }
        sources::git(&sources::clone_argv(&record.url, dir, &record.r#ref)).await.map_err(git_refusal)?;
    } else {
        sources::git(&sources::fetch_argv(dir, &record.r#ref)).await.map_err(git_refusal)?;
        sources::git(&sources::checkout_argv(dir)).await.map_err(git_refusal)?;
    }
    record.commit = Some(sources::git(&sources::head_argv(dir)).await.map_err(git_refusal)?);
    Ok(record)
}

/// Read a source and decide what installing it does: the limits, the skills, the plan.
/// Nothing is written. `Err` is the message.
pub fn prepare(record: &SourceRecord, root: &Path, imported_root: &Path, installed: &[Installed]) -> Result<SyncPlan, String> {
    if !sources::is_local(&record.url) {
        sources::check_limits(root)?;
    }
    let (found, unreadable) = sources::scan(root, &record.subdir)?;
    if sources::is_local(&record.url) {
        sources::check_limits(&sources::subdir_path(root, &record.subdir)?)?;
    }
    let mut plan = sources::plan_sync(record, &found, imported_root, &taken(installed));
    plan.skipped.splice(0..0, unreadable);
    Ok(plan)
}

async fn read_source(hub: &SkillHub, record: SourceRecord, store: &SourceStore) -> Result<SourceStep, Refusal> {
    let clone = store.clone_dir(&record.id);
    let root = if sources::is_local(&record.url) { PathBuf::from(&record.url) } else { clone.clone() };
    let (planned, imported_root, installed) = (record.clone(), hub.root(SkillSource::Imported), hub.installed());
    // Reading and hashing run off the worker threads: a source may hold 100 skills.
    let plan = tokio::task::spawn_blocking(move || prepare(&planned, &root, &imported_root, &installed)).await.map_err(internal)?.map_err(invalid)?;
    Ok(SourceStep { record, plan })
}

/// What adding a source does, before anything is installed: the request is checked, the
/// repository cloned (an `https://` address) and read. A refusal leaves nothing behind.
pub async fn add_step(
    hub: &SkillHub,
    store: &SourceStore,
    _writers: &Writers<'_>,
    url: Option<&str>,
    r#ref: Option<&str>,
    subdir: Option<&str>,
    name: Option<&str>,
) -> Result<SourceStep, Refusal> {
    let new = sources::validate(url, r#ref, subdir, name).map_err(invalid)?;
    if let Some(there) = store.list().into_iter().find(|s| s.url == new.url) {
        return Err(conflict(format!("{} is a source already ({})", new.url, there.name)));
    }
    let record = SourceRecord {
        id: sources::id_for(&new.url),
        name: new.name,
        url: new.url.clone(),
        r#ref: new.r#ref,
        subdir: new.subdir,
        commit: None,
        last_sync: None,
        last_error: None,
        skills: Vec::new(),
    };
    let clone = store.clone_dir(&record.id);
    let step = if sources::is_local(&new.url) {
        read_source(hub, record, store).await
    } else {
        match fetch_clone(&record, &clone, true).await {
            Ok(record) => read_source(hub, record, store).await,
            Err(refusal) => Err(refusal),
        }
    };
    // No half of a source: a clone that was made goes. A local folder is not KAgent's.
    if step.is_err() {
        if let Err(e) = remove_clone(&clone).await {
            tracing::warn!("sources: the clone of {} is not removed: {e}", clone.display());
        }
    }
    step
}

/// POST `/api/skills/sources/:id/sync`, before anything is installed: fetched (an
/// `https://` address), checked out and read.
pub async fn sync_step(hub: &SkillHub, store: &SourceStore, _writers: &Writers<'_>, id: &str) -> Result<SourceStep, Refusal> {
    let record = store.get(id).ok_or_else(|| unknown_source(id))?;
    if sources::is_local(&record.url) {
        return read_source(hub, record, store).await;
    }
    let clone = store.clone_dir(&record.id);
    match fetch_clone(&record, &clone, false).await {
        Ok(record) => read_source(hub, record, store).await,
        Err(refusal) => Err(refusal),
    }
}

impl SourceStep {
    pub fn id(&self) -> &str {
        &self.record.id
    }

    /// The skills that must be in `skills.disabled` before the first file is written:
    /// the new and the changed ones (changed content is new content).
    pub fn to_disable(&self) -> Vec<String> {
        self.plan.added.iter().chain(&self.plan.updated).map(|s| s.name.clone()).collect()
    }

    /// Install the plan, save the record and read the library again.
    pub async fn apply(mut self, _writers: &Writers<'_>, hub: &SkillHub, store: &SourceStore) -> Result<(SourceRecord, SyncReport), Refusal> {
        let failed = sources::install(&self.plan, &mut self.record, &hub.root(SkillSource::Imported)).await;
        self.record.last_sync = Some(Utc::now().to_rfc3339());
        self.record.last_error = None;
        store.put(&self.record).map_err(|e| broken(format!("cannot save the sources: {e}")))?;
        hub.reload();
        Ok((self.record, SyncReport::of(&self.plan, failed)))
    }
}

/// A sync did not get through: the source that is saved already keeps what it has and
/// says what went wrong.
pub fn note_failure(store: &SourceStore, id: &str, reason: &str) {
    let Some(mut record) = store.get(id) else { return };
    record.last_error = Some(reason.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect());
    if let Err(e) = store.put(&record) {
        tracing::warn!("sources: the failure of '{id}' is not recorded: {e}");
    }
}

/// What a sync, or the first install of a source, did (api.md: `added`, `updated`,
/// `removed`). `skipped` says what was not done, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: Vec<Skipped>,
}

impl SyncReport {
    /// What was done of `plan`. A skill that could not be written or removed is not in
    /// the lists of what was done: it is in `skipped` with why.
    fn of(plan: &SyncPlan, failed: Vec<Skipped>) -> Self {
        let failed_names: BTreeSet<&str> = failed.iter().map(|s| s.name.as_str()).collect();
        let done = |names: Vec<String>| names.into_iter().filter(|n| !failed_names.contains(n.as_str())).collect();
        Self {
            added: done(plan.added.iter().map(|s| s.name.clone()).collect()),
            updated: done(plan.updated.iter().map(|s| s.name.clone()).collect()),
            removed: done(plan.removed.clone()),
            skipped: plan.skipped.iter().cloned().chain(failed).collect(),
        }
    }
}

/// DELETE `/api/skills/sources/:id`: the record and its clone. With `remove_skills`, the
/// copies it installed too.
pub async fn remove_step(hub: &SkillHub, store: &SourceStore, _writers: &Writers<'_>, id: &str, remove_skills: bool) -> Result<(), Refusal> {
    let record = store.get(id).ok_or_else(|| unknown_source(id))?;
    if remove_skills {
        for skill in &record.skills {
            // Only a copy this source installed goes. A skill of the owner that has the
            // name by now, or one imported by hand over a removed copy, stays.
            let ours = hub.view(&skill.name, false).is_some_and(|v| v.source == SkillSource::Imported) && !store.imported_by_hand(&skill.name);
            if ours {
                crate::skills_api::delete(hub, &skill.name).await?;
            }
        }
    }
    if !sources::is_local(&record.url) {
        remove_clone(&store.clone_dir(id)).await.map_err(|e| broken(format!("cannot remove the clone: {e}")))?;
    }
    store.remove(id).map_err(|e| broken(format!("cannot save the sources: {e}")))?;
    hub.reload();
    Ok(())
}

/// Remove the clone of a source: `<data>/skill-sources/<id>`, and nothing else (a link
/// put in its place, or a folder outside, stays). A local path is read where it is and
/// is never removed.
async fn remove_clone(dir: &Path) -> io::Result<()> {
    let real = match std::fs::canonicalize(dir) {
        Ok(real) => real,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let base = std::fs::canonicalize(dir.parent().unwrap_or(dir)).map_err(io::Error::other)?;
    if !real.starts_with(&base) || real == base {
        return Err(io::Error::other("it is not a clone inside the clones folder"));
    }
    tokio::fs::remove_dir_all(&real).await
}

#[cfg(test)]
mod tests;
