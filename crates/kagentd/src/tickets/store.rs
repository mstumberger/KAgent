//! The files of the tickets (tickets.md section 3): one JSON file per project, written atomically.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Ticket, TicketError, Tickets};

/// The prefix of the Inbox.
pub const INBOX_PREFIX: &str = "IN";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectFile {
    pub project: Option<String>,
    pub prefix: String,
    pub next: u64,
    pub tickets: Vec<Ticket>,
}

/// The id (file stem) of a project: `inbox`, or `<name>-<8 hex of the sha-256 of the path>`.
pub fn project_id(project: Option<&str>) -> String {
    let Some(path) = project else { return "inbox".into() };
    let name: String = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(24)
        .collect();
    let digest = Sha256::digest(path.as_bytes());
    let hex: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{}-{hex}", name.trim_matches('-'))
}

/// The display name of a project.
pub fn project_name(project: Option<&str>) -> String {
    match project {
        None => "Inbox".into(),
        Some(p) => Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string()),
    }
}

/// The prefix a folder name suggests (tickets.md section 3): initials of its words, 2 to 4 letters; a one-word name gives
/// its first two letters; no letters gives `PR`.
pub fn suggest_prefix(name: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && previous_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_ascii_lowercase();
        current.push(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    let letters = |w: &str| w.chars().filter(char::is_ascii_alphabetic).collect::<String>();
    let initials: String = words.iter().filter_map(|w| w.chars().find(char::is_ascii_alphabetic)).map(|c| c.to_ascii_uppercase()).take(4).collect();
    if initials.len() >= 2 {
        return initials;
    }
    let first: String = words.iter().map(|w| letters(w)).find(|l| !l.is_empty()).unwrap_or_default();
    let two: String = first.chars().take(2).map(|c| c.to_ascii_uppercase()).collect();
    if two.len() == 2 { two } else { "PR".into() }
}

impl Tickets {
    fn path_of(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Every project file that can be read, in the order of their ids; damaged ones are named in the second list
    /// (never overwritten).
    pub(super) fn load_all(&self) -> (Vec<ProjectFile>, Vec<PathBuf>) {
        let mut files = Vec::new();
        let mut damaged = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return (files, damaged) };
        let mut paths: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        paths.sort();
        for path in paths {
            match std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<ProjectFile>(&b).ok()) {
                Some(mut file) => {
                    for t in &mut file.tickets {
                        t.project = file.project.clone();
                    }
                    files.push(file);
                }
                None => {
                    tracing::warn!("the tickets file {} is damaged: skipped, left as it is", path.display());
                    damaged.push(path);
                }
            }
        }
        (files, damaged)
    }

    /// The file of a project: `Ok(None)` when it does not exist yet; an error when it exists and cannot be read.
    pub(super) fn load(&self, project: Option<&str>) -> Result<Option<ProjectFile>, TicketError> {
        let path = self.path_of(&project_id(project));
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(TicketError::Failed(format!("cannot read the tickets file {}: {e}", path.display()))),
        };
        let mut file: ProjectFile = serde_json::from_slice(&bytes)
            .map_err(|_| TicketError::Failed(format!("the tickets file of {} is damaged: {}", project_name(project), path.display())))?;
        for t in &mut file.tickets {
            t.project = file.project.clone();
        }
        Ok(Some(file))
    }

    /// Write the file of a project atomically: a temporary file in the same folder (0600), then rename.
    pub(super) fn save(&self, file: &ProjectFile) -> Result<(), TicketError> {
        let fail = |e: &dyn std::fmt::Display| TicketError::Failed(format!("cannot write the tickets of {}: {e}", project_name(file.project.as_deref())));
        std::fs::create_dir_all(&self.dir).map_err(|e| fail(&e))?;
        let path = self.path_of(&project_id(file.project.as_deref()));
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| fail(&e))?;
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&tmp).map_err(|e| fail(&e))?;
            f.write_all(&bytes).map_err(|e| fail(&e))?;
            f.sync_all().map_err(|e| fail(&e))?;
        }
        std::fs::rename(&tmp, &path).map_err(|e| fail(&e))
    }

    /// A new, empty file for a project, with a prefix no other project has.
    pub(super) fn fresh(&self, project: Option<&str>) -> ProjectFile {
        let taken: Vec<String> = self.load_all().0.into_iter().map(|f| f.prefix).collect();
        let base = match project {
            None => INBOX_PREFIX.to_string(),
            Some(_) => suggest_prefix(&project_name(project)),
        };
        let mut prefix = base.clone();
        let mut n = 2;
        while taken.contains(&prefix) || (project.is_some() && prefix == INBOX_PREFIX) {
            prefix = format!("{base}{n}");
            n += 1;
        }
        ProjectFile { project: project.map(str::to_string), prefix, next: 1, tickets: Vec::new() }
    }
}
