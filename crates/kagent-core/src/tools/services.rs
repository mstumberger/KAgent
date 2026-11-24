//! systemd tools: systemd, journal, boot_analysis (tools.md section 3).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::exec::exec;
use super::parse;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

// ---------------------------------------------------------------- systemd

pub struct Systemd {
    contract: ToolContract,
}

impl Systemd {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "systemd",
                "systemd units: status, list (filter by state), or control actions start/stop/restart/reload/enable/disable (control requires the root helper, stage S8).",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["status","list","start","stop","restart","reload","enable","disable"]},
                    "unit":{"type":"string","description":"unit name; .service is assumed when no suffix"},
                    "state":{"type":"string","enum":["running","failed","enabled","all"],"description":"list filter, default running"},
                    "scope":{"type":"string","enum":["system","user"],"description":"default system"}
                }}),
            )
            .mutating(SideEffect::System, Risk::Medium)
            .timeout(30_000, 120_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct SystemdArgs {
    action: Option<String>,
    unit: Option<String>,
    state: Option<String>,
    scope: Option<String>,
}

pub fn valid_unit(unit: &str) -> bool {
    !unit.is_empty()
        && unit.len() <= 128
        && unit.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '_' | '.' | ':' | '-'))
}

/// Units KAgent must never stop or disable (tools.md section 3).
pub fn protected_unit(unit: &str) -> bool {
    let u = unit.strip_suffix(".service").unwrap_or(unit);
    u == "dbus"
        || u == "polkit"
        || u == "ssh"
        || u == "sshd"
        || u.starts_with("systemd-")
        || u.starts_with("kagent")
}

const CONTROL_ACTIONS: [&str; 6] = ["start", "stop", "restart", "reload", "enable", "disable"];

#[async_trait]
impl Tool for Systemd {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("status");
        let unit = input.get("unit").and_then(Value::as_str).unwrap_or("");
        let scope = input.get("scope").and_then(Value::as_str).unwrap_or("system");
        let summary = if action == "list" {
            format!("systemctl list ({})", input.get("state").and_then(Value::as_str).unwrap_or("running"))
        } else if unit.is_empty() {
            format!("systemctl {action}")
        } else {
            format!("systemctl {action} {unit}")
        };
        let mut a = CallAssessment::from_contract(&self.contract, summary.clone(), summary);
        if CONTROL_ACTIONS.contains(&action) && scope == "system" {
            // Root-scope control goes through the root helper.
            a.privileged = true;
            a.always_ask = true;
        }
        if action == "status" || action == "list" {
            a = a.as_read_only();
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: SystemdArgs = parse(input)?;
        let user = args.scope.as_deref() == Some("user");
        match args.action.as_deref() {
            Some("status") => {
                let Some(unit) = args.unit.as_deref().filter(|u| valid_unit(u)) else {
                    return Ok(ToolOutput::error("status requires a valid unit name"));
                };
                let unit = with_service_suffix(unit);
                let mut argv = base_argv(user);
                argv.extend(["status".into(), unit, "--no-pager".into(), "-n".into(), "20".into(), "-l".into()]);
                // systemctl status exits 3 for inactive units: still a valid answer.
                let r = exec(&ctx, &argv).run().await;
                Ok(match r {
                    Ok(r) if matches!(r.code, Some(0..=3)) && !r.timed_out => ToolOutput::ok(r.render()),
                    other => render(other),
                })
            }
            Some("list") => {
                let state = args.state.as_deref().unwrap_or("running");
                let mut argv = base_argv(user);
                match state {
                    "failed" => argv.extend(["--failed".into(), "--no-pager".into(), "--plain".into()]),
                    "running" => argv.extend([
                        "list-units".into(),
                        "--type=service".into(),
                        "--state=running".into(),
                        "--no-pager".into(),
                        "--plain".into(),
                    ]),
                    "enabled" => argv.extend([
                        "list-unit-files".into(),
                        "--state=enabled".into(),
                        "--no-pager".into(),
                        "--plain".into(),
                    ]),
                    _ => argv.extend([
                        "list-units".into(),
                        "--all".into(),
                        "--type=service".into(),
                        "--no-pager".into(),
                        "--plain".into(),
                    ]),
                }
                let r = exec(&ctx, &argv).run().await;
                Ok(render(r))
            }
            Some(action) if CONTROL_ACTIONS.contains(&action) => {
                let Some(unit) = args.unit.as_deref().filter(|u| valid_unit(u)) else {
                    return Ok(ToolOutput::error(format!("{action} requires a valid unit name")));
                };
                let unit = with_service_suffix(unit);
                if matches!(action, "stop" | "disable") && protected_unit(&unit) {
                    return Ok(ToolOutput::error(format!(
                        "refusing to {action} {unit}: it would break this machine's management (dbus/systemd/polkit/ssh/kagentd)"
                    )));
                }
                if !user {
                    let summary = format!("systemctl {action} {unit}");
                    return super::privop::apply_root(&ctx, "systemd", &summary, json!({"op": "systemctl", "action": action, "unit": unit})).await;
                }
                let before = unit_state(&ctx, true, &unit).await;
                let mut argv = base_argv(true);
                argv.extend([action.to_string(), unit.clone()]);
                let r = exec(&ctx, &argv).run().await?;
                let after = unit_state(&ctx, true, &unit).await;
                let (active, enabled) = before.split_once('/').unwrap_or(("", ""));
                let undo_action = match action {
                    "start" if active != "active" => Some("stop"),
                    "stop" if active == "active" => Some("start"),
                    "enable" if enabled != "enabled" => Some("disable"),
                    "disable" if enabled == "enabled" => Some("enable"),
                    _ => None,
                };
                let undo = undo_action.map(|u| json!({"kind": "user", "action": "command", "argv": ["systemctl", "--user", u, unit]}));
                super::privop::record_user(
                    &ctx,
                    "systemd",
                    &format!("systemctl --user {action} {unit}"),
                    json!({"kind": "user", "action": "command", "argv": argv}),
                    r.success(),
                    undo,
                    Some(before),
                    Some(after),
                    &r.render(),
                )
            }
            _ => Ok(ToolOutput::error("action must be status, list or a control action")),
        }
    }

    async fn preview(&self, input: &Value, ctx: &ToolContext) -> Option<String> {
        let unit = with_service_suffix(input.get("unit").and_then(Value::as_str).filter(|u| valid_unit(u))?);
        let user = input.get("scope").and_then(Value::as_str) == Some("user");
        let state = unit_state(ctx, user, &unit).await;
        Some(format!("{unit}: currently {state} (active/enabled)"))
    }
}

/// `active/enabled` state of a unit, e.g. `active/enabled`, `inactive/disabled`.
async fn unit_state(ctx: &ToolContext, user: bool, unit: &str) -> String {
    let mut out = Vec::new();
    for what in ["is-active", "is-enabled"] {
        let mut argv = base_argv(user);
        argv.extend([what.to_string(), unit.to_string()]);
        let v = exec(ctx, &argv).run().await.map(|r| r.stdout.trim().to_string()).unwrap_or_default();
        out.push(if v.is_empty() { "unknown".to_string() } else { v });
    }
    out.join("/")
}

fn base_argv(user: bool) -> Vec<String> {
    if user {
        vec!["systemctl".into(), "--user".into()]
    } else {
        vec!["systemctl".into()]
    }
}

fn with_service_suffix(unit: &str) -> String {
    if unit.contains('.') {
        unit.to_string()
    } else {
        format!("{unit}.service")
    }
}

fn render(r: anyhow::Result<super::exec::ExecResult>) -> ToolOutput {
    match r {
        Ok(r) if r.success() => ToolOutput::ok(r.render()),
        Ok(r) => ToolOutput::error(r.render()),
        Err(e) => ToolOutput::error(format!("error: {e:#}")),
    }
}

// ---------------------------------------------------------------- journal

pub struct Journal {
    contract: ToolContract,
}

#[derive(Deserialize, Default)]
pub struct JournalArgs {
    pub unit: Option<String>,
    pub priority: Option<String>,
    pub since: Option<String>,
    pub grep: Option<String>,
    pub lines: Option<usize>,
    pub kernel: Option<bool>,
    pub boot: Option<i64>,
    pub scope: Option<String>,
}

/// Pure argv builder (tools.md section 3, journal). Returns an error string when a
/// value would let the model smuggle extra argv into journalctl.
pub fn journal_argv(a: &JournalArgs) -> Result<Vec<String>, String> {
    let mut argv = vec![
        "journalctl".to_string(),
        "--no-pager".into(),
        "-o".into(),
        "short-iso".into(),
        "-n".into(),
        a.lines.unwrap_or(100).clamp(1, 500).to_string(),
    ];
    if a.scope.as_deref() == Some("user") {
        argv.push("--user".into());
    }
    if let Some(u) = &a.unit {
        if !valid_unit(u) {
            return Err(format!("invalid unit name '{u}'"));
        }
        argv.push("-u".into());
        argv.push(u.clone());
    }
    if let Some(p) = &a.priority {
        const P: [&str; 8] = ["emerg", "alert", "crit", "err", "warning", "notice", "info", "debug"];
        if !P.contains(&p.as_str()) {
            return Err(format!("invalid priority '{p}'"));
        }
        argv.push("-p".into());
        argv.push(p.clone());
    }
    if let Some(s) = &a.since {
        if s.chars().any(|c| ";|&$`".contains(c)) {
            return Err("since may not contain ; | & $ or backticks".into());
        }
        argv.push("--since".into());
        argv.push(s.clone());
    }
    if let Some(g) = &a.grep {
        if g.is_empty() {
            return Err("grep must not be empty".into());
        }
        argv.push("-g".into());
        argv.push(g.clone());
    }
    if a.kernel == Some(true) {
        argv.push("-k".into());
    }
    if let Some(b) = a.boot {
        argv.push("-b".into());
        argv.push(b.to_string());
    }
    Ok(argv)
}

impl Journal {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "journal",
                "Read the systemd journal: filter by unit, priority, time, regex, kernel messages, boot number.",
                json!({"type":"object","properties":{
                    "unit":{"type":"string"},
                    "priority":{"type":"string","enum":["emerg","alert","crit","err","warning","notice","info","debug"],"description":"max priority, e.g. err shows emerg..err"},
                    "since":{"type":"string","description":"e.g. '1 hour ago', 'today', '2026-11-20 10:00'"},
                    "grep":{"type":"string","description":"regex filter on message"},
                    "lines":{"type":"integer","description":"default 100, max 500"},
                    "kernel":{"type":"boolean"},
                    "boot":{"type":"integer","description":"0 = current boot, -1 = previous"},
                    "scope":{"type":"string","enum":["system","user"]}
                }}),
            )
            .timeout(20_000, 60_000),
        }
    }
}

#[async_trait]
impl Tool for Journal {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: JournalArgs = parse(input)?;
        let argv = match journal_argv(&args) {
            Ok(argv) => argv,
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        let r = exec(&ctx, &argv).run().await;
        Ok(render(r))
    }
}

// ---------------------------------------------------------------- boot_analysis

pub struct BootAnalysis {
    contract: ToolContract,
}

impl BootAnalysis {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "boot_analysis",
                "How long the last boot took and why: systemd-analyze time, top 20 slowest units (blame), critical chain.",
                json!({"type":"object","properties":{
                    "section":{"type":"string","enum":["time","blame","chain"]}
                }}),
            )
            .timeout(30_000, 60_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct BootArgs {
    section: Option<String>,
}

#[async_trait]
impl Tool for BootAnalysis {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: BootArgs = parse(input)?;
        let want = args.section.as_deref();
        let mut out = String::new();
        if want.is_none() || matches!(want, Some("time")) {
            match exec(&ctx, &["systemd-analyze".to_string(), "time".into()]).run().await {
                Ok(r) => {
                    out.push_str("## time\n");
                    out.push_str(&r.render());
                    out.push('\n');
                }
                Err(e) => out.push_str(&format!("## time\nunavailable: {e}\n")),
            }
        }
        if want.is_none() || matches!(want, Some("blame")) {
            match exec(
                &ctx,
                &["systemd-analyze".to_string(), "blame".into(), "--no-pager".into()],
            )
            .run()
            .await
            {
                Ok(r) => {
                    out.push_str("\n## blame (top 20)\n");
                    for line in r.stdout.lines().take(20) {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                Err(e) => out.push_str(&format!("\n## blame\nunavailable: {e}\n")),
            }
        }
        if want.is_none() || matches!(want, Some("chain")) {
            match exec(
                &ctx,
                &["systemd-analyze".to_string(), "critical-chain".into(), "--no-pager".into()],
            )
            .run()
            .await
            {
                Ok(r) => {
                    out.push_str("\n## critical-chain\n");
                    out.push_str(r.stdout.trim_end());
                    out.push('\n');
                }
                Err(e) => out.push_str(&format!("\n## critical-chain\nunavailable: {e}\n")),
            }
        }
        Ok(ToolOutput::ok(out.trim_end().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_argv_builder() {
        let a = JournalArgs { unit: Some("ssh".into()), lines: Some(50), ..Default::default() };
        assert_eq!(
            journal_argv(&a).unwrap(),
            vec!["journalctl", "--no-pager", "-o", "short-iso", "-n", "50", "-u", "ssh"]
        );
        let a = JournalArgs { kernel: Some(true), boot: Some(-1), ..Default::default() };
        let argv = journal_argv(&a).unwrap();
        assert!(argv.contains(&"-k".to_string()) && argv.contains(&"-1".to_string()));

        let bad = JournalArgs { since: Some("x; rm -rf /".into()), ..Default::default() };
        assert!(journal_argv(&bad).is_err());
        let bad = JournalArgs { unit: Some("a;b".into()), ..Default::default() };
        assert!(journal_argv(&bad).is_err());
        let bad = JournalArgs { priority: Some("nope".into()), ..Default::default() };
        assert!(journal_argv(&bad).is_err());
        let ok = JournalArgs { since: Some("1 hour ago".into()), ..Default::default() };
        assert!(journal_argv(&ok).is_ok());
    }

    #[test]
    fn unit_validation() {
        assert!(valid_unit("nginx.service"));
        assert!(valid_unit("user@1000.service"));
        assert!(!valid_unit("a b"));
        assert!(!valid_unit(""));
        assert!(!valid_unit("x;y"));
        assert!(protected_unit("dbus.service"));
        assert!(protected_unit("systemd-journald.service"));
        assert!(protected_unit("kagentd"));
        assert!(!protected_unit("nginx.service"));
    }
}
