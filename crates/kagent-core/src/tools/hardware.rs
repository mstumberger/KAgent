//! Hardware/driver tools (tools.md section 6): hardware, power, audio, bluetooth,
//! crashes, drivers, disk_health (PRIV part stubbed until S8).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::exec::exec;
use super::{parse, read_trim};
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

fn ro_contract(name: &str, description: &str, schema: Value) -> ToolContract {
    ToolContract::read_only(name, description, schema).timeout(30_000, 60_000)
}

async fn run(ctx: &ToolContext, argv: Vec<String>) -> String {
    match exec(ctx, &argv).run().await {
        Ok(r) => r.render(),
        Err(e) => format!("unavailable: {e}\n"),
    }
}

// ---------------------------------------------------------------- hardware

pub struct Hardware {
    contract: ToolContract,
}

impl Hardware {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "hardware",
                "Hardware inventory: PCI devices with drivers (lspci), USB devices, sensor temperatures, GPU status (nvidia-smi).",
                json!({"type":"object","properties":{
                    "section":{"type":"string","enum":["pci","usb","sensors","gpu"]}
                }}),
            ),
        }
    }
}

#[derive(Deserialize, Default)]
struct SectionArgs {
    section: Option<String>,
}

#[async_trait]
impl Tool for Hardware {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: SectionArgs = parse(input)?;
        let want = args.section.as_deref();
        let mut out = String::new();
        if want.is_none() || matches!(want, Some("pci")) {
            out.push_str("## pci\n");
            out.push_str(&run(&ctx, vec!["lspci".into(), "-nnk".into()]).await);
            out.push('\n');
        }
        if want.is_none() || matches!(want, Some("usb")) {
            out.push_str("\n## usb\n");
            out.push_str(&run(&ctx, vec!["lsusb".into()]).await);
            out.push('\n');
        }
        if want.is_none() || matches!(want, Some("sensors")) {
            out.push_str("\n## sensors\n");
            out.push_str(&run(&ctx, vec!["sensors".into(), "-j".into()]).await);
            out.push('\n');
        }
        if want.is_none() || matches!(want, Some("gpu")) {
            out.push_str("\n## gpu\n");
            if super::exec::which("nvidia-smi") {
                out.push_str(&run(
                    &ctx,
                    vec![
                        "nvidia-smi".into(),
                        "--query-gpu=name,driver_version,temperature.gpu,utilization.gpu,memory.used,memory.total,power.draw --format=csv,noheader".into(),
                    ],
                )
                .await);
            } else {
                out.push_str("unavailable: nvidia-smi not installed\n");
            }
        }
        Ok(ToolOutput::ok(out.trim_end().to_string()))
    }
}

// ---------------------------------------------------------------- power

pub struct Power {
    contract: ToolContract,
}

impl Power {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "power",
                "Power state: batteries, AC, active power profile, CPU governors.",
                json!({"type":"object","properties":{}}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Power {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let _ = parse::<SectionArgs>(input)?;
        let mut out = String::new();
        out.push_str("## batteries\n");
        let mut found = false;
        if let Ok(mut rd) = tokio::fs::read_dir("/sys/class/power_supply").await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let p = e.path();
                let kind = read_trim(p.join("type")).await.unwrap_or_default();
                if kind != "Battery" && kind != "Mains" {
                    continue;
                }
                found = true;
                if kind == "Battery" {
                    out.push_str(&format!(
                        "battery {}: {}%, {}\n",
                        p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        read_trim(p.join("capacity")).await.unwrap_or_else(|| "?".into()),
                        read_trim(p.join("status")).await.unwrap_or_else(|| "?".into()),
                    ));
                    if let (Some(en), Some(ef)) = (
                        read_trim(p.join("energy_now")).await.and_then(|v| v.parse::<f64>().ok()),
                        read_trim(p.join("energy_full")).await.and_then(|v| v.parse::<f64>().ok()),
                    ) {
                        out.push_str(&format!("  health: {:.0}% (design full {:.0} Wh)\n", en / ef * 100.0, ef / 1_000_000.0));
                    }
                    if let Some(pn) = read_trim(p.join("power_now")).await.and_then(|v| v.parse::<f64>().ok()) {
                        out.push_str(&format!("  draw: {:.1} W\n", pn / 1_000_000.0));
                    }
                } else {
                    out.push_str(&format!(
                        "AC online: {}\n",
                        read_trim(p.join("online")).await.unwrap_or_else(|| "?".into())
                    ));
                }
            }
        }
        if !found {
            out.push_str("(none)\n");
        }
        out.push_str("\n## profile\n");
        out.push_str(&run(&ctx, vec!["powerprofilesctl".into(), "get".into()]).await);
        out.push_str("\n## cpu governors\n");
        let mut govs = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir("/sys/devices/system/cpu/cpufreq").await {
            while let Ok(Some(e)) = rd.next_entry().await {
                if let Some(g) = read_trim(e.path().join("scaling_governor")).await {
                    if !govs.contains(&g) {
                        govs.push(g);
                    }
                }
            }
        }
        out.push_str(&if govs.is_empty() { "(unavailable)".into() } else { govs.join(", ") });
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- audio

pub struct Audio {
    contract: ToolContract,
}

impl Audio {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "audio",
                "PipeWire audio state: sinks, sources, defaults, volumes (wpctl status) and which applications are using the microphone.",
                json!({"type":"object","properties":{}}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Audio {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let _ = parse::<SectionArgs>(input)?;
        let mut out = String::new();
        out.push_str("## wpctl status\n");
        out.push_str(&run(&ctx, vec!["wpctl".into(), "status".into()]).await);
        out.push_str("\n## microphone users\n");
        if super::exec::which("pw-dump") {
            match exec(&ctx, &["pw-dump".to_string()]).run().await {
                Ok(r) => {
                    let v: Value = serde_json::from_str(&r.stdout).unwrap_or(Value::Array(vec![]));
                    let mut users = Vec::new();
                    if let Value::Array(nodes) = v {
                        for n in nodes {
                            let info = &n["info"];
                            if info.get("props").and_then(|p| p.get("media.class")).and_then(Value::as_str)
                                == Some("Stream/Input/Audio")
                            {
                                let name = info["props"]["application.name"].as_str().unwrap_or("?");
                                let pid = info["props"]["application.process.id"].as_str().unwrap_or("?");
                                users.push(format!("{name} (pid {pid})"));
                            }
                        }
                    }
                    out.push_str(&if users.is_empty() { "(no app is recording)".to_string() } else { users.join("\n") });
                }
                Err(e) => out.push_str(&format!("unavailable: {e}\n")),
            }
        } else {
            out.push_str("unavailable: pw-dump not installed\n");
        }
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- bluetooth

pub struct Bluetooth {
    contract: ToolContract,
}

impl Bluetooth {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "bluetooth",
                "Bluetooth adapters and paired devices (bluetoothctl); connected devices with details.",
                json!({"type":"object","properties":{}}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Bluetooth {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let _ = parse::<SectionArgs>(input)?;
        let mut out = String::new();
        out.push_str("## adapter\n");
        out.push_str(&run(&ctx, vec!["bluetoothctl".into(), "show".into()]).await);
        out.push_str("\n## devices\n");
        let devices = run(&ctx, vec!["bluetoothctl".into(), "devices".into()]).await;
        out.push_str(&devices);
        for line in devices.lines() {
            if let Some(mac) = line.split_whitespace().nth(1) {
                if line.contains("Mesh") {
                    continue;
                }
                let info = run(&ctx, vec!["bluetoothctl".into(), "info".into(), mac.to_string()]).await;
                if info.contains("Connected: yes") {
                    out.push_str(&format!("\n### connected {mac}\n{info}"));
                }
            }
        }
        Ok(ToolOutput::ok(out))
    }
}

// ---------------------------------------------------------------- crashes / drivers / disk_health

pub struct Crashes {
    contract: ToolContract,
}

impl Crashes {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "crashes",
                "Application crashes of the last days (coredumpctl); pass pid for full crash info.",
                json!({"type":"object","properties":{
                    "since":{"type":"string","description":"coredumpctl --since, default -7d"},
                    "pid":{"type":"integer","description":"show details of one coredump"}
                }}),
            ),
        }
    }
}

#[derive(Deserialize, Default)]
struct CrashArgs {
    since: Option<String>,
    pid: Option<i64>,
}

#[async_trait]
impl Tool for Crashes {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: CrashArgs = parse(input)?;
        let since = args.since.filter(|s| !s.is_empty()).unwrap_or_else(|| "-7d".into());
        if since.chars().any(|c| ";|&$`".contains(c)) {
            return Ok(ToolOutput::error("since may not contain ; | & $ or backticks"));
        }
        let argv = if let Some(pid) = args.pid.filter(|p| *p > 0) {
            vec!["coredumpctl".into(), "info".into(), pid.to_string(), "--no-pager".into()]
        } else {
            vec![
                "coredumpctl".into(),
                "list".into(),
                "--json=short".into(),
                format!("--since={since}"),
                "--no-pager".into(),
            ]
        };
        let r = exec(&ctx, &argv).run().await;
        Ok(match r {
            Ok(r) if r.success() => ToolOutput::ok(r.render()),
            Ok(r) => ToolOutput::error(r.render()),
            Err(e) => ToolOutput::error(format!("error: {e:#}")),
        })
    }
}

pub struct Drivers {
    contract: ToolContract,
}

impl Drivers {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "drivers",
                "Driver state: DKMS modules per kernel, Secure Boot, recommended drivers, NVIDIA module version.",
                json!({"type":"object","properties":{}}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Drivers {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let _ = parse::<SectionArgs>(input)?;
        let mut out = String::new();
        out.push_str("## kernel\n");
        out.push_str(&read_trim("/proc/sys/kernel/osrelease").await.unwrap_or_default());
        out.push_str("\n\n## dkms status\n");
        out.push_str(&run(&ctx, vec!["dkms".into(), "status".into()]).await);
        out.push_str("\n## secure boot\n");
        out.push_str(&run(&ctx, vec!["mokutil".into(), "--sb-state".into()]).await);
        if crate::platform::package_manager() == crate::platform::PackageManager::Apt {
            out.push_str("\n## ubuntu-drivers\n");
            out.push_str(&run(&ctx, vec!["ubuntu-drivers".into(), "list".into()]).await);
        }
        out.push_str("\n## nvidia module\n");
        out.push_str(&read_trim("/proc/driver/nvidia/version").await.unwrap_or_else(|| "not loaded".into()));
        Ok(ToolOutput::ok(out))
    }
}

pub struct DiskHealth {
    contract: ToolContract,
}

impl DiskHealth {
    pub fn new() -> Self {
        Self {
            contract: ro_contract(
                "disk_health",
                "SMART disk health through the root helper (asks once); without a device, lists disks.",
                json!({"type":"object","properties":{
                    "device":{"type":"string","description":"e.g. /dev/nvme0n1; omit to list disks"}
                }}),
            )
            .mutating(SideEffect::System, Risk::Low)
            .privileged()
            .timeout(60_000, 90_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct DiskHealthArgs {
    device: Option<String>,
}

#[async_trait]
impl Tool for DiskHealth {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let device = input.get("device").and_then(Value::as_str).unwrap_or("(list)");
        // Listing is read-only; a SMART read goes through the privileged helper.
        let mut a = CallAssessment::from_contract(&self.contract, format!("smartctl {device}"), format!("smartctl {device}"));
        if input.get("device").and_then(Value::as_str).map_or(true, |d| d.is_empty()) {
            a = a.as_read_only();
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: DiskHealthArgs = parse(input)?;
        if args.device.as_deref().map_or(true, |d| d.is_empty()) {
            let r = exec(&ctx, &["lsblk".to_string(), "-dn".into(), "-o".into(), "NAME,TYPE,SIZE,MODEL".into()])
                .run()
                .await;
            let body = match r {
                Ok(r) => r.render(),
                Err(e) => format!("error: {e:#}"),
            };
            return Ok(ToolOutput::ok(format!("## disks\n{body}\n(pass a device such as /dev/nvme0n1 for SMART data)")));
        }
        let device = args.device.unwrap_or_default();
        let resp = super::privop::query_root(&ctx, &json!({"op": "smartctl", "device": device})).await?;
        if !resp.ok {
            return Ok(ToolOutput::error(format!(
                "smartctl failed ({}): {} {}",
                resp.code.unwrap_or_default(),
                resp.message.unwrap_or_default(),
                resp.stderr.trim()
            )));
        }
        let v: Value = serde_json::from_str(&resp.stdout).unwrap_or(Value::Null);
        Ok(ToolOutput::ok(summarize_smart(&device, &v)))
    }
}

/// Condense `smartctl -a -j` (ATA or NVMe) into the facts that matter.
pub fn summarize_smart(device: &str, v: &Value) -> String {
    let s = |p: &str| v.pointer(p).map(|x| x.to_string().trim_matches('"').to_string());
    let mut out = format!("## SMART {device}\n");
    for (label, ptr) in [
        ("model", "/model_name"),
        ("serial", "/serial_number"),
        ("firmware", "/firmware_version"),
        ("health passed", "/smart_status/passed"),
        ("temperature C", "/temperature/current"),
        ("power-on hours", "/power_on_time/hours"),
        ("power cycles", "/power_cycle_count"),
        ("NVMe percentage used", "/nvme_smart_health_information_log/percentage_used"),
        ("NVMe media errors", "/nvme_smart_health_information_log/media_errors"),
        ("NVMe unsafe shutdowns", "/nvme_smart_health_information_log/unsafe_shutdowns"),
        ("NVMe available spare %", "/nvme_smart_health_information_log/available_spare"),
    ] {
        if let Some(val) = s(ptr) {
            out.push_str(&format!("{label}: {val}\n"));
        }
    }
    // ATA attributes worth surfacing when non-zero.
    if let Some(table) = v.pointer("/ata_smart_attributes/table").and_then(Value::as_array) {
        for a in table {
            let name = a.get("name").and_then(Value::as_str).unwrap_or("");
            let raw = a.pointer("/raw/value").and_then(Value::as_u64).unwrap_or(0);
            let watch = ["Reallocated_Sector_Ct", "Current_Pending_Sector", "Offline_Uncorrectable", "Reported_Uncorrect", "Wear_Leveling_Count", "Media_Wearout_Indicator"];
            if watch.contains(&name) {
                out.push_str(&format!("{name}: {raw}\n"));
            }
        }
    }
    if v.is_null() {
        out.push_str("(no JSON from smartctl)\n");
    }
    out
}
