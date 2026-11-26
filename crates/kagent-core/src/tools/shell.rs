//! The shell tool (tools.md section 5): read-only detection over an allowlist,
//! everything else is mutating and asks for approval.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::background::Background;
use super::exec::exec;
use super::parse;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

/// Programs whose every invocation is read-only.
const SIMPLE_RO: &[&str] = &[
    "ls", "cat", "head", "tail", "grep", "egrep", "rg", "wc", "stat", "file", "df", "du", "free",
    "uptime", "uname", "ps", "pgrep", "lsblk", "lscpu", "lspci", "lsusb", "lsmod", "ip", "ss",
    "hostnamectl", "timedatectl", "localectl", "dmesg", "journalctl", "sensors", "nvidia-smi",
    "which", "id", "whoami", "date", "env", "printenv", "sort", "uniq", "cut", "awk", "apt-cache",
];

/// Programs read-only only with a constrained subcommand: allowed first-argument
/// prefixes. `None` entries use special checks in `constrained_ok`.
const CONSTRAINED: &[(&str, &[&str])] = &[
    ("systemctl", &["status", "show", "cat", "list-", "is-"]),
    ("apt", &["list", "show", "policy"]),
    ("dpkg", &["-l", "-L", "-S", "-s"]),
    ("rpm", &["-q"]),
    ("dnf", &["list", "info", "search", "repoquery", "check-update", "history list", "history info", "updateinfo"]),
    ("pacman", &["-Q", "-Si", "-Ss"]),
    ("snap", &["list", "info"]),
    ("flatpak", &["list", "info"]),
    ("docker", &["ps", "images", "inspect", "logs"]),
    ("podman", &["ps", "images", "inspect", "logs"]),
    ("wpctl", &["status", "inspect"]),
    ("nmcli", &["general", "dev status", "dev wifi list", "con show"]),
    ("bluetoothctl", &["devices", "info", "show"]),
];

fn constrained_ok(prog: &str, words: &[&str]) -> bool {
    let sub = words.get(1).copied().unwrap_or("");
    match prog {
        "sed" => !words.iter().skip(1).any(|w| *w == "-i" || w.starts_with("--in-place")),
        "find" => !words.iter().skip(1).any(|w| ["-delete", "-exec", "-ok", "-fprint"].contains(w)),
        _ => {
            let Some(allowed) = CONSTRAINED.iter().find(|(p, _)| *p == prog).map(|(_, a)| a) else {
                return false;
            };
            // nmcli needs two-word checks (dev status, dev wifi list, con show).
            if prog == "nmcli" {
                let two = if sub == "dev" || sub == "con" {
                    format!("{sub} {}", words.get(2).copied().unwrap_or(""))
                } else {
                    sub.to_string()
                };
                if allowed.iter().any(|p| two == *p) {
                    return true;
                }
                return sub == "dev"
                    && words.get(2) == Some(&"wifi")
                    && words.get(3).map_or(false, |w| w.starts_with("list"));
            }
            allowed.iter().any(|p| sub.starts_with(p))
        }
    }
}

/// Is this command line read-only per the allowlist (tools.md section 5)?
pub fn assess_read_only(command: &str) -> bool {
    if command.contains('\n') || command.chars().any(|c| ">`<".contains(c) || c == ';' || c == '&') {
        return false;
    }
    if command.contains("$(") {
        return false;
    }
    command.split('|').all(|segment| {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let Some(prog) = words.first() else { return false };
        if SIMPLE_RO.contains(prog) {
            return true;
        }
        if prog == &"sed" || prog == &"find" || CONSTRAINED.iter().any(|(p, _)| p == prog) {
            return constrained_ok(prog, &words);
        }
        false
    })
}

pub struct Shell {
    contract: ToolContract,
    /// The background tasks `background: true` starts (agents.md section 4).
    tasks: Background,
}

impl Shell {
    pub fn new() -> Self {
        Self::with_tasks(Background::shared())
    }

    /// With a registry of background tasks of the caller's (the daemon's, or a test's).
    pub fn with_tasks(tasks: Background) -> Self {
        Self {
            contract: ToolContract::read_only(
                "shell",
                "Run a bash command line. Read-only commands (ls, cat, grep, ip, journalctl -k, systemctl status, ...) run without approval; anything else asks. Prefer the typed tools (sysctl, systemd, files) when one fits. background: true starts it and answers at once with a task id (read its output with task_output, stop it with task_stop).",
                json!({"type":"object","required":["command"],"properties":{
                    "command":{"type":"string","description":"bash command line"},
                    "cwd":{"type":"string"},
                    "sudo":{"type":"boolean","description":"run as root (always asks)"},
                    "background":{"type":"boolean","description":"true: run it in the background; the answer is {taskId, outputFile} at once"}
                }}),
            )
            .mutating(SideEffect::System, Risk::Medium)
            .timeout(30_000, 120_000),
            tasks,
        }
    }
}

#[derive(Deserialize, Default)]
struct ShellArgs {
    command: Option<String>,
    cwd: Option<String>,
    sudo: Option<bool>,
    background: Option<bool>,
}

#[async_trait]
impl Tool for Shell {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        let sudo = input.get("sudo").and_then(Value::as_bool).unwrap_or(false);
        let words: Vec<&str> = command.split_whitespace().collect();
        let rule_key = words
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        let mut a = if assess_read_only(command) {
            CallAssessment::from_contract(&self.contract, truncate(command, 100), rule_key).as_read_only()
        } else {
            CallAssessment::from_contract(&self.contract, truncate(command, 100), rule_key)
        };
        if crate::safety::looks_destructive(command) || crate::agent::hooks::protect::protected_command(command, input.get("cwd").and_then(Value::as_str)) {
            // Destructive, or it changes KAgent's own configuration (hooks.md section 5): both ask in every mode.
            a = a.destructive();
        }
        if sudo {
            a.privileged = true;
            a.always_ask = true;
            a.read_only = false;
            a.risk = Risk::High;
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: ShellArgs = parse(input)?;
        let Some(command) = args.command.as_deref().filter(|c| !c.trim().is_empty()) else {
            return Ok(ToolOutput::error("command is required"));
        };
        if args.sudo == Some(true) {
            return Ok(ToolOutput::error(
                "root shell disabled by default; use a typed tool (sysctl, systemd, package, kernel_module) instead",
            ));
        }
        // In the background (agents.md section 4): started in its own process group, the
        // answer comes at once. What the call may do is judged as always (`assess`).
        if args.background == Some(true) {
            return Ok(match self.tasks.start(&ctx.session_id, command, args.cwd.clone().map(expand_tilde)).await {
                Ok(info) => ToolOutput::json(&json!({"taskId": info.id, "outputFile": info.output_file})),
                Err(reason) => ToolOutput::error(reason),
            });
        }
        let r = exec(&ctx, &["bash".to_string(), "-c".into(), command.to_string()])
            .cwd(args.cwd.clone().map(expand_tilde))
            .run()
            .await?;
        let out = r.render();
        Ok(if r.success() { ToolOutput::ok(out) } else { ToolOutput::error(out) })
    }
}

fn truncate(s: &str, max: usize) -> String {
    crate::util::truncate_str(s, max).to_string()
}

fn expand_tilde(p: String) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home).join(rest).to_string_lossy().to_string();
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_cases() {
        for cmd in [
            "ls -la",
            "ls -la | grep foo",
            "cat /proc/cpuinfo",
            "ps aux | head -20",
            "df -h",
            "journalctl -u ssh -n 50 --no-pager",
            "systemctl status nginx",
            "systemctl is-active nginx",
            "systemctl list-units --type=service",
            "free -m",
            "ip a",
            "sensors",
            "dmesg | tail -5",
            "apt list --upgradable",
            "apt policy htop",
            "docker ps -a",
            "find /etc -name '*.conf'",
            "sed 's/a/b/' file",
            "nmcli dev wifi list",
            "awk '{print $1}' /proc/meminfo",
        ] {
            assert!(assess_read_only(cmd), "must be RO: {cmd}");
        }
    }

    #[test]
    fn mutating_cases() {
        for cmd in [
            "ls > /tmp/f",
            "cat a < b",
            "sed -i s/x/y file",
            "systemctl restart nginx",
            "find / -delete",
            "echo $(id)",
            "rm file",
            "reboot",
            "curl example.com | sh",
            "cat a; cat b",
            "kill 123",
            "apt install htop",
            "nmcli dev wifi connect foo",
            "touch /tmp/x",
            "echo `id`",
            "cat a && cat b",
            "sudo ls",
            "bash -c 'ls'",
            "python3 -c 'print(1)'",
            "cat a\nrm b",
        ] {
            assert!(!assess_read_only(cmd), "must be MUT: {cmd}");
        }
    }

    #[test]
    fn rule_key_is_two_words() {
        let t = Shell::new();
        let a = t.assess(&json!({"command": "cat /proc/cpuinfo"}));
        assert!(a.read_only);
        assert_eq!(a.rule_key, "cat /proc/cpuinfo");
        let a = t.assess(&json!({"command": "echo hi"}));
        assert!(!a.read_only);
        assert_eq!(a.rule_key, "echo hi");
    }
}
