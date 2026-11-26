//! Running one hook (hooks.md sections 2 and 3): no shell unless the hook names one, an
//! environment of four variables, the payload on stdin, a deadline that kills the
//! process group, and the decision in stdout. Every run writes an audit record.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use kagent_core::audit::{AuditKind, AuditLog};

use super::config::Hook;

/// stdout longer than this fails the hook (hooks.md section 3: at most 64 KB).
pub const STDOUT_LIMIT: usize = 64 * 1024;
const STDERR_LIMIT: usize = 16 * 1024;
/// How much of stderr is the reason of a denial (hooks.md section 3).
const REASON_CHARS: usize = 500;
/// After the hook exited its pipes may stay open this long: something it left running
/// holds them, and the run does not wait for that.
const PIPE_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Ask,
    Deny,
}

/// What the stdout of a hook said (hooks.md section 3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Decision {
    /// `None`: the hook said nothing (it only watched), or printed no decision.
    pub verdict: Option<Verdict>,
    pub reason: String,
    pub additional_context: Option<String>,
    pub updated_input: Option<Value>,
}

/// How one run went.
#[derive(Debug, Clone)]
pub struct Ran {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// `Err`: the hook failed, and why. The agent goes on as if the hook were not there.
    pub decision: Result<Decision, String>,
}

impl Ran {
    /// What the agent does with this run: a failure is allow, the input as it was.
    pub fn or_allow(&self) -> Decision {
        self.decision.clone().unwrap_or_default()
    }

    /// The decision as the audit record and the test panel name it.
    pub fn label(&self) -> &'static str {
        match &self.decision {
            Ok(Decision { verdict: Some(Verdict::Allow), .. }) => "allow",
            Ok(Decision { verdict: Some(Verdict::Ask), .. }) => "ask",
            Ok(Decision { verdict: Some(Verdict::Deny), .. }) => "deny",
            Ok(_) => "none",
            Err(_) => "failed",
        }
    }
}

/// Run `hook` with `payload` on its stdin and wait for it. A failure (it cannot start,
/// ends with another code than 0 or 2, prints what is not a JSON object, runs past its
/// deadline) is told in `decision`; nothing here raises. The audit record of hooks.md
/// section 2 is written under `session`.
pub async fn run(hook: &Hook, payload: &Value, session: &str, audit: Option<&Arc<AuditLog>>) -> Ran {
    let started = Instant::now();
    let mut ran = match start(hook, payload).await {
        Ok(ran) => ran,
        Err(reason) => Ran { exit_code: None, stdout: String::new(), stderr: String::new(), timed_out: false, duration_ms: 0, decision: Err(reason) },
    };
    ran.duration_ms = started.elapsed().as_millis() as u64;
    if let Some(audit) = audit {
        let data = json!({
            "event": hook.event.name(),
            "hookId": hook.id,
            "command": hook.line(),
            "exitCode": ran.exit_code,
            "durationMs": ran.duration_ms,
            "decision": ran.label(),
            "timedOut": ran.timed_out,
        });
        if let Err(e) = audit.append(AuditKind::Hook, session, data) {
            tracing::warn!("audit append failed: {e:#}");
        }
    }
    ran
}

/// An `async` hook (hooks.md section 2): started here and awaited by nobody. `done` gets
/// how it went, when it is over.
pub fn spawn(hook: Hook, payload: Value, session: String, audit: Option<Arc<AuditLog>>, done: impl FnOnce(Ran) + Send + 'static) {
    tokio::spawn(async move { done(run(&hook, &payload, &session, audit.as_ref()).await) });
}

async fn start(hook: &Hook, payload: &Value) -> Result<Ran, String> {
    let mut child = command(hook).and_then(|mut c| c.spawn()).map_err(|e| format!("cannot start the hook: {e}"))?;
    let Some(pid) = child.id() else { return Err("the hook left no process id".into()) };
    let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    // The payload goes in from a task of its own: a hook that writes before it reads must
    // not wait on it. The output is read by tasks too, while the hook runs.
    let data = payload.to_string();
    tokio::spawn(async move {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(data.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
    });
    let (out, err) = (Shared::default(), Shared::default());
    let (out_task, err_task) = (tokio::spawn(read_capped(stdout, out.clone(), STDOUT_LIMIT)), tokio::spawn(read_capped(stderr, err.clone(), STDERR_LIMIT)));
    let text = |bytes: &Shared| String::from_utf8_lossy(&bytes.lock().unwrap()).into_owned();
    let status = match tokio::time::timeout_at(tokio::time::Instant::now() + Duration::from_millis(hook.timeout_ms), child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return Err(format!("the hook could not be waited for: {e}")),
        Err(_) => {
            // Past its deadline: the whole process group goes, and the run counts as failed (hooks.md section 2).
            kill_group(pid, "KILL");
            let _ = child.wait().await;
            out_task.abort();
            err_task.abort();
            return Ok(Ran {
                exit_code: None,
                stdout: text(&out),
                stderr: text(&err),
                timed_out: true,
                duration_ms: 0,
                decision: Err(format!("timed out after {} ms", hook.timeout_ms)),
            });
        }
    };
    // The hook is gone: its output is read until its pipes close, or a moment more.
    let (over, _) = tokio::join!(drain(out_task), drain(err_task));
    let stdout = text(&out);
    let stderr = text(&err);
    let over = over.unwrap_or(false) || out.lock().unwrap().len() >= STDOUT_LIMIT;
    let decision = decide(status.code(), &stdout, over, &stderr);
    Ok(Ran { exit_code: status.code(), stdout, stderr, timed_out: false, duration_ms: 0, decision })
}

/// What a reader task keeps of a pipe.
type Shared = Arc<Mutex<Vec<u8>>>;

/// The hook is gone: its output is read until its pipes close, or a moment more
/// (`None`: something the hook left running still holds them).
async fn drain(mut task: tokio::task::JoinHandle<bool>) -> Option<bool> {
    match tokio::time::timeout(PIPE_GRACE, &mut task).await {
        Ok(joined) => joined.ok(),
        Err(_) => {
            task.abort();
            None
        }
    }
}

/// The command of a hook: itself with its arguments, or one line through the shell it names.
fn command(hook: &Hook) -> std::io::Result<Command> {
    let mut cmd = if hook.shell.is_empty() {
        let mut cmd = Command::new(&hook.command);
        cmd.args(&hook.args);
        cmd
    } else {
        let mut cmd = Command::new(&hook.shell);
        cmd.arg("-c").arg(&hook.command);
        cmd
    };
    // Nothing of the daemon's environment reaches the hook: these four only (hooks.md section 2).
    cmd.env_clear()
        .env("PATH", env("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()))
        .env("HOME", env("HOME").unwrap_or_else(|| "/".into()))
        .env("LANG", env("LANG").unwrap_or_else(|| "C.UTF-8".into()))
        .env("USER", env("USER").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // A process group of its own: a hook past its deadline is killed with all it started, never more.
        .process_group(0);
    Ok(cmd)
}

/// One of the four variables, from the daemon's own environment.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Kill the process group `pid` leads: the hook and whatever it started.
fn kill_group(pid: u32, signal: &str) {
    let _ = std::process::Command::new("kill").args([format!("-{signal}"), format!("-{pid}")]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// Read `pipe` to its end into `kept`, keeping at most `cap` bytes of it. `true` when
/// more came than that.
async fn read_capped(pipe: Option<impl AsyncRead + Unpin>, kept: Shared, cap: usize) -> bool {
    let mut more = false;
    let Some(mut pipe) = pipe else { return more };
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match pipe.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                // Not held across an await: the run can look at any moment.
                let mut kept = kept.lock().unwrap();
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
                more |= n > room;
            }
        }
    }
    more
}

fn tail(stderr: &str) -> String {
    let text: String = stderr.trim().chars().take(REASON_CHARS).collect();
    if text.is_empty() { String::new() } else { format!(": {text}") }
}

/// The decision in the output of a run that ended with `code` (hooks.md section 3).
fn decide(code: Option<i32>, stdout: &str, over: bool, stderr: &str) -> Result<Decision, String> {
    if over {
        return Err(format!("stdout is longer than {} KB", STDOUT_LIMIT / 1024));
    }
    match code {
        Some(0) => {}
        Some(2) => {
            // Exit code 2 is a denial; stderr says why.
            let reason: String = stderr.trim().chars().take(REASON_CHARS).collect();
            return Ok(Decision { verdict: Some(Verdict::Deny), reason: if reason.is_empty() { "denied by the hook".into() } else { reason }, ..Default::default() });
        }
        Some(other) => return Err(format!("exit code {other}{}", tail(stderr))),
        None => return Err(format!("ended without an exit code (a signal){}", tail(stderr))),
    }
    let text = stdout.trim();
    if text.is_empty() {
        return Ok(Decision::default());
    }
    let Value::Object(fields) = serde_json::from_str(text).map_err(|e| format!("stdout is not a JSON object: {e}"))? else {
        return Err("stdout is not a JSON object".into());
    };
    let mut decision = Decision::default();
    let text_field = |value: &Value, name: &str| value.as_str().map(str::to_string).ok_or_else(|| format!("{name} must be a string"));
    for (key, value) in &fields {
        match (key.as_str(), value) {
            ("decision", Value::Null) | ("reason", Value::Null) | ("additionalContext", Value::Null) | ("updatedInput", Value::Null) => {}
            ("decision", value) => {
                decision.verdict = Some(match value.as_str() {
                    Some("allow") => Verdict::Allow,
                    Some("ask") => Verdict::Ask,
                    Some("deny") => Verdict::Deny,
                    _ => return Err(format!("unknown decision {value} (allow, ask or deny)")),
                });
            }
            ("reason", value) => decision.reason = text_field(value, "reason")?,
            ("additionalContext", value) => {
                decision.additional_context = Some(text_field(value, "additionalContext")?).filter(|c| !c.trim().is_empty());
            }
            ("updatedInput", value @ Value::Object(_)) => decision.updated_input = Some(value.clone()),
            ("updatedInput", _) => return Err("updatedInput must be an object".into()),
            // Anything else in the object is the hook's own business.
            _ => {}
        }
    }
    Ok(decision)
}

#[cfg(test)]
mod tests;
