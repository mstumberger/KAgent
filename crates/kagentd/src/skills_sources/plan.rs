//! The sync plan (what installing a source changes, from the hashes) and what doing
//! it writes to the imported folder.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

use chrono::Utc;

use crate::skills_import::{clear_staging, reason_of, stage, NAME_EXISTS, Skipped};

use super::read::{hash_of_skill, Found};
use super::{SkillRecord, SourceRecord};

// ------------------------------------------------------------------ the sync plan

/// What installing a source does. Decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncPlan {
    pub added: Vec<Found>,
    pub updated: Vec<Found>,
    /// Copies whose skill left the source.
    pub removed: Vec<String>,
    /// A copy with the same content is in place already (an interrupted sync, or the
    /// source added again with its skills kept): it is only recorded.
    pub adopted: Vec<Found>,
    /// What is not installed, with why.
    pub skipped: Vec<Skipped>,
}

/// Compare what the source holds now (`found`) with what it installed last (`record`).
/// `taken` says whether a skill of another root has the name: an import never takes it.
pub fn plan_sync(record: &SourceRecord, found: &[Found], imported_root: &Path, taken: &dyn Fn(&str) -> bool) -> SyncPlan {
    let mut plan = SyncPlan::default();
    let before: BTreeMap<&str, &SkillRecord> = record.skills.iter().map(|s| (s.name.as_str(), s)).collect();
    let mut names = BTreeSet::new();
    for skill in found {
        names.insert(skill.name.as_str());
        let missing = std::fs::symlink_metadata(imported_root.join(&skill.name)).is_err();
        match before.get(skill.name.as_str()) {
            // Unchanged: it stays as it is, on or off. A copy that was removed comes back, off.
            Some(old) if old.hash == skill.hash => {
                if missing && !taken(&skill.name) {
                    plan.added.push(skill.clone());
                }
            }
            Some(_) => plan.updated.push(skill.clone()),
            None if taken(&skill.name) => plan.skipped.push(Skipped { name: skill.name.clone(), reason: NAME_EXISTS.into() }),
            None if !missing && hash_of_skill(&imported_root.join(&skill.name)).is_ok_and(|h| h == skill.hash) => plan.adopted.push(skill.clone()),
            None if !missing => plan.skipped.push(Skipped { name: skill.name.clone(), reason: NAME_EXISTS.into() }),
            None => plan.added.push(skill.clone()),
        }
    }
    plan.removed = record.skills.iter().filter(|s| !names.contains(s.name.as_str())).map(|s| s.name.clone()).collect();
    plan
}

fn at_record(skill: &Found, at: &str) -> SkillRecord {
    SkillRecord { name: skill.name.clone(), path: skill.dir.clone(), hash: skill.hash.clone(), synced_at: at.to_string() }
}

/// Do `plan` under `imported_root` and make `record` say what is installed now. A
/// skill that cannot be written or removed is in the answer with why (and stays to do);
/// the others are done. The record is not saved here.
pub async fn install(plan: &SyncPlan, record: &mut SourceRecord, imported_root: &Path) -> Vec<Skipped> {
    let at = Utc::now().to_rfc3339();
    let mut failed = Vec::new();
    let writes = !plan.added.is_empty() || !plan.updated.is_empty();
    if writes {
        clear_staging(imported_root).await;
    }
    let mut keep = |skill: &Found| {
        record.skills.retain(|s| s.name != skill.name);
        record.skills.push(at_record(skill, &at));
    };
    for skill in &plan.adopted {
        keep(skill);
    }
    for skill in &plan.added {
        match stage(&skill.dir, &imported_root.join(&skill.name), imported_root, false).await {
            Ok(()) => keep(skill),
            Err(e) => failed.push(Skipped { name: skill.name.clone(), reason: reason_of(&e) }),
        }
    }
    for skill in &plan.updated {
        match stage(&skill.dir, &imported_root.join(&skill.name), imported_root, true).await {
            Ok(()) => keep(skill),
            Err(e) => failed.push(Skipped { name: skill.name.clone(), reason: reason_of(&e) }),
        }
    }
    for name in &plan.removed {
        match remove_copy(&imported_root.join(name), imported_root).await {
            Ok(()) => record.skills.retain(|s| &s.name != name),
            Err(e) => failed.push(Skipped { name: name.clone(), reason: format!("cannot be removed: {e}") }),
        }
    }
    if writes {
        clear_staging(imported_root).await;
    }
    failed
}

/// Remove a copy a source installed: a real folder inside the imported folder, nothing
/// else (a link of that name is left alone).
async fn remove_copy(path: &Path, root: &Path) -> io::Result<()> {
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other("it is not a folder KAgent made"));
    }
    match (std::fs::canonicalize(path), std::fs::canonicalize(root)) {
        (Ok(real), Ok(base)) if real.starts_with(&base) && real != base => tokio::fs::remove_dir_all(path).await,
        _ => Err(io::Error::other("it is not inside the imported folder")),
    }
}

