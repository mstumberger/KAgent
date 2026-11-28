//! "It worked yesterday": what_changed builds one timeline of everything that changed
//! on the machine in a window (packages, kernel/boots, config files, services, KAgent's
//! own changes), and doctor scores machine health from kernel facts, no model needed.

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Local, NaiveDateTime, TimeZone};
use serde_json::{json, Value};

use super::exec::exec;
use super::read_trim;
use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput};

// ---------------------------------------------------------------- what_changed

pub struct WhatChanged {
    contract: ToolContract,
}

impl WhatChanged {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "what_changed",
                "Timeline of what changed on this machine in the last N hours: package installs/upgrades/removals, kernel and boots, modified files in /etc, failed services, and KAgent's own changes. Use it first when something 'worked yesterday'.",
                json!({"type":"object","properties":{"hours":{"type":"integer","description":"look-back window, default 48, max 720"}}}),
            )
            .timeout(30_000, 60_000),
        }
    }
}

/// dpkg.log lines: `2026-11-23 10:12:01 upgrade htop:amd64 3.2 3.3` -> (time, text).
pub fn parse_dpkg(log: &str, since: DateTime<Local>) -> Vec<(DateTime<Local>, String)> {
    log.lines()
        .filter_map(|l| {
            let mut it = l.splitn(4, ' ');
            let (d, t, action, rest) = (it.next()?, it.next()?, it.next()?, it.next()?);
            if !matches!(action, "install" | "upgrade" | "remove" | "purge") {
                return None;
            }
            let ts = NaiveDateTime::parse_from_str(&format!("{d} {t}"), "%Y-%m-%d %H:%M:%S").ok()?;
            let ts = Local.from_local_datetime(&ts).single()?;
            (ts >= since).then(|| {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let pkg = parts.first().map(|p| p.split(':').next().unwrap_or(p)).unwrap_or("");
                let (from, to) = (parts.get(1).copied().unwrap_or(""), parts.get(2).copied().unwrap_or(""));
                let text = match action {
                    "upgrade" => format!("package upgraded: {pkg} {from} -> {to}"),
                    "install" => format!("package installed: {pkg} {to}"),
                    _ => format!("package removed: {pkg} {from}"),
                };
                (ts, text)
            })
        })
        .collect()
}

#[async_trait]
impl Tool for WhatChanged {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let hours = input.get("hours").and_then(Value::as_i64).unwrap_or(48).clamp(1, 720);
        let since = Local::now() - ChronoDuration::hours(hours);
        let mut events: Vec<(DateTime<Local>, String)> = Vec::new();

        // Packages: dpkg.log, rpm install times or pacman.log (platform backend).
        events.extend(crate::platform::packages::history(&ctx, crate::platform::package_manager(), since).await);
        // Collapse big upgrade runs into one line per minute.
        events = collapse(events);

        // Boots and kernel versions.
        if let Ok(r) = exec(&ctx, &["journalctl", "--list-boots", "--no-pager", "-o", "json"]).run().await {
            if let Ok(Value::Array(boots)) = serde_json::from_str::<Value>(&r.stdout) {
                for b in boots {
                    let us = b.get("first_entry").and_then(Value::as_i64).unwrap_or(0);
                    if let Some(ts) = Local.timestamp_opt(us / 1_000_000, 0).single().filter(|t| *t >= since) {
                        events.push((ts, format!("boot {}", b.get("index").map(|i| i.to_string()).unwrap_or_default())));
                    }
                }
            }
        }
        // Config files touched in /etc (readable listing; contents not needed).
        let mins = (hours * 60).to_string();
        if let Ok(r) = exec(&ctx, &["find", "/etc", "-xdev", "-type", "f", "-mmin", &format!("-{mins}"), "-printf", "%T@ %p\\n"]).run().await {
            let mut files: Vec<(f64, String)> = r.stdout.lines().filter_map(|l| l.split_once(' ').and_then(|(t, p)| Some((t.parse().ok()?, p.to_string())))).collect();
            files.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            for (t, p) in files.into_iter().take(40) {
                if let Some(ts) = Local.timestamp_opt(t as i64, 0).single() {
                    events.push((ts, format!("config file modified: {p}")));
                }
            }
        }
        // KAgent's own changes.
        if let Some(svc) = ctx.services.as_deref() {
            for c in svc.journal.list(100) {
                let ts = c.ts.with_timezone(&Local);
                if ts >= since {
                    events.push((ts, format!("KAgent change ({:?}): {}", c.status, c.summary)));
                }
            }
        }
        events.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = format!("## what changed in the last {hours} h ({} events)\n", events.len());
        for (ts, text) in events.iter().rev().take(150).collect::<Vec<_>>().into_iter().rev() {
            out.push_str(&format!("{}  {text}\n", ts.format("%m-%d %H:%M")));
        }
        out.push_str(&crate::util::more_note(events.len(), 150, "the oldest events are left out; ask for fewer hours"));
        let failed = exec(&ctx, &["systemctl", "--failed", "--no-legend", "--plain", "--no-pager"]).run().await.map(|r| r.stdout).unwrap_or_default();
        if !failed.trim().is_empty() {
            out.push_str("\n## currently failed services\n");
            out.push_str(&failed);
        }
        let kernel = read_trim("/proc/sys/kernel/osrelease").await.unwrap_or_default();
        out.push_str(&format!("\nrunning kernel: {kernel}\n"));
        Ok(ToolOutput::ok(out))
    }
}

fn collapse(mut events: Vec<(DateTime<Local>, String)>) -> Vec<(DateTime<Local>, String)> {
    events.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<(DateTime<Local>, String)> = Vec::new();
    let mut group: Vec<(DateTime<Local>, String)> = Vec::new();
    let flush = |group: &mut Vec<(DateTime<Local>, String)>, out: &mut Vec<(DateTime<Local>, String)>| {
        if group.len() > 6 {
            let names: Vec<String> = group.iter().map(|(_, t)| t.split(": ").nth(1).unwrap_or("").split(' ').next().unwrap_or("").to_string()).collect();
            out.push((group[0].0, format!("{} package changes: {}{}", group.len(), names[..12.min(names.len())].join(", "), if names.len() > 12 { ", ..." } else { "" })));
        } else {
            out.append(group);
        }
        group.clear();
    };
    for e in events {
        if group.last().is_some_and(|l| (e.0 - l.0).num_seconds() > 120) {
            flush(&mut group, &mut out);
        }
        group.push(e);
    }
    flush(&mut group, &mut out);
    out
}

// ---------------------------------------------------------------- doctor

pub struct Doctor {
    contract: ToolContract,
}

impl Doctor {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "doctor",
                "Quick machine health check with a 0-100 score: CPU/memory/IO pressure (PSI), disk space, failed services, errors this boot, temperatures, swap, pending security updates, reboot required, NVIDIA driver. Use it for 'is everything OK?'.",
                json!({"type":"object","properties":{}}),
            )
            .timeout(30_000, 60_000),
        }
    }
}

/// `/proc/pressure/*`: "some avg10=1.23 avg60=0.80 ..." -> avg60 of the "some" line.
pub fn psi_avg60(text: &str) -> Option<f64> {
    text.lines().find(|l| l.starts_with("some"))?.split_whitespace().find_map(|f| f.strip_prefix("avg60=")?.parse().ok())
}

pub async fn checkup(ctx: &ToolContext) -> Value {
    let mut checks: Vec<Value> = Vec::new();
    let mut add = |name: &str, status: &str, detail: String, penalty: u32| {
        checks.push(json!({"name": name, "status": status, "detail": detail, "penalty": penalty}));
    };
    for (res, label) in [("cpu", "CPU pressure"), ("memory", "memory pressure"), ("io", "disk I/O pressure")] {
        match read_trim(format!("/proc/pressure/{res}")).await.as_deref().and_then(psi_avg60) {
            Some(v) if v >= 20.0 => add(label, "bad", format!("{v:.1}% of the last minute stalled"), 15),
            Some(v) if v >= 5.0 => add(label, "warn", format!("{v:.1}% stalled"), 5),
            Some(v) => add(label, "ok", format!("{v:.1}% stalled"), 0),
            None => {}
        }
    }
    let df = exec(ctx, &["df", "-P", "--output=target,pcent,fstype"]).run().await.map(|r| r.stdout).unwrap_or_default();
    for l in df.lines().skip(1) {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 || !["ext4", "btrfs", "xfs", "vfat"].contains(&f[2]) {
            continue;
        }
        let pct: u32 = f[1].trim_end_matches('%').parse().unwrap_or(0);
        let (st, p) = if pct >= 95 { ("bad", 20) } else if pct >= 85 { ("warn", 7) } else { ("ok", 0) };
        add(&format!("disk {}", f[0]), st, format!("{pct}% used"), p);
    }
    let failed = exec(ctx, &["systemctl", "--failed", "--no-legend", "--plain"]).run().await.map(|r| r.stdout.lines().count()).unwrap_or(0);
    add("failed system services", if failed > 0 { "warn" } else { "ok" }, failed.to_string(), if failed > 0 { 8 } else { 0 });
    let errs = exec(ctx, &["journalctl", "-b", "-p", "err", "-q", "--no-pager", "-o", "cat"]).run().await.map(|r| r.stdout.lines().count()).unwrap_or(0);
    add("errors logged this boot", if errs > 200 { "warn" } else { "ok" }, errs.to_string(), if errs > 200 { 5 } else { 0 });
    let snap = super::sys::system_snapshot().await;
    let hot = snap["temperatures"].as_array().map(|a| a.iter().filter_map(|t| t["celsius"].as_f64()).fold(0.0, f64::max)).unwrap_or(0.0);
    if hot > 0.0 {
        let (st, p) = if hot >= 90.0 { ("bad", 15) } else if hot >= 80.0 { ("warn", 5) } else { ("ok", 0) };
        add("hottest sensor", st, format!("{hot:.0} C"), p);
    }
    let swap_total = snap["memoryMiB"]["swapTotal"].as_u64().unwrap_or(0);
    let swap_used = swap_total.saturating_sub(snap["memoryMiB"]["swapFree"].as_u64().unwrap_or(0));
    if swap_total > 0 {
        let pct = swap_used * 100 / swap_total;
        add("swap in use", if pct > 50 { "warn" } else { "ok" }, format!("{pct}% ({swap_used} MiB)"), if pct > 50 { 5 } else { 0 });
    }
    let pm = crate::platform::package_manager();
    if let Some(why) = crate::platform::packages::reboot_required(pm).await {
        add("reboot required", "warn", why, 5);
    }
    let up = crate::platform::packages::updates(ctx, pm).await;
    match up.security {
        Some(sec) => add("security updates pending", if sec.is_empty() { "ok" } else { "warn" }, sec.len().to_string(), if sec.is_empty() { 0 } else { 5 }),
        // No advisories (Arch): many pending updates is the closest signal.
        None => add("updates pending", if up.all.len() > 50 { "warn" } else { "ok" }, up.all.len().to_string(), if up.all.len() > 50 { 3 } else { 0 }),
    }
    let lspci = exec(ctx, &["lspci"]).run().await.map(|r| r.stdout).unwrap_or_default();
    if lspci.lines().any(|l| l.contains("NVIDIA") && (l.contains("VGA") || l.contains("3D"))) {
        let loaded = tokio::fs::metadata("/sys/module/nvidia").await.is_ok();
        add("NVIDIA driver", if loaded { "ok" } else { "bad" }, if loaded { read_trim("/sys/module/nvidia/version").await.unwrap_or_else(|| "loaded".into()) } else { "not loaded".into() }, if loaded { 0 } else { 15 });
    }
    let score = 100u32.saturating_sub(checks.iter().map(|c| c["penalty"].as_u64().unwrap_or(0) as u32).sum());
    json!({"score": score, "checks": checks, "at": Local::now().to_rfc3339()})
}

#[async_trait]
impl Tool for Doctor {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, _input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let r = checkup(&ctx).await;
        let mut out = format!("health score: {}/100\n", r["score"]);
        for c in r["checks"].as_array().into_iter().flatten() {
            out.push_str(&format!("[{}] {}: {}\n", c["status"].as_str().unwrap_or(""), c["name"].as_str().unwrap_or(""), c["detail"].as_str().unwrap_or("")));
        }
        Ok(ToolOutput::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpkg_parsing_and_collapsing() {
        let log = "2026-11-23 10:12:01 startup archives unpack\n2026-11-23 10:12:05 upgrade htop:amd64 3.2.1 3.3.0\n2026-11-23 10:12:06 install jq:amd64 <none> 1.7\n2020-01-01 00:00:00 remove old:amd64 1 <none>\n";
        let since = Local.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        let e = parse_dpkg(log, since);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].1, "package upgraded: htop 3.2.1 -> 3.3.0");
        assert_eq!(e[1].1, "package installed: jq 1.7");
        let t = Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap();
        let many: Vec<_> = (0..10).map(|i| (t + ChronoDuration::seconds(i), format!("package upgraded: p{i} 1 -> 2"))).collect();
        let c = collapse(many);
        assert_eq!(c.len(), 1);
        assert!(c[0].1.starts_with("10 package changes: p0, p1"));
    }

    #[test]
    fn psi_parsing() {
        assert_eq!(psi_avg60("some avg10=1.00 avg60=2.50 avg300=0.10 total=1\nfull avg10=0 avg60=9 avg300=0 total=0"), Some(2.5));
        assert_eq!(psi_avg60(""), None);
    }
}
