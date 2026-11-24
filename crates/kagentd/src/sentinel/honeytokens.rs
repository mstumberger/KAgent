//! Honeytokens (docs/specs/alerts.md section 4): decoy credentials nothing legitimate
//! ever opens. Any open/read/modify is a high-confidence intrusion signal.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use inotify::{Inotify, WatchMask};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::watch::{AlertManager, Hit, Severity};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Token {
    pub path: PathBuf,
    pub kind: String,
    pub marker: String,
}

fn marker() -> String {
    format!("kagent-ht-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

fn rand_alnum(n: usize, upper: bool) -> String {
    use rand::Rng;
    let set: &[u8] = if upper { b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567" } else { b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789" };
    let mut r = rand::rng();
    (0..n).map(|_| set[r.random_range(0..set.len())] as char).collect()
}

/// Decoy contents: plausible enough to be stolen, carrying the marker so a leak is
/// recognisable, and never valid anywhere.
pub fn decoys(home: &Path) -> Vec<(Token, String)> {
    let mut out = Vec::new();
    let m = marker();
    out.push((
        Token { path: home.join(".aws/credentials.legacy"), kind: "aws".into(), marker: m.clone() },
        format!("[backup]\naws_access_key_id = AKIA{}\naws_secret_access_key = {}{}\nregion = eu-central-1\n", rand_alnum(16, true), rand_alnum(24, false), m.replace('-', "")),
    ));
    let m = marker();
    out.push((
        Token { path: home.join(".config/backup.env"), kind: "env".into(), marker: m.clone() },
        format!("# old deployment secrets\nOPENAI_API_KEY=sk-proj-{}{}\nDB_PASSWORD={}\nSTRIPE_SECRET_KEY=sk_live_{}\n", rand_alnum(20, false), m.replace('-', ""), rand_alnum(18, false), rand_alnum(24, false)),
    ));
    let m = marker();
    out.push((
        Token { path: home.join(".ssh/id_backup"), kind: "ssh".into(), marker: m.clone() },
        // Not a usable key: base64-looking body that no server will ever accept.
        format!("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW\n{}\n{}\n-----END OPENSSH PRIVATE KEY-----\n", rand_alnum(70, false), m),
    ));
    let m = marker();
    out.push((
        Token { path: home.join("Documents/passwords-old.kdbx"), kind: "kdbx".into(), marker: m.clone() },
        // KeePass 2 magic + random-looking payload (written as text bytes; still opaque).
        format!("\u{3}\u{d9}\u{a2}\u{9a}g\u{fb}K\u{b5}{}{}", rand_alnum(4000, false), m),
    ));
    out
}

pub fn registry_path(data: &Path) -> PathBuf {
    data.join("sentinel/honeytokens.json")
}

pub fn load(data: &Path) -> Vec<Token> {
    std::fs::read(registry_path(data)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Create the decoys that do not exist yet. Never overwrites a real file.
pub fn enable(data: &Path, home: &Path) -> anyhow::Result<Vec<Token>> {
    let mut reg = load(data);
    for (t, content) in decoys(home) {
        if reg.iter().any(|r| r.path == t.path) || t.path.exists() {
            continue;
        }
        if let Some(dir) = t.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&t.path, content)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&t.path, std::fs::Permissions::from_mode(0o600))?;
        reg.push(t);
    }
    std::fs::create_dir_all(registry_path(data).parent().unwrap())?;
    std::fs::write(registry_path(data), serde_json::to_vec_pretty(&reg)?)?;
    Ok(reg)
}

/// Remove decoys we created (only if they still carry our marker).
pub fn disable(data: &Path) -> anyhow::Result<usize> {
    let reg = load(data);
    let mut n = 0;
    for t in &reg {
        // Some decoys embed the marker without dashes (inside fake keys).
        let ours = std::fs::read(&t.path)
            .map(|b| {
                let c = String::from_utf8_lossy(&b);
                c.contains(&t.marker) || c.contains(&t.marker.replace('-', ""))
            })
            .unwrap_or(false);
        if ours {
            std::fs::remove_file(&t.path)?;
            n += 1;
        }
    }
    let _ = std::fs::remove_file(registry_path(data));
    Ok(n)
}

/// Best effort: processes holding the file open right now (fast readers may be gone).
fn openers(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let pid = e.file_name().to_string_lossy().to_string();
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else { continue };
        if fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|l| l == path)) {
            let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let exe = std::fs::read_link(e.path().join("exe")).map(|p| p.display().to_string()).unwrap_or_default();
            let ppid = std::fs::read_to_string(e.path().join("stat")).ok().and_then(|s| s.rsplit(')').next().and_then(|r| r.split_whitespace().nth(1).map(String::from))).unwrap_or_default();
            out.push(format!("pid {pid} {} ({exe}) parent {ppid}", comm.trim()));
        }
    }
    out
}

pub async fn watch(tokens: Vec<Token>, alerts: Arc<AlertManager>, cancel: CancellationToken) -> anyhow::Result<()> {
    if tokens.is_empty() {
        return Ok(());
    }
    let inotify = Inotify::init()?;
    let mut by_wd = HashMap::new();
    for t in &tokens {
        let mask = WatchMask::OPEN | WatchMask::ACCESS | WatchMask::MODIFY | WatchMask::ATTRIB | WatchMask::DELETE_SELF | WatchMask::MOVE_SELF;
        match inotify.watches().add(&t.path, mask) {
            Ok(wd) => {
                by_wd.insert(wd, t.clone());
            }
            Err(e) => tracing::warn!(path = %t.path.display(), "cannot watch honeytoken: {e}"),
        }
    }
    tracing::info!("watching {} honeytokens", by_wd.len());
    let mut stream = inotify.into_event_stream([0u8; 4096])?;
    loop {
        tokio::select! {
            ev = stream.next() => {
                let Some(ev) = ev else { anyhow::bail!("inotify stream ended") };
                let ev = ev?;
                let Some(t) = by_wd.get(&ev.wd) else { continue };
                let what = if ev.mask.intersects(inotify::EventMask::MODIFY | inotify::EventMask::ATTRIB | inotify::EventMask::DELETE_SELF | inotify::EventMask::MOVE_SELF) { "modified/moved" } else { "opened" };
                let who = openers(&t.path);
                let rule = if what == "opened" { "honeytoken-access" } else { "honeytoken-modified" };
                alerts.hit(Hit {
                    rule,
                    key: format!("{rule}:{}", t.path.display()),
                    severity: Severity::High,
                    title: format!("Decoy {} file {what}: {}", t.kind, t.path.display()),
                    cause: "a honeytoken was touched; nothing legitimate reads these files".into(),
                    evidence: if who.is_empty() { format!("{:?} (reader already gone; enable eBPF attribution for pid/parent)", ev.mask) } else { who.join("\n") },
                    proposal: "check the process tree and recent logins; if unexpected: disconnect the network, look at persistence alerts, rotate real credentials".into(),
                    threshold: None,
                    cooldown: Duration::from_secs(600),
                });
                if ev.mask.intersects(inotify::EventMask::DELETE_SELF | inotify::EventMask::MOVE_SELF) {
                    anyhow::bail!("honeytoken {} removed; restart watcher", t.path.display());
                }
            }
            _ = cancel.cancelled() => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_never_overwrites_and_disable_removes_only_ours() {
        let data = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".config")).unwrap();
        std::fs::write(home.path().join(".config/backup.env"), "REAL=1").unwrap();
        let reg = enable(data.path(), home.path()).unwrap();
        assert_eq!(reg.len(), 3, "existing real file skipped");
        assert_eq!(std::fs::read_to_string(home.path().join(".config/backup.env")).unwrap(), "REAL=1");
        let aws = std::fs::read_to_string(home.path().join(".aws/credentials.legacy")).unwrap();
        assert!(aws.contains("AKIA") && aws.contains(&reg[0].marker.replace('-', "")));
        assert_eq!(enable(data.path(), home.path()).unwrap().len(), 3, "idempotent");
        assert_eq!(disable(data.path()).unwrap(), 3);
        assert!(!home.path().join(".aws/credentials.legacy").exists());
        assert!(home.path().join(".config/backup.env").exists());
    }

    #[tokio::test]
    async fn opening_a_token_raises_a_high_alert() {
        let data = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let reg = enable(data.path(), home.path()).unwrap();
        let (tx, _) = tokio::sync::broadcast::channel(16);
        let alerts = AlertManager::new(data.path().join("alerts.json"), tx, None, false);
        let cancel = CancellationToken::new();
        let task = tokio::spawn(watch(reg.clone(), alerts.clone(), cancel.clone()));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = std::fs::read_to_string(&reg[0].path).unwrap();
        for _ in 0..40 {
            if alerts.open_count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cancel.cancel();
        let _ = task.await;
        let a = alerts.list(true, 10);
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].rule.as_str(), a[0].severity), ("honeytoken-access", Severity::High));
    }
}
