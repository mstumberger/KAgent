//! `system_power` tool (docs/specs/tools.md section 15): reboot, power off, suspend,
//! lock the screen, or cancel a scheduled reboot. Goes through systemd-logind, which
//! allows the local active user without root. Reboot/power off always ask.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

pub struct SystemPower {
    contract: ToolContract,
}

impl SystemPower {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "system_power",
                "Reboot, power off, suspend, or lock the screen; `minutes` schedules a reboot/power off (cancel undoes it). Use for 'reboot to apply the kernel update'. Always asks first; warn about unsaved work.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["reboot","poweroff","suspend","lock","cancel"]},
                    "minutes":{"type":"integer","minimum":0,"maximum":1440}
                }}),
            )
            .mutating(SideEffect::System, Risk::High),
        }
    }
}

/// argv for an action (pure, tested).
pub fn power_argv(action: &str, minutes: u64) -> Option<Vec<String>> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    Some(match action {
        "reboot" | "poweroff" if minutes > 0 => vec!["systemctl".into(), action.into(), format!("--when=+{minutes}min")],
        "reboot" | "poweroff" => v(&["systemctl", action]),
        "suspend" => v(&["systemctl", "suspend"]),
        "lock" => v(&["loginctl", "lock-session"]),
        "cancel" => v(&["systemctl", "reboot", "--when=cancel"]),
        _ => return None,
    })
}

#[async_trait]
impl Tool for SystemPower {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let m = input.get("minutes").and_then(Value::as_u64).unwrap_or(0);
        let when = if m > 0 { format!(" in {m} min") } else { " now".into() };
        let mut a = CallAssessment::from_contract(&self.contract, format!("{action}{}", if matches!(action, "reboot" | "poweroff") { when } else { String::new() }), format!("system_power {action}"));
        match action {
            "lock" | "cancel" => {
                a.needs_approval = false;
                a.risk = Risk::Low;
            }
            "reboot" | "poweroff" => {
                a.always_ask = true;
                a = a.destructive();
            }
            _ => {}
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let minutes = input.get("minutes").and_then(Value::as_u64).unwrap_or(0).min(1440);
        // Boot-loop guard: right after a boot a resumed conversation must not reboot again
        // (happened: "Ezel" and "b" right after a reboot each rebooted once more).
        if matches!(action, "reboot" | "poweroff") && minutes == 0 {
            let up = std::fs::read_to_string("/proc/uptime").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok())).unwrap_or(1e9);
            if up < 600.0 {
                return Ok(ToolOutput::error(format!("refused: the system booted {} minutes ago; reboot/power off is blocked for the first 10 minutes after boot (ask the user, then schedule with `minutes`)", (up / 60.0).round())));
            }
        }
        let Some(argv) = power_argv(action, minutes) else { return Ok(ToolOutput::error("action must be reboot, poweroff, suspend, lock or cancel")) };
        let r = exec(&ctx, &argv).run().await?;
        Ok(if r.success() {
            ToolOutput::ok(match (action, minutes) {
                ("reboot" | "poweroff", m) if m > 0 => format!("{action} scheduled in {m} minutes (system_power cancel undoes it)"),
                _ => format!("{action} requested"),
            })
        } else {
            ToolOutput::error(format!("{action} refused: {} (another user logged in, or polkit wants a password)", r.render()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_and_assessment() {
        assert_eq!(power_argv("reboot", 0).unwrap(), vec!["systemctl", "reboot"]);
        assert_eq!(power_argv("reboot", 5).unwrap(), vec!["systemctl", "reboot", "--when=+5min"]);
        assert_eq!(power_argv("lock", 0).unwrap(), vec!["loginctl", "lock-session"]);
        assert!(power_argv("halt", 0).is_none());
        let t = SystemPower::new();
        assert!(t.assess(&json!({"action": "reboot"})).always_ask);
        assert!(t.assess(&json!({"action": "reboot"})).destructive && t.assess(&json!({"action": "poweroff"})).destructive);
        assert!(!t.assess(&json!({"action": "lock"})).destructive);
        assert!(!t.assess(&json!({"action": "lock"})).needs_approval);
        assert!(t.assess(&json!({"action": "suspend"})).needs_approval);
    }
}
