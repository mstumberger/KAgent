//! Kernel tools: kernel_info, kernel_module, sysctl (tools.md section 2).

use std::path::Path;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::exec::exec;
use super::{parse, read_trim};
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

const TAINT_FLAGS: [(u64, char, &str); 19] = [
    (0, 'P', "proprietary module"),
    (1, 'F', "forced load"),
    (2, 'S', "out-of-spec system (Secure Boot out of spec)"),
    (3, 'R', "forced unload"),
    (4, 'M', "machine check"),
    (5, 'B', "bad page"),
    (6, 'U', "user request"),
    (7, 'D', "kernel died (oops)"),
    (8, 'A', "ACPI table overridden"),
    (9, 'W', "warning"),
    (10, 'C', "staging driver"),
    (11, 'I', "firmware workaround"),
    (12, 'O', "out-of-tree module"),
    (13, 'E', "unsigned module"),
    (14, 'L', "soft lockup"),
    (15, 'K', "live patched"),
    (16, 'X', "auxiliary"),
    (17, 'T', "struct randomization"),
    (18, 'N', "in-kernel test"),
];

pub fn decode_taint(value: u64) -> Vec<(char, String)> {
    TAINT_FLAGS
        .iter()
        .filter(|(bit, _, _)| value & (1 << bit) != 0)
        .map(|(_, letter, meaning)| (*letter, (*meaning).to_string()))
        .collect()
}

// ---------------------------------------------------------------- kernel_info

pub struct KernelInfo {
    contract: ToolContract,
}

impl KernelInfo {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "kernel_info",
                "Kernel facts: version, boot command line, taint flags (decoded), loaded modules (filterable), kernel log (journalctl -k).",
                json!({"type":"object","properties":{
                    "section":{"type":"string","enum":["version","cmdline","taint","modules","log"],"description":"omit for version+cmdline+taint"},
                    "filter":{"type":"string","description":"modules: name substring; log: case-insensitive substring"},
                    "lines":{"type":"integer","description":"log lines, default 100, max 500"}
                }}),
            )
            .timeout(20_000, 60_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct KernelInfoArgs {
    section: Option<String>,
    filter: Option<String>,
    lines: Option<usize>,
}

#[async_trait]
impl Tool for KernelInfo {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: KernelInfoArgs = parse(input)?;
        let want = args.section.as_deref();
        let mut out = String::new();
        let all = want.is_none();
        if all || matches!(want, Some("version")) {
            out.push_str(&format!("## version\n{}\n\n", read_trim("/proc/version").await.unwrap_or_else(|| "unavailable".into())));
        }
        if all || matches!(want, Some("cmdline")) {
            out.push_str(&format!("## cmdline\n{}\n\n", read_trim("/proc/cmdline").await.unwrap_or_else(|| "unavailable".into())));
        }
        if all || matches!(want, Some("taint")) {
            let v: u64 = read_trim("/proc/sys/kernel/tainted").await.and_then(|s| s.parse().ok()).unwrap_or(0);
            out.push_str(&format!("## taint\n{v}\n"));
            let flags = decode_taint(v);
            if flags.is_empty() {
                out.push_str("not tainted\n\n");
            } else {
                for (letter, meaning) in flags {
                    out.push_str(&format!("{letter}: {meaning}\n"));
                }
                out.push('\n');
            }
        }
        if matches!(want, Some("modules")) {
            out.push_str(&self.modules(&args.filter).await);
        }
        if matches!(want, Some("log")) {
            let lines = args.lines.unwrap_or(100).clamp(1, 500);
            let mut argv = vec![
                "journalctl".to_string(),
                "-k".into(),
                "-n".into(),
                lines.to_string(),
                "--no-pager".into(),
                "-o".into(),
                "short-iso".into(),
            ];
            if let Some(f) = args.filter.as_deref().filter(|f| !f.is_empty()) {
                argv.push("--grep".into());
                argv.push(f.to_string());
            }
            let r = exec(&ctx, &argv).run().await;
            out.push_str("## log\n");
            out.push_str(&match r {
                Ok(r) => r.render(),
                Err(e) => format!("unavailable: {e}\n"),
            });
        }
        Ok(ToolOutput::ok(out.trim_end().to_string()))
    }
}

impl KernelInfo {
    async fn modules(&self, filter: &Option<String>) -> String {
        let filter = filter.as_deref().map(str::to_lowercase);
        let raw = read_trim("/proc/modules").await.unwrap_or_default();
        let mut rows: Vec<String> = raw
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                if f.len() < 4 {
                    return None;
                }
                if let Some(fl) = &filter {
                    if !f[0].to_lowercase().contains(fl.as_str()) {
                        return None;
                    }
                }
                Some(format!("{:<28} {:>10} {:>8} {} {}", f[0], f[1], f[2], f[3], f.get(4).unwrap_or(&"")))
            })
            .collect();
        rows.sort();
        let mut out = format!("## modules ({} loaded)\n", rows.len());
        for r in rows.iter().take(500) {
            out.push_str(r);
            out.push('\n');
        }
        out.push_str(&crate::util::more_note(rows.len(), 500, "pass `filter` with part of a module name"));
        out
    }
}

// ---------------------------------------------------------------- kernel_module

pub struct KernelModule {
    contract: ToolContract,
}

impl KernelModule {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "kernel_module",
                "Inspect a kernel module (modinfo, loaded state, current parameters), or load/unload it and set persistent parameters (root helper; load/unload/set_params always ask).",
                json!({"type":"object","required":["action","module"],"properties":{
                    "action":{"type":"string","enum":["info","load","unload","set_params"]},
                    "module":{"type":"string","description":"module name, e.g. btusb"},
                    "params":{"type":"object","description":"load/set_params: key -> value strings","additionalProperties":{"type":"string"}},
                    "persist":{"type":"boolean","description":"set_params: write /etc/modprobe.d/kagent-<module>.conf"}
                }}),
            )
            .mutating(SideEffect::Kernel, Risk::High)
            .privileged()
            .timeout(30_000, 60_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct ModuleArgs {
    action: Option<String>,
    module: Option<String>,
    params: Option<std::collections::BTreeMap<String, String>>,
    persist: Option<bool>,
}

fn module_loaded_sync(module: &str) -> bool {
    // /proc/modules and /sys/module use underscores even when the name has dashes.
    std::path::Path::new(&format!("/sys/module/{}", module.replace('-', "_"))).exists()
}

#[async_trait]
impl Tool for KernelModule {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let module = input.get("module").and_then(Value::as_str).unwrap_or("?");
        let action = input.get("action").and_then(Value::as_str).unwrap_or("info");
        let mut a = CallAssessment::from_contract(
            &self.contract,
            format!("{action} module {module}"),
            format!("{action} {module}"),
        );
        if action == "info" {
            a = a.as_read_only();
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: ModuleArgs = parse(input)?;
        let module = args.module.clone().unwrap_or_default();
        if !valid_module_name(&module) {
            return Ok(ToolOutput::error(format!("invalid module name '{module}'")));
        }
        match args.action.as_deref() {
            Some("info") => {
                let r = exec(&ctx, &["modinfo".to_string(), module.clone()]).run().await;
                let loaded = read_trim("/proc/modules")
                    .await
                    .unwrap_or_default()
                    .lines()
                    .any(|l| l.starts_with(&format!("{} ", module.replace('-', "_"))));
                let mut out = format!("## modinfo {module}\n");
                out.push_str(&match r {
                    Ok(r) => r.render(),
                    Err(e) => format!("unavailable: {e}\n"),
                });
                out.push_str(&format!("\n## state\nloaded: {loaded}\n"));
                if loaded {
                    out.push_str("## parameters\n");
                    let dir = format!("/sys/module/{}/parameters", module.replace('-', "_"));
                    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
                        while let Ok(Some(e)) = rd.next_entry().await {
                            if let Some(name) = e.file_name().to_str() {
                                let v = read_trim(Path::new(&dir).join(name)).await.unwrap_or_default();
                                out.push_str(&format!("{name} = {v}\n"));
                            }
                        }
                    }
                }
                Ok(ToolOutput::ok(out))
            }
            Some("load") | Some("unload") => {
                let remove = args.action.as_deref() == Some("unload");
                let params = if remove { Default::default() } else { args.params.unwrap_or_default() };
                let summary = if remove { format!("modprobe -r {module}") } else { format!("modprobe {module}") };
                super::privop::apply_root(&ctx, "kernel_module", &summary, json!({"op": "modprobe", "module": module, "remove": remove, "params": params})).await
            }
            Some("set_params") => {
                let params = args.params.unwrap_or_default();
                let summary = format!("set {module} parameters {} (/etc/modprobe.d/kagent-{module}.conf)", json!(params));
                let mut out = super::privop::apply_root(&ctx, "kernel_module", &summary, json!({"op": "modprobe_conf", "module": module, "params": params})).await?;
                if args.persist == Some(false) {
                    out.content.push_str("note: parameters are always persisted through /etc/modprobe.d; they apply the next time the module loads.\n");
                } else if module_loaded_sync(&module) {
                    out.content.push_str("note: the module is loaded; new parameters apply after `kernel_module unload` + `load` or a reboot.\n");
                }
                Ok(out)
            }
            Some(other) => Ok(ToolOutput::error(format!("unknown action {other}"))),
            None => Ok(ToolOutput::error("action is required")),
        }
    }

    async fn preview(&self, input: &Value, _ctx: &ToolContext) -> Option<String> {
        let module = input.get("module").and_then(Value::as_str)?;
        let state = if module_loaded_sync(module) { "loaded" } else { "not loaded" };
        let users = read_trim("/proc/modules").await.unwrap_or_default().lines()
            .find(|l| l.starts_with(&format!("{} ", module.replace('-', "_"))))
            .and_then(|l| l.split_whitespace().nth(3).map(|u| u.trim_end_matches(',').to_string()))
            .filter(|u| u != "-")
            .map(|u| format!("\nused by: {u}"))
            .unwrap_or_default();
        Some(format!("{module}: currently {state}{users}"))
    }
}

pub fn valid_module_name(m: &str) -> bool {
    !m.is_empty() && m.len() <= 64 && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------- sysctl

pub struct Sysctl {
    contract: ToolContract,
}

impl Sysctl {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "sysctl",
                "Read kernel parameters (/proc/sys): get one key, list keys under a prefix, or set one through the root helper (set always asks; persisted in /etc/sysctl.d/99-kagent.conf by default; undoable).",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["get","list","set"]},
                    "key":{"type":"string","description":"e.g. vm.swappiness; list: prefix such as net.ipv4"},
                    "value":{"type":"string","description":"set: new value"},
                    "persist":{"type":"boolean","description":"set: also write /etc/sysctl.d/99-kagent.conf (default true)"}
                }}),
            )
            .mutating(SideEffect::Kernel, Risk::High)
            .privileged()
            .timeout(10_000, 30_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct SysctlArgs {
    action: Option<String>,
    key: Option<String>,
    value: Option<String>,
    persist: Option<bool>,
}

pub fn valid_sysctl_key(key: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"^[a-z0-9_]+(\.[a-z0-9_-]+)+$").unwrap()).is_match(key)
}

#[async_trait]
impl Tool for Sysctl {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("get");
        let key = input.get("key").and_then(Value::as_str).unwrap_or("?");
        let summary = match (action, input.get("value").and_then(Value::as_str)) {
            ("set", Some(v)) => format!("sysctl {key}={v}"),
            _ => format!("sysctl {action} {key}"),
        };
        let mut a = CallAssessment::from_contract(&self.contract, summary, format!("sysctl {action} {key}"));
        if action != "set" {
            a = a.as_read_only();
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: SysctlArgs = parse(input)?;
        match args.action.as_deref() {
            Some("get") => {
                let Some(key) = args.key.as_deref().filter(|k| valid_sysctl_key(k)) else {
                    return Ok(ToolOutput::error("get requires a valid key, e.g. vm.swappiness"));
                };
                let path = format!("/proc/sys/{}", key.replace('.', "/"));
                match read_trim(&path).await {
                    Some(v) => Ok(ToolOutput::ok(format!("{key} = {v}"))),
                    None => Ok(ToolOutput::error(format!("key {key} not found or unreadable"))),
                }
            }
            Some("list") => {
                let prefix = args.key.clone().unwrap_or_default();
                let root = if prefix.is_empty() {
                    "/proc/sys".to_string()
                } else {
                    format!("/proc/sys/{}", prefix.replace('.', "/"))
                };
                let mut rows = Vec::new();
                collect_keys(std::path::Path::new(&root), &prefix, &mut rows, 300).await;
                if rows.is_empty() {
                    return Ok(ToolOutput::error(format!("no keys found under {prefix}")));
                }
                let mut out = String::new();
                for (k, v) in rows {
                    out.push_str(&format!("{k} = {v}\n"));
                }
                Ok(ToolOutput::ok(out))
            }
            Some("set") => {
                let (Some(key), Some(value)) = (args.key.filter(|k| valid_sysctl_key(k)), args.value) else {
                    return Ok(ToolOutput::error("set requires a valid key and a value"));
                };
                let persist = args.persist.unwrap_or(true);
                let summary = format!("sysctl {key}={value}{}", if persist { " (persist)" } else { "" });
                super::privop::apply_root(&ctx, "sysctl", &summary, json!({"op": "sysctl_set", "key": key, "value": value, "persist": persist})).await
            }
            _ => Ok(ToolOutput::error("action must be get, list or set")),
        }
    }

    async fn preview(&self, input: &Value, _ctx: &ToolContext) -> Option<String> {
        let key = input.get("key").and_then(Value::as_str).filter(|k| valid_sysctl_key(k))?;
        let new = input.get("value").and_then(Value::as_str).unwrap_or("?");
        let old = read_trim(format!("/proc/sys/{}", key.replace('.', "/"))).await.unwrap_or_else(|| "(unknown key)".into());
        let persist = input.get("persist").and_then(Value::as_bool).unwrap_or(true);
        Some(format!("{key}: {old} -> {new}{}", if persist { "\npersisted in /etc/sysctl.d/99-kagent.conf" } else { "\nruntime only (lost on reboot)" }))
    }
}

async fn collect_keys(root: &Path, prefix: &str, out: &mut Vec<(String, String)>, max: usize) {
    // Iterative walk: /proc/sys nests only a few levels, and a worklist avoids
    // boxing a recursive future.
    let mut queue: Vec<(std::path::PathBuf, String)> = vec![(root.to_path_buf(), prefix.to_string())];
    while let Some((dir, prefix)) = queue.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else { continue };
        let mut names: Vec<(std::ffi::OsString, std::path::PathBuf)> = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            names.push((entry.file_name(), entry.path()));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, path) in names {
            if out.len() >= max {
                return;
            }
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if path.is_dir() {
                let sub = if prefix.is_empty() { name.clone() } else { format!("{prefix}.{name}") };
                queue.push((path, sub));
            } else if path.is_file() {
                let key = if prefix.is_empty() { name.clone() } else { format!("{prefix}.{name}") };
                if let Some(v) = read_trim(&path).await {
                    out.push((key, v));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taint_decode() {
        assert!(decode_taint(0).is_empty());
        let flags = decode_taint((1 << 12) | 1);
        assert_eq!(flags[0].0, 'P');
        assert_eq!(flags[1].0, 'O');
        assert!(flags[1].1.contains("out-of-tree"));
    }

    #[test]
    fn sysctl_key_validation() {
        assert!(valid_sysctl_key("vm.swappiness"));
        assert!(valid_sysctl_key("net.ipv4.tcp_rmem"));
        assert!(!valid_sysctl_key("../etc/passwd"));
        assert!(!valid_sysctl_key("vm"));
        assert!(!valid_sysctl_key("vm swappiness"));
    }
}
