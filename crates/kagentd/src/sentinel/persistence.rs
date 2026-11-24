//! Persistence watch (docs/specs/alerts.md section 4): snapshot the places attackers
//! use to come back after a reboot, diff every 10 minutes against a baseline.
//! First run is silent; re-baselining is an explicit owner action.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::watch::{AlertManager, Hit, Severity};

/// item id ("kind:detail") -> content hash ("" when only existence matters).
pub type Snapshot = BTreeMap<String, String>;

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().take(12).map(|x| format!("{x:02x}")).collect()
}

fn files_in(dir: &Path, kind: &str, recursive: bool, snap: &mut Snapshot) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.display().to_string();
        // Files KAgent manages itself are not intrusions.
        if name.contains("kagent") {
            continue;
        }
        let meta = std::fs::symlink_metadata(&p);
        if meta.as_ref().is_ok_and(|m| m.is_dir()) {
            if recursive {
                files_in(&p, kind, true, snap);
            }
            continue;
        }
        let hash = std::fs::read(&p).map(|b| sha(&b)).unwrap_or_default();
        snap.insert(format!("{kind}:{name}"), hash);
    }
}

pub fn take(home: &Path, with_suid: bool) -> Snapshot {
    let mut s = Snapshot::new();
    for line in std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap_or_default().lines() {
        let l = line.trim();
        if !l.is_empty() && !l.starts_with('#') {
            s.insert(format!("authorized_key:{}", sha(l.as_bytes())), String::new());
        }
    }
    files_in(&home.join(".config/systemd/user"), "user_unit", true, &mut s);
    files_in(Path::new("/etc/systemd/system"), "system_unit", true, &mut s);
    files_in(&home.join(".config/autostart"), "autostart", false, &mut s);
    files_in(Path::new("/etc/xdg/autostart"), "autostart", false, &mut s);
    for d in ["/etc/cron.d", "/etc/cron.daily", "/etc/cron.hourly", "/etc/cron.weekly"] {
        files_in(Path::new(d), "cron", false, &mut s);
    }
    files_in(Path::new("/etc/sudoers.d"), "sudoers", false, &mut s);
    files_in(Path::new("/etc/pam.d"), "pam", false, &mut s);
    if let Ok(out) = std::process::Command::new("crontab").arg("-l").output() {
        if out.status.success() && !out.stdout.is_empty() {
            s.insert("crontab:user".into(), sha(&out.stdout));
        }
    }
    s.insert("ld_preload:/etc/ld.so.preload".into(), std::fs::read("/etc/ld.so.preload").map(|b| sha(&b)).unwrap_or_else(|_| "absent".into()));
    for rc in [".bashrc", ".profile", ".bash_profile", ".zshrc", ".config/fish/config.fish"] {
        if let Ok(b) = std::fs::read(home.join(rc)) {
            s.insert(format!("shell_rc:{}", home.join(rc).display()), sha(&b));
        }
    }
    let taint = std::fs::read_to_string("/proc/sys/kernel/tainted").unwrap_or_default().trim().parse::<u64>().unwrap_or(0);
    // Bits 12 (O, out-of-tree) and 13 (E, unsigned) mark foreign kernel code.
    s.insert("taint:foreign_modules".into(), ((taint >> 12) & 0b11).to_string());
    if with_suid {
        if let Ok(out) = std::process::Command::new("find").args(["/usr", "/opt", "/home", "-xdev", "-perm", "-4000", "-type", "f"]).output() {
            for l in String::from_utf8_lossy(&out.stdout).lines() {
                s.insert(format!("suid:{l}"), String::new());
            }
        }
    }
    s
}

/// Added or changed items -> hits. Removals are not intrusions (uninstalls).
pub fn diff(base: &Snapshot, now: &Snapshot, suid_scanned: bool) -> Vec<Hit> {
    let mut out = Vec::new();
    for (k, v) in now {
        let (kind, detail) = k.split_once(':').unwrap_or((k, ""));
        if kind == "suid" && !suid_scanned {
            continue;
        }
        let change = match base.get(k) {
            None => "new",
            Some(old) if old != v => "changed",
            _ => continue,
        };
        if kind == "ld_preload" && v == "absent" {
            continue;
        }
        if kind == "taint" && v == "0" {
            continue;
        }
        let (rule, severity, title, proposal) = match kind {
            "authorized_key" => ("persist-authorized-keys", Severity::High, "New SSH key in ~/.ssh/authorized_keys".to_string(), "if you did not add it: remove it, check logins (journal unit=ssh) and rotate keys"),
            "user_unit" | "system_unit" => {
                let content = std::fs::read_to_string(detail).unwrap_or_default();
                let suspicious = content.lines().filter(|l| l.trim_start().starts_with("Exec")).any(|l| l.contains("/tmp/") || l.contains("/dev/shm") || l.contains("/.cache/") || l.contains("/."));
                ("persist-unit", if suspicious { Severity::High } else { Severity::Review }, format!("Systemd unit {change}: {detail}"), "read the unit (read_file); if unknown: systemd stop + disable it and find who created it")
            }
            "autostart" => ("persist-autostart", Severity::Review, format!("Autostart entry {change}: {detail}"), "check the Exec line of the .desktop file"),
            "cron" | "crontab" => ("persist-cron", Severity::Review, format!("Scheduled job {change}: {detail}"), "inspect the cron entry"),
            "suid" => ("persist-suid", Severity::High, format!("New setuid-root program: {detail}"), "check its package owner (OWNER_CMD) and hash; unowned SUID binaries are a classic backdoor"),
            "ld_preload" => ("persist-preload", Severity::High, "/etc/ld.so.preload changed".to_string(), "every program now loads that library: read the file, remove unknown entries"),
            "sudoers" | "pam" => ("sudoers-change", Severity::High, format!("Authentication config {change}: {detail}"), "check who changed sudo/PAM config (package history: what_changed, journal)"),
            "shell_rc" => ("persist-shell-rc", Severity::Info, format!("Shell startup file {change}: {detail}"), "if you did not edit it, look for curl/wget/nc or base64 lines"),
            "taint" => ("module-foreign", Severity::Review, "Out-of-tree or unsigned kernel module loaded".to_string(), "kernel_info section=modules / taint: NVIDIA or VirtualBox explain this; anything else, investigate"),
            _ => continue,
        };
        out.push(Hit {
            rule,
            key: format!("{rule}:{k}"),
            severity,
            title,
            cause: format!("{kind} {change} since the baseline"),
            evidence: k.clone(),
            proposal: proposal.replace("OWNER_CMD", kagent_core::platform::package_manager().owner_cmd()),
            threshold: None,
            cooldown: Duration::from_secs(24 * 3600),
        });
    }
    out
}

pub fn baseline_path(data: &Path) -> PathBuf {
    data.join("sentinel/baseline.json")
}

pub fn rebaseline(data: &Path, home: &Path) -> anyhow::Result<usize> {
    let snap = take(home, true);
    std::fs::create_dir_all(baseline_path(data).parent().unwrap())?;
    std::fs::write(baseline_path(data), serde_json::to_vec_pretty(&snap)?)?;
    Ok(snap.len())
}

pub async fn watch(data: PathBuf, home: PathBuf, alerts: Arc<AlertManager>, cancel: CancellationToken) -> anyhow::Result<()> {
    let mut round = 0u64;
    loop {
        let with_suid = round % 6 == 0; // SUID scan hourly (slow-ish)
        let (d, h) = (data.clone(), home.clone());
        let now = tokio::task::spawn_blocking(move || take(&h, with_suid)).await?;
        let base: Option<Snapshot> = std::fs::read(baseline_path(&d)).ok().and_then(|b| serde_json::from_slice(&b).ok());
        match base {
            None => {
                std::fs::create_dir_all(baseline_path(&data).parent().unwrap())?;
                std::fs::write(baseline_path(&data), serde_json::to_vec_pretty(&now)?)?;
                tracing::info!("persistence baseline taken ({} items)", now.len());
            }
            Some(base) => {
                for hit in diff(&base, &now, with_suid) {
                    alerts.hit(hit);
                }
            }
        }
        round += 1;
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(600)) => {}, _ = cancel.cancelled() => return Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_rules() {
        let mut base = Snapshot::new();
        base.insert("ld_preload:/etc/ld.so.preload".into(), "absent".into());
        base.insert("shell_rc:/h/.bashrc".into(), "aaa".into());
        base.insert("suid:/usr/bin/sudo".into(), String::new());
        let mut now = base.clone();
        now.insert("authorized_key:abc".into(), String::new());
        now.insert("suid:/usr/local/bin/x".into(), String::new());
        now.insert("shell_rc:/h/.bashrc".into(), "bbb".into());
        now.insert("ld_preload:/etc/ld.so.preload".into(), "1234".into());
        now.remove("suid:/usr/bin/sudo"); // removals are ignored
        let hits = diff(&base, &now, true);
        let rules: Vec<(&str, Severity)> = hits.iter().map(|h| (h.rule, h.severity)).collect();
        assert!(rules.contains(&("persist-authorized-keys", Severity::High)));
        assert!(rules.contains(&("persist-suid", Severity::High)));
        assert!(rules.contains(&("persist-preload", Severity::High)));
        assert!(rules.contains(&("persist-shell-rc", Severity::Info)));
        assert_eq!(hits.len(), 4);
        // SUID items are ignored on rounds without a SUID scan.
        assert_eq!(diff(&base, &now, false).len(), 3);
    }

    #[test]
    fn snapshot_of_a_fake_home() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
        std::fs::write(home.path().join(".ssh/authorized_keys"), "ssh-ed25519 AAAA me\n# comment\n").unwrap();
        std::fs::create_dir_all(home.path().join(".config/systemd/user")).unwrap();
        std::fs::write(home.path().join(".config/systemd/user/x.service"), "[Service]\nExecStart=/tmp/x\n").unwrap();
        std::fs::write(home.path().join(".config/systemd/user/kagent-heartbeat.timer"), "x").unwrap();
        let s = take(home.path(), false);
        assert_eq!(s.keys().filter(|k| k.starts_with("authorized_key:")).count(), 1);
        assert!(s.keys().any(|k| k.ends_with("x.service")));
        assert!(!s.keys().any(|k| k.contains("kagent")), "own files ignored");
        let hits = diff(&Snapshot::new(), &s, false);
        let unit = hits.iter().find(|h| h.rule == "persist-unit" && h.title.ends_with("x.service")).unwrap();
        assert_eq!(unit.severity, Severity::High, "ExecStart in /tmp is suspicious");
    }
}
