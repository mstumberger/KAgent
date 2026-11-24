//! Typed root operations: parse, validate, compute undo, execute (via `System`).
//! Spec: docs/specs/priv-ops.md. Every rejection is a `Fail` with a stable code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::system::{RunOutput, System};

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum Op {
    SysctlSet { key: String, value: String, #[serde(default = "yes")] persist: bool },
    Modprobe { module: String, #[serde(default)] remove: bool, #[serde(default)] params: BTreeMap<String, String> },
    ModprobeConf { module: String, #[serde(default)] params: BTreeMap<String, String> },
    Systemctl { action: String, unit: String },
    /// apt, dnf or pacman (detected by the helper). `apt` is the pre-2026-11-24 name,
    /// still accepted so old undo records work.
    #[serde(alias = "apt")]
    Package { action: String, packages: Vec<String>, #[serde(default)] simulate: bool },
    WriteFile { path: String, #[serde(default)] content: String, #[serde(default = "mode644")] mode: String, #[serde(default)] delete: bool },
    ReadRoot { path: String, #[serde(default)] tail_lines: Option<usize> },
    Kill { pid: i64, signal: String },
    Smartctl { device: String },
    #[serde(alias = "ufw_status")]
    FirewallStatus {},
    Bpftrace { script: String, seconds: u64, #[serde(default)] path_prefix: Option<String> },
    ShellRoot { command: String },
}

fn yes() -> bool {
    true
}
fn mode644() -> String {
    "0644".into()
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    pub op: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub undo: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Debug)]
pub(crate) struct Fail {
    pub code: &'static str,
    pub message: String,
}

pub(crate) fn fail<T>(code: &'static str, message: impl Into<String>) -> Result<T, Fail> {
    Err(Fail { code, message: message.into() })
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        pub(crate) fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).unwrap())
        }
    };
}
re!(re_sysctl_key, r"^[a-z0-9_]+(\.[a-z0-9_-]+){1,6}$");
re!(re_sysctl_val, r"^[A-Za-z0-9 _.,:/-]{1,256}$");
re!(re_sysctl_deny, r"^(kernel\.(modules_disabled|kexec_load_disabled|core_pattern)|kernel\.yama\..*)$");
re!(re_module, r"^[A-Za-z0-9_-]{1,64}$");
re!(re_param_key, r"^[A-Za-z0-9_]{1,64}$");
re!(re_param_val, r"^[A-Za-z0-9_.,:/=-]{0,128}$");
re!(re_unit, r"^[A-Za-z0-9@_.:-]{1,128}$");
re!(re_unit_deny, r"^(dbus|systemd-.*|polkit|ssh|sshd|kagent.*)\.service$");
re!(re_package, r"^[A-Za-z0-9][A-Za-z0-9+._@-]{0,99}$");
re!(re_mode, r"^0[0-7]{3}$");
re!(re_device, r"^/dev/(sd[a-z]{1,2}|nvme[0-9]+n[0-9]+|hd[a-z])$");
re!(re_prefix, r"^/[A-Za-z0-9._/-]{1,200}$");
re!(
    re_write_allow,
    r"^(/etc/sysctl\.d/99-kagent\.conf|/etc/modprobe\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/systemd/system/[A-Za-z0-9@_.:-]{1,128}\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/systemd/journald\.conf\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/NetworkManager/conf\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/udev/rules\.d/90-kagent-[A-Za-z0-9_-]{1,64}\.rules)$"
);
re!(
    re_read_allow,
    r"^(/var/log/(auth|kern|ufw|dpkg|pacman|dnf|dnf5|dnf\.rpm|firewalld)\.log|/var/log/(syslog|secure|messages|firewalld)|/var/log/apt/history\.log|/etc/sudoers|/etc/sudoers\.d/[A-Za-z0-9_.-]{1,64})$"
);

pub const SIGNALS: [&str; 7] = ["TERM", "KILL", "HUP", "INT", "STOP", "CONT", "USR1"];
const SYSCTL_PERSIST: &str = "/etc/sysctl.d/99-kagent.conf";
const BACKUP_DIR: &str = "/var/lib/kagent/backup";
const ROOT_SHELL_FLAG: &str = "/etc/kagent/allow-root-shell";

/// Lexical check: absolute, no `..`, `.` or empty components.
fn clean_abs(path: &str) -> bool {
    path.starts_with('/') && !path.split('/').skip(1).any(|c| c.is_empty() || c == "." || c == "..")
}

/// A path may be used only if it matches the allowlist and nothing along the way is a
/// symlink that would redirect it (parent must canonicalize to itself; target itself
/// must not be a symlink).
fn check_path(sys: &dyn System, path: &str, allow: &Regex) -> Result<PathBuf, Fail> {
    if !clean_abs(path) || !allow.is_match(path) {
        return fail("not_allowed", format!("path not in allowlist: {path}"));
    }
    let p = PathBuf::from(path);
    if let Some(parent) = p.parent() {
        if let Some(c) = sys.canonicalize(parent) {
            if c != parent {
                return fail("not_allowed", format!("symlink in path: {path}"));
            }
        }
    }
    if sys.is_symlink(&p) {
        return fail("not_allowed", format!("target is a symlink: {path}"));
    }
    Ok(p)
}

fn params_ok(params: &BTreeMap<String, String>) -> Result<(), Fail> {
    for (k, v) in params {
        if !re_param_key().is_match(k) || !re_param_val().is_match(v) {
            return fail("invalid_input", format!("bad module parameter {k}={v}"));
        }
    }
    Ok(())
}

pub(crate) fn output(resp: &mut Response, out: RunOutput) {
    resp.ok = out.ok();
    resp.exit = out.exit;
    resp.stdout = out.stdout;
    resp.stderr = out.stderr;
    if out.timed_out {
        resp.ok = false;
        resp.code = Some("timeout".into());
    } else if !resp.ok {
        resp.code = Some("failed".into());
    }
}

pub(crate) fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Parse and run one request. Never panics on bad input.
pub fn handle(raw: &str, sys: &mut dyn System) -> Response {
    let mut v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return reject("?", "invalid_input", format!("not JSON: {e}")),
    };
    let dry_run = v.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    if let Some(o) = v.as_object_mut() {
        o.remove("dryRun");
    }
    let op_name = v.get("op").and_then(Value::as_str).unwrap_or("?").to_string();
    let op: Op = match serde_json::from_value(v) {
        Ok(op) => op,
        Err(e) => return reject(&op_name, "invalid_input", e.to_string()),
    };
    let mut resp = Response { op: op_name.clone(), dry_run, ..Default::default() };
    match run(op, sys, dry_run, &mut resp) {
        Ok(()) => resp,
        Err(f) => reject(&op_name, f.code, f.message),
    }
}

fn reject(op: &str, code: &str, message: String) -> Response {
    Response { ok: false, op: op.into(), code: Some(code.into()), message: Some(message), ..Default::default() }
}

fn run(op: Op, sys: &mut dyn System, dry: bool, r: &mut Response) -> Result<(), Fail> {
    match op {
        Op::SysctlSet { key, value, persist } => {
            if !re_sysctl_key().is_match(&key) || !re_sysctl_val().is_match(&value) {
                return fail("invalid_input", "bad sysctl key or value");
            }
            if re_sysctl_deny().is_match(&key) {
                return fail("not_allowed", format!("{key} is on the deny list"));
            }
            let proc_path = PathBuf::from(format!("/proc/sys/{}", key.replace('.', "/")));
            let Some(old) = sys.read(&proc_path) else { return fail("invalid_input", format!("unknown sysctl {key}")) };
            let old = old.trim().to_string();
            r.before = Some(old.clone());
            r.undo = Some(json!({"op": "sysctl_set", "key": key, "value": old, "persist": persist}));
            if dry {
                r.ok = true;
                return Ok(());
            }
            output(r, sys.run(&argv(&["/usr/sbin/sysctl", "-w", &format!("{key}={value}")]), Duration::from_secs(10)));
            if r.ok && persist {
                let path = Path::new(SYSCTL_PERSIST);
                let prev = sys.read(path).unwrap_or_default();
                let line = format!("{key} = {value}");
                let mut found = false;
                let mut lines: Vec<String> = prev
                    .lines()
                    .map(|l| {
                        let k = l.split('=').next().unwrap_or("").trim();
                        if k == key {
                            found = true;
                            line.clone()
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                if !found {
                    if lines.is_empty() {
                        lines.push("# Managed by KAgent (kagent-priv). Each line was approved by the owner.".into());
                    }
                    lines.push(line);
                }
                if let Err(e) = sys.write(path, &(lines.join("\n") + "\n"), 0o644) {
                    return fail("failed", format!("value set but persisting failed: {e}"));
                }
            }
            r.after = sys.read(&proc_path).map(|s| s.trim().to_string());
        }
        Op::Modprobe { module, remove, params } => {
            if !re_module().is_match(&module) {
                return fail("invalid_input", "bad module name");
            }
            params_ok(&params)?;
            if remove && !params.is_empty() {
                return fail("invalid_input", "params make no sense with remove");
            }
            let loaded = sys.exists(Path::new(&format!("/sys/module/{}", module.replace('-', "_"))));
            r.before = Some(if loaded { "loaded" } else { "not loaded" }.into());
            if loaded != remove {
                // State already as requested: nothing to undo.
            } else {
                r.undo = Some(json!({"op": "modprobe", "module": module, "remove": !remove}));
            }
            if dry {
                r.ok = true;
                return Ok(());
            }
            let mut a = vec!["/usr/sbin/modprobe".to_string()];
            if remove {
                a.push("-r".into());
            }
            a.push(module.clone());
            a.extend(params.iter().map(|(k, v)| format!("{k}={v}")));
            output(r, sys.run(&a, Duration::from_secs(30)));
        }
        Op::ModprobeConf { module, params } => {
            if !re_module().is_match(&module) {
                return fail("invalid_input", "bad module name");
            }
            params_ok(&params)?;
            let path = format!("/etc/modprobe.d/kagent-{module}.conf");
            let p = check_path(sys, &path, re_write_allow())?;
            let prev = sys.read(&p);
            let prev_params: BTreeMap<String, String> = prev
                .as_deref()
                .and_then(|c| c.lines().find(|l| l.starts_with("options ")))
                .map(|l| {
                    l.split_whitespace()
                        .skip(2)
                        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            r.before = prev.clone();
            r.undo = Some(json!({"op": "modprobe_conf", "module": module, "params": prev_params}));
            r.ok = true;
            if dry {
                return Ok(());
            }
            let res = if params.is_empty() {
                sys.remove(&p)
            } else {
                let opts: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
                sys.write(&p, &format!("# Managed by KAgent\noptions {module} {}\n", opts.join(" ")), 0o644)
            };
            if let Err(e) = res {
                return fail("failed", e.to_string());
            }
        }
        Op::Systemctl { action, unit } => {
            if !["start", "stop", "restart", "reload", "enable", "disable"].contains(&action.as_str()) {
                return fail("invalid_input", format!("bad action {action}"));
            }
            if !re_unit().is_match(&unit) {
                return fail("invalid_input", "bad unit name");
            }
            let full = if unit.contains('.') { unit.clone() } else { format!("{unit}.service") };
            if (action == "stop" || action == "disable") && re_unit_deny().is_match(&full) {
                return fail("not_allowed", format!("refusing to {action} {full}: the system or KAgent depends on it"));
            }
            let q = |sys: &mut dyn System, what: &str| {
                sys.run(&argv(&["/usr/bin/systemctl", what, &full]), Duration::from_secs(10)).stdout.trim().to_string()
            };
            let active = q(sys, "is-active");
            let enabled = q(sys, "is-enabled");
            r.before = Some(format!("{active}/{enabled}"));
            let undo_action = match action.as_str() {
                "start" if active != "active" => Some("stop"),
                "stop" if active == "active" => Some("start"),
                "enable" if enabled != "enabled" => Some("disable"),
                "disable" if enabled == "enabled" => Some("enable"),
                _ => None,
            };
            r.undo = undo_action.map(|a| json!({"op": "systemctl", "action": a, "unit": full}));
            if dry {
                r.ok = true;
                return Ok(());
            }
            output(r, sys.run(&argv(&["/usr/bin/systemctl", &action, &full]), Duration::from_secs(60)));
            r.after = Some(format!("{}/{}", q(sys, "is-active"), q(sys, "is-enabled")));
        }
        Op::Package { action, packages, simulate } => crate::pkg::run(sys, &action, &packages, simulate || dry, r)?,
        Op::WriteFile { path, content, mode, delete } => {
            if !re_mode().is_match(&mode) || content.len() > 256 * 1024 {
                return fail("invalid_input", "bad mode or content over 256 KiB");
            }
            let p = check_path(sys, &path, re_write_allow())?;
            let prev = sys.read(&p);
            r.before = prev.clone();
            r.undo = Some(match &prev {
                Some(c) => json!({"op": "write_file", "path": path, "content": c, "mode": mode}),
                None => json!({"op": "write_file", "path": path, "delete": true}),
            });
            r.ok = true;
            if dry {
                return Ok(());
            }
            if let Some(c) = &prev {
                let name = path.trim_start_matches('/').replace('/', "_");
                let bp = PathBuf::from(format!("{BACKUP_DIR}/{}-{name}", sys.now_stamp()));
                if sys.write(&bp, c, 0o600).is_ok() {
                    r.backup = Some(bp.display().to_string());
                }
            }
            let m = u32::from_str_radix(&mode[1..], 8).unwrap_or(0o644);
            let res = if delete { sys.remove(&p) } else { sys.write(&p, &content, m) };
            if let Err(e) = res {
                return fail("failed", e.to_string());
            }
        }
        Op::ReadRoot { path, tail_lines } => {
            let allowed = re_read_allow().is_match(&path) || re_write_allow().is_match(&path);
            if !allowed {
                return fail("not_allowed", format!("path not readable through the helper: {path}"));
            }
            let p = check_path(sys, &path, if re_read_allow().is_match(&path) { re_read_allow() } else { re_write_allow() })?;
            let Some(c) = sys.read(&p) else { return fail("failed", format!("cannot read {path}")) };
            let n = tail_lines.unwrap_or(200).min(2000);
            let lines: Vec<&str> = c.lines().collect();
            r.stdout = lines[lines.len().saturating_sub(n)..].join("\n");
            r.ok = true;
        }
        Op::Kill { pid, signal } => {
            if pid <= 1 || pid > i32::MAX as i64 || !SIGNALS.contains(&signal.as_str()) {
                return fail("invalid_input", "pid must be > 1 and signal one of TERM KILL HUP INT STOP CONT USR1");
            }
            let comm = sys.read(Path::new(&format!("/proc/{pid}/comm"))).unwrap_or_default();
            let cmdline = sys.read(Path::new(&format!("/proc/{pid}/cmdline"))).unwrap_or_default();
            if comm.is_empty() {
                return fail("invalid_input", format!("no process {pid}"));
            }
            if cmdline.is_empty() || comm.trim() == "systemd" {
                return fail("not_allowed", format!("refusing to signal {} ({pid})", comm.trim()));
            }
            r.before = Some(comm.trim().to_string());
            r.ok = true;
            if !dry {
                if let Err(e) = sys.kill(pid as i32, &signal) {
                    return fail("failed", e);
                }
            }
        }
        Op::Smartctl { device } => {
            if !re_device().is_match(&device) {
                return fail("invalid_input", "bad device");
            }
            let out = sys.run(&argv(&["/usr/sbin/smartctl", "-a", "-j", &device]), Duration::from_secs(30));
            // smartctl's exit code is a bitmask; bits 0-1 mean it could not read at all.
            let usable = out.exit.is_some_and(|c| c & 3 == 0) && !out.timed_out;
            output(r, out);
            r.ok = usable;
            if usable {
                r.code = None;
            }
        }
        Op::FirewallStatus {} => crate::pkg::firewall_status(sys, r),
        Op::Bpftrace { script, seconds, path_prefix } => {
            if !(1..=10).contains(&seconds) {
                return fail("invalid_input", "seconds must be 1-10");
            }
            if let Some(p) = &path_prefix {
                if !re_prefix().is_match(p) || !clean_abs(p.trim_end_matches('/')) {
                    return fail("invalid_input", "bad path prefix");
                }
            }
            let Some(body) = crate::bpf::render(&script, seconds, path_prefix.as_deref()) else {
                return fail("invalid_input", format!("unknown script id {script} (or missing pathPrefix)"));
            };
            output(r, sys.run(&["/usr/bin/bpftrace".into(), "-e".into(), body], Duration::from_secs(seconds + 15)));
        }
        Op::ShellRoot { command } => {
            if !sys.exists(Path::new(ROOT_SHELL_FLAG)) {
                return fail("not_allowed", format!("root shell disabled (owner creates {ROOT_SHELL_FLAG} to enable)"));
            }
            if command.is_empty() || command.len() > 4096 {
                return fail("invalid_input", "command must be 1-4096 bytes");
            }
            output(r, sys.run(&argv(&["/bin/bash", "-c", &command]), Duration::from_secs(120)));
        }
    }
    Ok(())
}
