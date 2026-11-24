//! The only side-effect boundary of the helper. Validation and planning in `ops.rs` go
//! through this trait, so they are unit-tested with `FakeSystem` and never need root.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone)]
pub struct RunOutput {
    pub exit: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl RunOutput {
    pub fn ok(&self) -> bool {
        self.exit == Some(0) && !self.timed_out
    }
}

pub trait System {
    fn read(&self, path: &Path) -> Option<String>;
    fn exists(&self, path: &Path) -> bool;
    /// Canonical path; `None` when it does not exist.
    fn canonicalize(&self, path: &Path) -> Option<PathBuf>;
    fn is_symlink(&self, path: &Path) -> bool;
    fn write(&mut self, path: &Path, content: &str, mode: u32) -> std::io::Result<()>;
    fn remove(&mut self, path: &Path) -> std::io::Result<()>;
    fn run(&mut self, argv: &[String], timeout: Duration) -> RunOutput;
    fn kill(&mut self, pid: i32, signal: &str) -> Result<(), String>;
    /// Home directory of the user who invoked sudo/pkexec.
    fn caller_home(&self) -> Option<PathBuf>;
    fn now_stamp(&self) -> String;
}

const OUTPUT_CAP: usize = 64 * 1024;

fn cap_tail(bytes: Vec<u8>) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_CAP);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// The real system. Children get a cleared environment and absolute argv[0].
pub struct RealSystem;

impl System for RealSystem {
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }
    fn is_symlink(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false)
    }
    fn write(&mut self, path: &Path, content: &str, mode: u32) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("kagent-tmp");
        std::fs::write(&tmp, content)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, path)
    }
    fn remove(&mut self, path: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
    fn run(&mut self, argv: &[String], timeout: Duration) -> RunOutput {
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .env_clear()
            .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
            .env("LC_ALL", "C.UTF-8")
            .env("DEBIAN_FRONTEND", "noninteractive")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return RunOutput { exit: None, stderr: format!("spawn {}: {e}", argv[0]), ..Default::default() },
        };
        let mut out = child.stdout.take().expect("piped");
        let mut err = child.stderr.take().expect("piped");
        let t_out = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            b
        });
        let t_err = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b);
            b
        });
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    timed_out = true;
                    break child.wait().ok();
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => break None,
            }
        };
        RunOutput {
            exit: status.and_then(|s| s.code()),
            stdout: cap_tail(t_out.join().unwrap_or_default()),
            stderr: cap_tail(t_err.join().unwrap_or_default()),
            timed_out,
        }
    }
    fn kill(&mut self, pid: i32, signal: &str) -> Result<(), String> {
        let out = self.run(&["/bin/kill".into(), format!("-{signal}"), pid.to_string()], Duration::from_secs(2));
        if out.ok() { Ok(()) } else { Err(out.stderr.trim().to_string()) }
    }
    fn caller_home(&self) -> Option<PathBuf> {
        let uid = std::env::var("SUDO_UID").or_else(|_| std::env::var("PKEXEC_UID")).ok()?;
        // /etc/passwd: name:x:uid:gid:gecos:home:shell. Never trust $HOME under sudo.
        std::fs::read_to_string("/etc/passwd").ok()?.lines().find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 7 && f[2] == uid).then(|| PathBuf::from(f[5]))
        })
    }
    fn now_stamp(&self) -> String {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        secs.to_string()
    }
}
