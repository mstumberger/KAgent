//! measure, undo_change, list_changes (docs/specs/tools.md section 8). Split from
//! change.rs to keep files small.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use super::read_trim;
use crate::journal::ChangeStatus;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

// ---------------------------------------------------------------- measure

pub struct Measure {
    contract: ToolContract,
}

impl Measure {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "measure",
                "Quick metrics for before/after comparisons around a change: boot time, memory, thermal, cpu, network latency. Call it before and after every change and report the difference.",
                json!({"type":"object","properties":{"kind":{"type":"string","enum":["boot","memory","thermal","cpu","network","all"],"description":"default all"}}}),
            ),
        }
    }
}

pub async fn measure(ctx: &ToolContext, kind: &str) -> String {
    let mut out = Vec::new();
    let all = kind == "all";
    if all || kind == "boot" {
        let t = exec(ctx, &["systemd-analyze", "time"]).run().await.map(|r| r.stdout.lines().next().unwrap_or("").to_string()).unwrap_or_default();
        out.push(format!("boot: {}", t.trim()));
    }
    if all || kind == "memory" {
        let mi = read_trim("/proc/meminfo").await.unwrap_or_default();
        let get = |k: &str| mi.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) / 1024;
        out.push(format!("memory MiB: total {} available {} swap used {}", get("MemTotal:"), get("MemAvailable:"), get("SwapTotal:").saturating_sub(get("SwapFree:"))));
    }
    if all || kind == "thermal" {
        let snap = super::sys::system_snapshot().await;
        let max = snap["temperatures"].as_array().map(|a| a.iter().filter_map(|t| t["celsius"].as_f64()).fold(f64::MIN, f64::max)).filter(|m| *m > f64::MIN);
        let gpu = exec(ctx, &["nvidia-smi", "--query-gpu=temperature.gpu", "--format=csv,noheader"]).run().await.ok().filter(|r| r.success()).map(|r| r.stdout.trim().to_string());
        out.push(format!("thermal C: max zone {} gpu {}", max.map(|m| format!("{m:.1}")).unwrap_or("?".into()), gpu.unwrap_or("?".into())));
    }
    if all || kind == "cpu" {
        let load = read_trim("/proc/loadavg").await.unwrap_or_default();
        out.push(format!("load: {}", load.split_whitespace().take(3).collect::<Vec<_>>().join(" ")));
        let ps = exec(ctx, &["ps", "-eo", "pcpu,comm", "--sort=-pcpu", "--no-headers"]).run().await.map(|r| r.stdout.lines().take(5).map(|l| l.trim().to_string()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
        out.push(format!("top cpu: {ps}"));
    }
    if all || kind == "network" {
        let p = exec(ctx, &["ping", "-c3", "-W1", "-q", "1.1.1.1"]).run().await.map(|r| r.stdout.lines().last().unwrap_or("").to_string()).unwrap_or_default();
        out.push(format!("ping 1.1.1.1: {}", if p.is_empty() { "unreachable".into() } else { p }));
    }
    out.join("\n")
}

#[async_trait]
impl Tool for Measure {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let kind = input.get("kind").and_then(Value::as_str).unwrap_or("all").to_string();
        Ok(ToolOutput::ok(measure(&ctx, &kind).await))
    }
}

// ---------------------------------------------------------------- undo_change / list_changes

pub struct UndoChange {
    contract: ToolContract,
}

impl UndoChange {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "undo_change",
                "Undo a change KAgent made earlier, by change id (see list_changes).",
                json!({"type":"object","required":["change_id"],"properties":{"change_id":{"type":"string"}}}),
            )
            .mutating(SideEffect::System, Risk::Medium)
            .timeout(120_000, 600_000),
        }
    }
}

/// Run a journaled undo. Root undos go through the helper; user undos restore a backup
/// or run the recorded argv.
pub async fn run_undo(ctx: &ToolContext, change_id: &str) -> anyhow::Result<ToolOutput> {
    let services = ctx.services()?;
    let mut change = services.journal.get(change_id)?;
    if change.status != ChangeStatus::Applied {
        return Ok(ToolOutput::error(format!("change {change_id} is {:?}; only applied changes can be undone", change.status)));
    }
    let Some(undo) = change.undo.clone() else {
        return Ok(ToolOutput::error(format!("change {change_id} ({}) has no automatic undo", change.summary)));
    };
    let handler = undo.get("action").and_then(Value::as_str).and_then(|a| services.undo.read().unwrap().iter().find(|h| h.action() == a).cloned());
    let (ok, detail) = if undo.get("op").is_some() {
        let r = services.priv_client.call(ctx, &undo).await?;
        (r.ok, format!("{}{}", r.stdout.trim(), r.message.map(|m| format!("\n{m}")).unwrap_or_default()))
    } else if let Some(handler) = handler {
        handler.undo(&undo).await
    } else {
        match undo.get("action").and_then(Value::as_str) {
            Some("restore_file") => {
                let path = PathBuf::from(undo["path"].as_str().unwrap_or_default());
                match undo.get("backup").and_then(Value::as_str) {
                    Some(b) => match tokio::fs::copy(Path::new(b), &path).await {
                        Ok(_) => (true, format!("restored {}", path.display())),
                        Err(e) => (false, format!("restore failed: {e}")),
                    },
                    None => match tokio::fs::remove_file(&path).await {
                        Ok(_) => (true, format!("removed {} (it did not exist before)", path.display())),
                        Err(e) => (false, format!("remove failed: {e}")),
                    },
                }
            }
            Some("command") => {
                let argv: Vec<String> = undo["argv"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default();
                if argv.is_empty() {
                    (false, "empty undo command".into())
                } else {
                    let r = exec(ctx, &argv).run().await?;
                    (r.success(), r.render())
                }
            }
            _ => (false, "unknown undo kind".into()),
        }
    };
    change.status = if ok { ChangeStatus::Undone } else { ChangeStatus::UndoFailed };
    change.undone_at = Some(chrono::Utc::now());
    change.note = Some(detail.clone());
    services.journal.save(&change)?;
    let text = format!("{} undo of {} ({})\n{detail}", if ok { "done:" } else { "FAILED:" }, change.id, change.summary);
    Ok(if ok { ToolOutput::ok(text) } else { ToolOutput::error(text) })
}

#[async_trait]
impl Tool for UndoChange {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let id = input.get("change_id").and_then(Value::as_str).unwrap_or("?").to_string();
        // Undoing a root change needs root: look it up so the card says so.
        let mut a = CallAssessment::from_contract(&self.contract, format!("undo {id}"), format!("undo {id}"));
        a.summary = format!("undo change {id}");
        a
    }
    async fn preview(&self, input: &Value, ctx: &ToolContext) -> Option<String> {
        let id = input.get("change_id")?.as_str()?;
        let c = ctx.services().ok()?.journal.get(id).ok()?;
        Some(format!("{}\nstatus: {:?}\nundo: {}", c.summary, c.status, c.undo.map(|u| u.to_string()).unwrap_or_else(|| "none".into())))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let id = input.get("change_id").and_then(Value::as_str).unwrap_or_default().to_string();
        run_undo(&ctx, &id).await
    }
}

pub struct ListChanges {
    contract: ToolContract,
}

impl ListChanges {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "list_changes",
                "Changes KAgent made on this machine, newest first, with ids for undo_change.",
                json!({"type":"object","properties":{"limit":{"type":"integer","description":"default 20"}}}),
            ),
        }
    }
}

#[async_trait]
impl Tool for ListChanges {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(20).min(200) as usize;
        let changes = ctx.services()?.journal.list(limit);
        if changes.is_empty() {
            return Ok(ToolOutput::ok("no changes recorded yet"));
        }
        let rows: Vec<String> = changes
            .iter()
            .map(|c| format!("{} {} {:?}{} {}", c.id, c.ts.format("%Y-%m-%d %H:%M"), c.status, if c.undo.is_some() { "" } else { " (no undo)" }, c.summary))
            .collect();
        Ok(ToolOutput::ok(rows.join("\n")))
    }
}

