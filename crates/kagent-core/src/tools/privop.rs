//! Client for the root helper `kagent-priv` (docs/specs/priv-ops.md) and the shared
//! "apply a change and journal it" path used by every mutating tool.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::Value;

use super::exec::exec;
use crate::journal::{Change, ChangeJournal, ChangeStatus};
use crate::tool::{ToolContext, ToolOutput};

pub const DEFAULT_HELPER: &str = "/usr/local/libexec/kagent-priv";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrivResponse {
    pub ok: bool,
    pub op: String,
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
    pub undo: Option<Value>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub backup: Option<String>,
    pub code: Option<String>,
    pub message: Option<String>,
}

pub struct PrivClient {
    helper: PathBuf,
}

impl PrivClient {
    /// `KAGENT_PRIV` overrides the helper path (tests, development builds).
    pub fn new() -> Self {
        let helper = std::env::var_os("KAGENT_PRIV").map(PathBuf::from).unwrap_or_else(|| DEFAULT_HELPER.into());
        Self { helper }
    }

    pub fn with_path(helper: impl Into<PathBuf>) -> Self {
        Self { helper: helper.into() }
    }

    pub fn helper(&self) -> &std::path::Path {
        &self.helper
    }

    /// `installed` when the helper exists and is owned by root (otherwise sudo would
    /// run something the user can modify).
    pub fn status(&self) -> &'static str {
        use std::os::unix::fs::MetadataExt;
        match std::fs::metadata(&self.helper) {
            Ok(m) if m.uid() == 0 && m.mode() & 0o022 == 0 => "installed",
            Ok(_) => "unsafe",
            Err(_) => "missing",
        }
    }

    pub async fn call(&self, ctx: &ToolContext, op: &Value) -> anyhow::Result<PrivResponse> {
        match self.status() {
            "installed" => {}
            "unsafe" => anyhow::bail!(
                "{} is not root-owned or is writable by others; reinstall it (`{} sudoers`)",
                self.helper.display(),
                kagentd_path()
            ),
            _ => anyhow::bail!(
                "root helper not installed at {}. {}",
                self.helper.display(),
                setup_hint()
            ),
        }
        let mut big = ctx.clone();
        big.max_output_bytes = big.max_output_bytes.max(64 * 1024);
        let helper = self.helper.display().to_string();
        let r = exec(&big, &[helper]).privileged(true).stdin(op.to_string()).run().await?;
        if r.timed_out {
            anyhow::bail!("root helper timed out");
        }
        let line = r.stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or("");
        match serde_json::from_str::<PrivResponse>(line) {
            Ok(resp) => Ok(resp),
            Err(_) if r.stderr.contains("password is required") || r.stderr.contains("a terminal is required") => {
                anyhow::bail!("sudo needs a password for kagent-priv: install the sudoers rule (`{} sudoers`)", kagentd_path())
            }
            Err(_) => anyhow::bail!("root helper gave no answer: {}", r.stderr.trim()),
        }
    }
}

impl Default for PrivClient {
    fn default() -> Self {
        Self::new()
    }
}

fn record(ctx: &ToolContext, tool: &str, summary: &str, op: Value, privileged: bool, ok: bool, undo: Option<Value>, before: Option<String>, after: Option<String>) -> anyhow::Result<Option<Change>> {
    let Some(services) = ctx.services.as_deref() else { return Ok(None) };
    let change = Change {
        id: ChangeJournal::new_id(),
        ts: chrono::Utc::now(),
        session_id: ctx.session_id.clone(),
        call_id: ctx.call_id.clone(),
        tool: tool.into(),
        summary: summary.into(),
        op,
        undo,
        privileged,
        status: if ok { ChangeStatus::Applied } else { ChangeStatus::Failed },
        before,
        after,
        undone_at: None,
        note: None,
    };
    services.journal.save(&change)?;
    Ok(Some(change))
}

fn describe(summary: &str, change: Option<&Change>, before: Option<&str>, after: Option<&str>, detail: &str) -> String {
    let mut out = format!("done: {summary}\n");
    if let Some(b) = before.filter(|b| !b.is_empty() && b.len() < 300) {
        out.push_str(&format!("before: {b}\n"));
    }
    if let Some(a) = after.filter(|a| !a.is_empty() && a.len() < 300) {
        out.push_str(&format!("after: {a}\n"));
    }
    if let Some(c) = change {
        match c.undo {
            Some(_) => out.push_str(&format!("change id: {} (undo available: undo_change)\n", c.id)),
            None => out.push_str(&format!("change id: {} (no automatic undo)\n", c.id)),
        }
    }
    if !detail.trim().is_empty() {
        out.push_str(detail.trim_end());
        out.push('\n');
    }
    out
}

/// Run a root-helper op, journal it, and format the result for the model.
pub async fn apply_root(ctx: &ToolContext, tool: &str, summary: &str, op: Value) -> anyhow::Result<ToolOutput> {
    let services = ctx.services()?;
    let resp = services.priv_client.call(ctx, &op).await?;
    if !resp.ok && resp.code.as_deref().is_some_and(|c| c == "invalid_input" || c == "not_allowed") {
        // Rejected before doing anything: nothing to journal.
        return Ok(ToolOutput::error(format!(
            "refused by the root helper ({}): {}",
            resp.code.unwrap_or_default(),
            resp.message.unwrap_or_default()
        )));
    }
    let detail = [resp.stdout.trim(), resp.stderr.trim()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n");
    let change = record(ctx, tool, summary, op, true, resp.ok, resp.undo.clone(), resp.before.clone(), resp.after.clone())?;
    let text = describe(summary, change.as_ref(), resp.before.as_deref(), resp.after.as_deref(), &crate::util::budget_output(&detail, 6_000));
    Ok(if resp.ok {
        ToolOutput::ok(text)
    } else {
        ToolOutput::error(format!("FAILED ({}): {}\n{text}", resp.code.unwrap_or_default(), resp.message.unwrap_or_default()))
    })
}

/// Journal a change done without root (user units, home files, power profile).
pub fn record_user(ctx: &ToolContext, tool: &str, summary: &str, op: Value, ok: bool, undo: Option<Value>, before: Option<String>, after: Option<String>, detail: &str) -> anyhow::Result<ToolOutput> {
    let change = record(ctx, tool, summary, op, false, ok, undo, before.clone(), after.clone())?;
    let text = describe(summary, change.as_ref(), before.as_deref(), after.as_deref(), detail);
    Ok(if ok { ToolOutput::ok(text) } else { ToolOutput::error(format!("FAILED: {text}")) })
}

/// Query a root-helper op that changes nothing (smartctl, read_root, ufw_status, bpftrace).
/// Full path of the running daemon binary, for commands shown to the owner
/// (`kagentd` is usually not on PATH).
pub fn kagentd_path() -> String {
    std::env::current_exe()
        .ok()
        .filter(|p| p.file_name().is_some_and(|n| n == "kagentd"))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "kagentd".into())
}

/// One line the owner pastes into a terminal to install the helper built next to the
/// daemon and its sudoers rule (None when the helper binary is not built yet).
pub fn setup_oneliner() -> Option<String> {
    let built = std::env::current_exe().ok()?.parent()?.join("kagent-priv");
    if !built.is_file() {
        return None;
    }
    let user = std::env::var("USER").ok().filter(|u| u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))?;
    Some(format!(
        "sudo install -o root -g root -m 0755 {} {DEFAULT_HELPER} && echo '{user} ALL=(root) NOPASSWD: {DEFAULT_HELPER}' | sudo tee /etc/sudoers.d/kagent >/dev/null && sudo chmod 0440 /etc/sudoers.d/kagent && sudo visudo -c",
        built.display()
    ))
}

fn setup_hint() -> String {
    match setup_oneliner() {
        Some(l) => format!("The owner pastes this one line into a terminal (it asks for their password once; put it on the clipboard for them if they want):\n{l}"),
        None => format!("The owner runs `{} sudoers` and then the lines it prints.", kagentd_path()),
    }
}

pub async fn query_root(ctx: &ToolContext, op: &Value) -> anyhow::Result<PrivResponse> {
    let services = ctx.services()?;
    services.priv_client.call(ctx, op).await
}
