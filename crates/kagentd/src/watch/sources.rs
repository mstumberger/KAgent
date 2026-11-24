//! Watcher tasks: a journal stream plus periodic checks. Each runs supervised: a crash
//! restarts it with backoff and after repeated failures raises an info alert.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use super::rules::{self, Entry, Hit, Severity};
use super::AlertManager;

async fn sh(argv: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new(argv[0]).args(&argv[1..]).env("LC_ALL", "C.UTF-8").output().await.ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn supervise<F, Fut>(name: &'static str, alerts: Arc<AlertManager>, cancel: CancellationToken, f: F)
where
    F: Fn(Arc<AlertManager>, CancellationToken) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send,
{
    tokio::spawn(async move {
        let mut failures = 0u32;
        while !cancel.is_cancelled() {
            match f(alerts.clone(), cancel.clone()).await {
                Ok(()) => break,
                Err(e) => {
                    failures += 1;
                    tracing::warn!(watcher = name, "watcher failed ({failures}): {e:#}");
                    if failures == 3 {
                        alerts.hit(Hit {
                            rule: "watcher-failed",
                            key: format!("watcher:{name}"),
                            severity: Severity::Info,
                            title: format!("Watcher {name} keeps failing"),
                            cause: format!("{e:#}"),
                            evidence: String::new(),
                            proposal: "check kagentd logs".into(),
                            threshold: None,
                            cooldown: Duration::from_secs(24 * 3600),
                        });
                    }
                    let backoff = [5, 30, 300][(failures as usize - 1).min(2)];
                    tokio::select! { _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}, _ = cancel.cancelled() => break }
                }
            }
        }
    });
}

async fn journal(alerts: Arc<AlertManager>, cancel: CancellationToken) -> anyhow::Result<()> {
    let mut child = tokio::process::Command::new("journalctl")
        .args(["-f", "-o", "json", "--since", "now", "--no-pager"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(child.stdout.take().expect("piped")).lines();
    loop {
        tokio::select! {
            l = lines.next_line() => {
                let Some(l) = l? else { anyhow::bail!("journalctl ended") };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&l) else { continue };
                if let Some(e) = Entry::from_json(&v) {
                    let quiet = alerts.quiet_now();
                    for h in rules::journal_rules(&e, quiet) {
                        alerts.hit(h);
                    }
                }
            }
            _ = cancel.cancelled() => return Ok(()),
        }
    }
}

async fn periodic(alerts: Arc<AlertManager>, cancel: CancellationToken, priv_ok: bool, data_dir: std::path::PathBuf) -> anyhow::Result<()> {
    let mut tick = 0u64;
    let mut hot: HashMap<String, u32> = HashMap::new();
    let mut seen_failed: HashSet<String> = HashSet::new();
    startup_checks(&alerts, &data_dir).await;
    loop {
        // every 30 s: thermal; every 60 s: failed units; every 5 min: disk; daily: updates, SMART
        if let Some(j) = sh(&["sensors", "-j"]).await {
            for (label, t, high) in rules::temperatures(&j) {
                let c = hot.entry(label.clone()).or_default();
                *c = if t >= high { *c + 1 } else { 0 };
                if *c == 4 {
                    alerts.hit(rules::thermal_hit(&label, t, high));
                }
            }
        }
        if let Some(g) = sh(&["nvidia-smi", "--query-gpu=temperature.gpu", "--format=csv,noheader,nounits"]).await {
            if let Ok(t) = g.trim().parse::<f64>() {
                let c = hot.entry("gpu".into()).or_default();
                *c = if t >= 85.0 { *c + 1 } else { 0 };
                if *c == 4 {
                    alerts.hit(rules::thermal_hit("NVIDIA GPU", t, 85.0));
                }
            }
        }
        if tick % 2 == 0 {
            for scope in [&["systemctl", "--failed", "--output=json"][..], &["systemctl", "--user", "--failed", "--output=json"][..]] {
                let user = scope.len() == 4;
                let failed = rules::failed_units(&sh(scope).await.unwrap_or_default());
                for (template, n, example) in rules::group_failed(&failed) {
                    let key = format!("{}:{template}:{n}", if user { "user" } else { "system" });
                    if !seen_failed.insert(key) {
                        continue;
                    }
                    // Units already failed when KAgent starts are old news: informational.
                    let severity = if tick == 0 { Severity::Info } else { Severity::Review };
                    let what = if n > 1 { format!("{n} failed instances of {template}") } else { format!("Service failed: {template}") };
                    alerts.hit(Hit {
                        rule: "unit-failed",
                        key: format!("unit:{}:{template}", if user { "user" } else { "system" }),
                        severity,
                        title: format!("{what}{}", if user { " (user)" } else { "" }),
                        cause: if template.contains("drkonqi") || template.contains("whoopsie") {
                            "crash reporter runs: applications crashed (see crashes)".into()
                        } else {
                            "a systemd unit is in the failed state".into()
                        },
                        evidence: example.clone(),
                        proposal: {
                            let scope = if user { " --user" } else { "" };
                            if n > 1 {
                                format!("see `systemctl{scope} --failed`; inspect one with `systemctl{scope} status '{template}@*'`; `systemctl{scope} reset-failed` clears them")
                            } else {
                                format!("`systemctl{scope} status {template}` and `journalctl{scope} -u {template} -n 50`")
                            }
                        },
                        threshold: None,
                        cooldown: Duration::from_secs(6 * 3600),
                    });
                }
            }
        }
        if tick % 10 == 0 {
            for (mount, pct) in rules::disk_usage(&sh(&["df", "-P", "--output=target,pcent,fstype"]).await.unwrap_or_default()) {
                if let Some(h) = rules::disk_hit(&mount, pct) {
                    alerts.hit(h);
                }
            }
        }
        if tick % 2880 == 5 {
            daily_checks(&alerts, priv_ok).await;
        }
        tick += 1;
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(30)) => {}, _ = cancel.cancelled() => return Ok(()) }
    }
}

async fn startup_checks(alerts: &AlertManager, data_dir: &std::path::Path) {
    // NVIDIA card present but module not loaded (typical after a kernel update when the
    // DKMS build failed). Also remember the kernel to spot updates.
    let kernel = sh(&["uname", "-r"]).await.unwrap_or_default().trim().to_string();
    let marker = data_dir.join("last-kernel");
    let previous = std::fs::read_to_string(&marker).unwrap_or_default();
    let _ = std::fs::write(&marker, &kernel);
    let lspci = sh(&["lspci"]).await.unwrap_or_default();
    let has_nvidia = lspci.lines().any(|l| (l.contains("VGA") || l.contains("3D")) && l.contains("NVIDIA"));
    if has_nvidia && !std::path::Path::new("/sys/module/nvidia").exists() {
        alerts.hit(Hit {
            rule: "dkms-broken",
            key: format!("dkms:{kernel}"),
            severity: Severity::High,
            title: format!("NVIDIA driver not loaded on kernel {kernel}"),
            cause: if previous.trim() != kernel && !previous.is_empty() { format!("kernel changed from {} - the driver module probably did not build", previous.trim()) } else { "the nvidia module is not loaded".into() },
            evidence: lspci.lines().filter(|l| l.contains("NVIDIA")).collect::<Vec<_>>().join("\n"),
            proposal: "run the after-kernel-update playbook (drivers tool, dkms status)".into(),
            threshold: None,
            cooldown: Duration::from_secs(24 * 3600),
        });
    }
}

async fn daily_checks(alerts: &AlertManager, priv_ok: bool) {
    let ctx = kagent_core::tool::ToolContext::standalone(Duration::from_secs(120), 512_000);
    let up = kagent_core::platform::packages::updates(&ctx, kagent_core::platform::package_manager()).await;
    let sec: Vec<String> = up.security.unwrap_or_default();
    if !sec.is_empty() {
        alerts.hit(Hit {
            rule: "security-updates",
            key: "secupd".into(),
            severity: Severity::Info,
            title: format!("{} security updates waiting", sec.len()),
            cause: "packages with security fixes are not installed yet".into(),
            evidence: sec.iter().take(15).cloned().collect::<Vec<_>>().join("\n"),
            proposal: "install them (package install, or the Software app)".into(),
            threshold: None,
            cooldown: Duration::from_secs(24 * 3600),
        });
    }
    let _ = priv_ok; // SMART needs a root call per disk: done by the agent on demand (disk_health).
}

pub fn start(alerts: Arc<AlertManager>, cancel: CancellationToken, priv_ok: bool, data_dir: std::path::PathBuf) {
    supervise("journal", alerts.clone(), cancel.clone(), journal);
    supervise("periodic", alerts, cancel, move |a, c| periodic(a, c, priv_ok, data_dir.clone()));
}
