//! The sites the owner warmed (web.md section 9): a human passed their check or logged
//! in once, in a window on the reader's profile. `<data>/reader-sites.json`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Warmed {
    pub host: String,
    /// Milliseconds since the epoch.
    pub at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    sites: Vec<Warmed>,
}

pub struct Sites {
    path: PathBuf,
    lock: std::sync::Mutex<()>,
}

impl Sites {
    pub fn new(data: &std::path::Path) -> Self {
        Self { path: data.join("reader-sites.json"), lock: std::sync::Mutex::new(()) }
    }

    pub fn list(&self) -> Vec<Warmed> {
        let _held = self.lock.lock().unwrap();
        self.read().sites
    }

    pub fn add(&self, host: &str, at: u64) -> std::io::Result<()> {
        let _held = self.lock.lock().unwrap();
        let mut file = self.read();
        file.sites.retain(|s| s.host != host);
        file.sites.push(Warmed { host: host.to_string(), at });
        self.write(&file)
    }

    pub fn remove(&self, host: &str) -> std::io::Result<()> {
        let _held = self.lock.lock().unwrap();
        let mut file = self.read();
        file.sites.retain(|s| s.host != host);
        self.write(&file)
    }

    pub fn clear(&self) -> std::io::Result<()> {
        let _held = self.lock.lock().unwrap();
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// A file that is not what it should be reads as an empty list (it is rewritten at the next change).
    fn read(&self) -> File {
        std::fs::read_to_string(&self.path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
    }

    fn write(&self, file: &File) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(file).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_added_once_listed_forgotten_and_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let sites = Sites::new(dir.path());
        assert!(sites.list().is_empty());
        sites.add("a.example.com", 1).unwrap();
        sites.add("b.example.org", 2).unwrap();
        sites.add("a.example.com", 3).unwrap();
        assert_eq!(sites.list(), vec![Warmed { host: "b.example.org".into(), at: 2 }, Warmed { host: "a.example.com".into(), at: 3 }]);
        sites.remove("b.example.org").unwrap();
        assert_eq!(sites.list().len(), 1);
        sites.clear().unwrap();
        sites.clear().unwrap();
        assert!(sites.list().is_empty());
        std::fs::write(dir.path().join("reader-sites.json"), "not json").unwrap();
        assert!(sites.list().is_empty(), "a damaged file is an empty list");
    }
}
