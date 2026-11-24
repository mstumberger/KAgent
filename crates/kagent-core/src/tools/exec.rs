//! The single subprocess entry point (ZCode rule: one exec boundary that owns timeout,
//! cancellation, privilege escalation and output capture).

use std::process::Stdio;

use anyhow::{bail, Context};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::tool::{PrivilegeMethod, ToolContext};

#[derive(Debug, Default)]
pub struct ExecResult {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl ExecResult {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// Format for the model: stdout first, stderr and exit code only when relevant.
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.timed_out {
            out.push_str("[timed out]\n");
        }
        if !self.stdout.trim().is_empty() {
            out.push_str(self.stdout.trim_end());
            out.push('\n');
        }
        if !self.stderr.trim().is_empty() {
            out.push_str("--- stderr ---\n");
            out.push_str(self.stderr.trim_end());
            out.push('\n');
        }
        if self.code != Some(0) {
            out.push_str(&format!("[exit code: {}]\n", self.code.map(|c| c.to_string()).unwrap_or("signal".into())));
        }
        if out.is_empty() {
            out.push_str("(no output, exit code 0)");
        }
        out
    }
}

pub struct Exec<'a> {
    argv: Vec<String>,
    privileged: bool,
    stdin: Option<String>,
    cwd: Option<String>,
    ctx: &'a ToolContext,
}

pub fn exec<'a, S: AsRef<str>>(ctx: &'a ToolContext, argv: &[S]) -> Exec<'a> {
    Exec {
        argv: argv.iter().map(|s| s.as_ref().to_string()).collect(),
        privileged: false,
        stdin: None,
        cwd: None,
        ctx,
    }
}

impl Exec<'_> {
    pub fn privileged(mut self, yes: bool) -> Self {
        self.privileged = yes;
        self
    }
    pub fn stdin(mut self, data: String) -> Self {
        self.stdin = Some(data);
        self
    }
    pub fn cwd(mut self, dir: Option<String>) -> Self {
        self.cwd = dir;
        self
    }

    pub async fn run(self) -> anyhow::Result<ExecResult> {
        let mut argv = self.argv;
        if argv.is_empty() {
            bail!("empty command");
        }
        if self.privileged {
            let prefix: &[&str] = match self.ctx.privilege {
                // -n: never prompt on a terminal nobody is watching; fail fast instead.
                PrivilegeMethod::Sudo => &["sudo", "-n", "--"],
                PrivilegeMethod::Pkexec => &["pkexec", "--disable-internal-agent"],
                PrivilegeMethod::None => bail!(
                    "privileged execution is disabled (Settings -> privilege method = none)"
                ),
            };
            let mut full: Vec<String> = prefix.iter().map(|s| s.to_string()).collect();
            full.append(&mut argv);
            argv = full;
        }
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("LC_ALL", "C.UTF-8")
            .env("SYSTEMD_PAGER", "")
            .env("PAGER", "cat")
            .kill_on_drop(true);
        // Own process group so cancellation can kill the whole tree (S6 kill switch).
        cmd.process_group(0);
        if let Some(dir) = &self.cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().with_context(|| format!("failed to start {}", argv[0]))?;
        let pid = child.id();
        let _running = pid.map(RunningGuard::new);
        if let Some(data) = self.stdin {
            if let Some(mut si) = child.stdin.take() {
                si.write_all(data.as_bytes()).await?;
            }
        }
        let limit = self.ctx.max_output_bytes * 4;
        let wait = child.wait_with_output();
        tokio::select! {
            out = wait => {
                let out = out?;
                Ok(ExecResult {
                    code: out.status.code(),
                    stdout: lossy_capped(&out.stdout, limit),
                    stderr: lossy_capped(&out.stderr, limit),
                    timed_out: false,
                })
            }
            _ = tokio::time::sleep(self.ctx.timeout) => {
                if let Some(pid) = pid {
                    kill_group(pid, "KILL");
                }
                Ok(ExecResult { timed_out: true, ..Default::default() })
            }
            _ = self.ctx.cancel.cancelled() => {
                if let Some(pid) = pid {
                    kill_group(pid, "TERM");
                }
                bail!("cancelled")
            }
        }
    }
}

/// Process groups of tool children currently running (for the kill switch report).
static RUNNING: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

struct RunningGuard(u32);

impl RunningGuard {
    fn new(pid: u32) -> Self {
        RUNNING.lock().unwrap().push(pid);
        Self(pid)
    }
}

impl Drop for RunningGuard {
    fn drop(&mut self) {
        RUNNING.lock().unwrap().retain(|p| *p != self.0);
    }
}

/// Number of tool child process groups alive right now.
pub fn running_process_groups() -> usize {
    RUNNING.lock().unwrap().len()
}

/// Best-effort process-group kill: TERM first, KILL after 2 s (kill switch, S6).
fn kill_group(pid: u32, signal: &str) {
    let pgid = format!("-{pid}");
    let _ = std::process::Command::new("kill")
        .args([format!("-{signal}").as_str(), pgid.as_str()])
        .status();
    if signal == "TERM" {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = std::process::Command::new("kill").args(["-KILL", pgid.as_str()]).status();
        });
    }
}

fn lossy_capped(bytes: &[u8], limit: usize) -> String {
    let slice = if bytes.len() > limit { &bytes[bytes.len() - limit..] } else { bytes };
    String::from_utf8_lossy(slice).into_owned()
}

/// Program available on PATH?
pub fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::ToolContext;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn cancel_kills_process_group() {
        let cancel = CancellationToken::new();
        let mut ctx = ToolContext::standalone(Duration::from_secs(60), 24_000);
        ctx.cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            // Child in its own group; the whole tree must die on cancel.
            exec(&ctx, &["bash".to_string(), "-c".into(), "sleep 60 & sleep 60".into()])
                .run()
                .await
        });
        tokio::time::sleep(Duration::from_millis(700)).await; // let children spawn
        let before = pgrep_sleep_count();
        assert!(before >= 1, "sleep children should be running");
        cancel.cancel();
        let _ = handle.await;
        // TERM (then KILL after 2 s) must reap the group quickly.
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while pgrep_sleep_count() > 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert_eq!(pgrep_sleep_count(), 0, "process group survived cancellation");
    }

    fn pgrep_sleep_count() -> usize {
        std::process::Command::new("pgrep")
            .args(["-c", "-f", "^sleep 60$"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0))
            .unwrap_or(0)
    }
}
