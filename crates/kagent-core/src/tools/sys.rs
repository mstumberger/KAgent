//! Machine overview tools: system_info, process_list, process_signal, disk_info,
//! network_info. Read from /proc and /sys directly where that is cheap and exact.

use std::collections::HashMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::exec::exec;
use super::{parse, read_trim};
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

// ---------------------------------------------------------------- system_info

pub struct SystemInfo {
    contract: ToolContract,
}

impl SystemInfo {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "system_info",
                "Overview of this Linux machine: hostname, distro, kernel, uptime, load, CPU, memory, swap, temperatures, battery. Start here when you need machine context.",
                json!({"type":"object","properties":{}}),
            ),
        }
    }
}

pub async fn system_snapshot() -> Value {
    let meminfo = read_trim("/proc/meminfo").await.unwrap_or_default();
    let mem = |key: &str| -> u64 {
        meminfo
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    let cpuinfo = read_trim("/proc/cpuinfo").await.unwrap_or_default();
    let cpu_model = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let cpus = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    let os_release = read_trim("/etc/os-release").await.unwrap_or_default();
    let distro = os_release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_default();
    let uptime_s: f64 = read_trim("/proc/uptime")
        .await
        .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse().ok()))
        .unwrap_or(0.0);
    let load = read_trim("/proc/loadavg").await.unwrap_or_default();
    let load: Vec<&str> = load.split_whitespace().take(3).collect();

    let mut temps = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir("/sys/class/thermal").await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let p = e.path();
            if !p.file_name().map(|n| n.to_string_lossy().starts_with("thermal_zone")).unwrap_or(false) {
                continue;
            }
            let kind = read_trim(p.join("type")).await.unwrap_or_default();
            if let Some(t) = read_trim(p.join("temp")).await.and_then(|v| v.parse::<f64>().ok()) {
                temps.push(json!({"zone": kind, "celsius": (t / 100.0).round() / 10.0}));
            }
        }
    }
    let mut battery = Value::Null;
    if let Ok(mut rd) = tokio::fs::read_dir("/sys/class/power_supply").await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let p = e.path();
            if read_trim(p.join("type")).await.as_deref() == Some("Battery") {
                battery = json!({
                    "capacityPercent": read_trim(p.join("capacity")).await,
                    "status": read_trim(p.join("status")).await,
                });
            }
        }
    }
    json!({
        "hostname": read_trim("/proc/sys/kernel/hostname").await,
        "distro": distro,
        "kernel": read_trim("/proc/sys/kernel/osrelease").await,
        "arch": std::env::consts::ARCH,
        "user": std::env::var("USER").unwrap_or_default(),
        "uptimeHours": (uptime_s / 360.0).round() / 10.0,
        "loadAvg": load,
        "cpu": {"model": cpu_model, "logicalCores": cpus},
        "memoryMiB": {
            "total": mem("MemTotal:") / 1024,
            "available": mem("MemAvailable:") / 1024,
            "swapTotal": mem("SwapTotal:") / 1024,
            "swapFree": mem("SwapFree:") / 1024,
        },
        "temperatures": temps,
        "battery": battery,
    })
}

#[async_trait]
impl Tool for SystemInfo {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, _input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::json(&system_snapshot().await))
    }
}

// ---------------------------------------------------------------- process_list

pub struct ProcessList {
    contract: ToolContract,
}

impl ProcessList {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "process_list",
                "List running processes with pid, user, CPU% (sampled over 0.5 s), RSS memory and command line. Sort by cpu or mem, optionally filter by a substring of the command.",
                json!({"type":"object","properties":{
                    "sort":{"type":"string","enum":["cpu","mem","pid"],"description":"default cpu"},
                    "filter":{"type":"string","description":"case-insensitive substring of name/command"},
                    "limit":{"type":"integer","description":"default 25, max 200"}
                }}),
            ),
        }
    }
}

#[derive(Deserialize, Default)]
struct ProcArgs {
    sort: Option<String>,
    filter: Option<String>,
    limit: Option<usize>,
}

struct ProcSample {
    pid: u32,
    name: String,
    state: String,
    ticks: u64,
    rss_kib: u64,
    uid: u32,
}

async fn sample_processes() -> HashMap<u32, ProcSample> {
    let mut out = HashMap::new();
    let Ok(mut rd) = tokio::fs::read_dir("/proc").await else { return out };
    while let Ok(Some(e)) = rd.next_entry().await {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Some(stat) = read_trim(format!("/proc/{pid}/stat")).await else { continue };
        // comm may contain spaces/parens: split at the LAST ')'.
        let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
        let name = stat[open + 1..close].to_string();
        let rest: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let field = |i: usize| rest.get(i).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let state = rest.first().unwrap_or(&"?").to_string();
        let ticks = field(11) + field(12); // utime + stime
        let rss_kib = field(21) * 4; // rss pages * 4 KiB
        let uid = read_trim(format!("/proc/{pid}/status"))
            .await
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("Uid:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or(0);
        out.insert(pid, ProcSample { pid, name, state, ticks, rss_kib, uid });
    }
    out
}

async fn users() -> HashMap<u32, String> {
    read_trim("/etc/passwd")
        .await
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            Some((f.get(2)?.parse().ok()?, f.first()?.to_string()))
        })
        .collect()
}

#[async_trait]
impl Tool for ProcessList {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: ProcArgs = parse(input)?;
        let first = sample_processes().await;
        let interval = Duration::from_millis(500);
        tokio::time::sleep(interval).await;
        let second = sample_processes().await;
        let hz = 100.0; // USER_HZ on every mainstream Linux build
        let users = users().await;
        let filter = args.filter.map(|f| f.to_lowercase());
        let mut rows = Vec::new();
        for (pid, s) in &second {
            let cmdline = tokio::fs::read(format!("/proc/{pid}/cmdline"))
                .await
                .map(|b| String::from_utf8_lossy(&b).replace('\0', " ").trim().to_string())
                .unwrap_or_default();
            if let Some(f) = &filter {
                if !s.name.to_lowercase().contains(f) && !cmdline.to_lowercase().contains(f) {
                    continue;
                }
            }
            let prev = first.get(pid).map(|p| p.ticks).unwrap_or(s.ticks);
            let cpu = (s.ticks.saturating_sub(prev)) as f64 / hz / interval.as_secs_f64() * 100.0;
            rows.push((s, cpu, cmdline));
        }
        match args.sort.as_deref().unwrap_or("cpu") {
            "mem" => rows.sort_by(|a, b| b.0.rss_kib.cmp(&a.0.rss_kib)),
            "pid" => rows.sort_by_key(|r| r.0.pid),
            _ => rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap().then(b.0.rss_kib.cmp(&a.0.rss_kib))),
        }
        let limit = args.limit.unwrap_or(25).clamp(1, 200);
        let total = rows.len();
        let mut text = format!("{total} processes matched; showing {}\nPID     USER         CPU%   RSS_MiB  S  COMMAND\n", limit.min(total));
        for (s, cpu, cmd) in rows.into_iter().take(limit) {
            let cmd = if cmd.is_empty() { format!("[{}]", s.name) } else { cmd };
            let user = users.get(&s.uid).cloned().unwrap_or_else(|| s.uid.to_string());
            text.push_str(&format!(
                "{:<7} {:<12} {:>5.1} {:>9.1}  {}  {}\n",
                s.pid,
                crate::util::truncate_str(&user, 12),
                cpu,
                s.rss_kib as f64 / 1024.0,
                s.state,
                crate::util::truncate_str(&cmd, 160)
            ));
        }
        Ok(ToolOutput::ok(text))
    }
}

// ---------------------------------------------------------------- process_signal

pub struct ProcessSignal {
    contract: ToolContract,
}

const SIGNALS: [&str; 7] = ["TERM", "KILL", "HUP", "INT", "STOP", "CONT", "USR1"];

impl ProcessSignal {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "process_signal",
                "Send a signal to a process (default TERM). Use process_list first to confirm the pid. Set sudo=true only for processes owned by another user.",
                json!({"type":"object","required":["pid"],"properties":{
                    "pid":{"type":"integer"},
                    "signal":{"type":"string","enum":SIGNALS},
                    "sudo":{"type":"boolean"}
                }}),
            )
            .mutating(SideEffect::System, Risk::High)
            .destructive(),
        }
    }
}

#[derive(Deserialize, Default)]
struct SignalArgs {
    pid: i64,
    signal: Option<String>,
    sudo: Option<bool>,
}

#[async_trait]
impl Tool for ProcessSignal {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let pid = input.get("pid").and_then(Value::as_i64).unwrap_or(0);
        let sig = input.get("signal").and_then(Value::as_str).unwrap_or("TERM");
        let sudo = input.get("sudo").and_then(Value::as_bool).unwrap_or(false);
        let mut a = CallAssessment::from_contract(&self.contract, format!("kill -{sig} {pid}"), format!("kill -{sig}"));
        // Ending a process can lose unsaved work.
        if matches!(sig, "TERM" | "KILL" | "INT" | "HUP" | "QUIT") {
            a = a.destructive();
        }
        if sudo {
            a.privileged = true;
            a.always_ask = true;
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: SignalArgs = parse(input)?;
        let sig = args.signal.unwrap_or_else(|| "TERM".into());
        if !SIGNALS.contains(&sig.as_str()) {
            anyhow::bail!("unsupported signal {sig}");
        }
        if args.pid <= 1 {
            anyhow::bail!("refusing to signal pid {} (init/kernel)", args.pid);
        }
        let comm = read_trim(format!("/proc/{}/comm", args.pid)).await.unwrap_or_else(|| "?".into());
        if args.sudo.unwrap_or(false) {
            let summary = format!("kill -{sig} {} ({comm}) as root", args.pid);
            return super::privop::apply_root(&ctx, "process_signal", &summary, json!({"op": "kill", "pid": args.pid, "signal": sig})).await;
        }
        let r = exec(&ctx, &["kill".into(), format!("-{sig}"), args.pid.to_string()]).run().await?;
        let out = format!("sent SIG{sig} to {} ({comm})\n{}", args.pid, r.render());
        Ok(if r.success() { ToolOutput::ok(out) } else { ToolOutput::error(out) })
    }
}

// ---------------------------------------------------------------- disk_info / network_info

pub struct CommandReport {
    contract: ToolContract,
    sections: Vec<(&'static str, Vec<&'static str>)>,
}

impl CommandReport {
    pub fn disk() -> Self {
        Self {
            contract: ToolContract::read_only(
                "disk_info",
                "Block devices (lsblk) and mounted filesystem usage (df). Optional `section`: devices | usage.",
                json!({"type":"object","properties":{"section":{"type":"string","enum":["devices","usage"]}}}),
            ),
            sections: vec![
                ("devices", vec!["lsblk", "-o", "NAME,SIZE,TYPE,FSTYPE,MOUNTPOINTS,MODEL"]),
                ("usage", vec!["df", "-hT", "-x", "tmpfs", "-x", "devtmpfs", "-x", "squashfs", "-x", "overlay", "-x", "efivarfs"]),
            ],
        }
    }

    pub fn network() -> Self {
        Self {
            contract: ToolContract::read_only(
                "network_info",
                "Network state: interface addresses, routes, listening sockets (with owning process when visible), DNS. Optional `section`: addresses | routes | listening | dns.",
                json!({"type":"object","properties":{"section":{"type":"string","enum":["addresses","routes","listening","dns"]}}}),
            ),
            sections: vec![
                ("addresses", vec!["ip", "-br", "addr"]),
                ("routes", vec!["ip", "route"]),
                ("listening", vec!["ss", "-tulpnH"]),
                ("dns", vec!["cat", "/etc/resolv.conf"]),
            ],
        }
    }
}

#[async_trait]
impl Tool for CommandReport {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let want = input.get("section").and_then(Value::as_str);
        let mut out = String::new();
        for (name, argv) in &self.sections {
            if want.is_some_and(|w| w != *name) {
                continue;
            }
            let r = exec(&ctx, argv).run().await;
            out.push_str(&format!("## {name}\n"));
            match r {
                Ok(r) => out.push_str(&r.render()),
                Err(e) => out.push_str(&format!("unavailable: {e}\n")),
            }
            out.push('\n');
        }
        Ok(ToolOutput::ok(out))
    }
}
