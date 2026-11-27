//! What the module remembers between runs, apart from the secrets (calendar.md sections 2
//! and 6): where `home` was resolved to, and the morning report's book. One small JSON file
//! under `<data>`; a damaged or missing one is a clean start, never an error.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Saved {
    /// The `home` text the coordinates belong to.
    pub home_for: String,
    pub home_lat_lng: Option<[f64; 2]>,
    /// `sl` or `en`: the language of the last thing the owner wrote (`persona.language: auto`).
    pub last_language: String,
    /// The last report made for delivery.
    pub last_report: Option<LastReport>,
    /// The day (`2026-11-27`) whose report was delivered or given up: once per day.
    pub report_day: String,
    /// The owner was told once that the report is skipped (not connected).
    pub told_skip: bool,
    /// A report that waits for the first live voice session.
    pub spoken: Option<Spoken>,
}

/// What the page shows of the last report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastReport {
    /// RFC 3339.
    pub at: String,
    pub text: String,
    /// `telegram`, `spoken`.
    pub sent: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spoken {
    pub day: String,
    pub text: String,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("calendar-state.json")
}

pub fn load(data_dir: &Path) -> Saved {
    let file = path(data_dir);
    match std::fs::read(&file) {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|e| {
            tracing::warn!("{}: not read, starting clean: {e}", file.display());
            Saved::default()
        }),
        Err(_) => Saved::default(),
    }
}

pub fn save(data_dir: &Path, saved: &Saved) {
    let file = path(data_dir);
    let write = || -> anyhow::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(saved)?)?;
        std::fs::rename(&tmp, &file)?;
        Ok(())
    };
    if let Err(e) = write() {
        tracing::warn!("calendar: the state could not be saved: {e:#}");
    }
}
