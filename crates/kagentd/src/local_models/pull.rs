//! Downloads through Ollama (local-models.md section 3): only ids of the shipped list, a disk
//! check first, one pull at a time with the others queued, progress as `localModelsChanged`
//! (at most twice a second), cancel, and a failed pull that keeps its error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use kagent_core::protocol::AgentEvent;

use super::{ollama, Disk, LocalModels, PullRow, PullState};

/// Free space that must stay after the download (local-models.md section 3).
pub const SPARE_MB: u64 = 2048;
const EVENT_EVERY: Duration = Duration::from_millis(500);
const MB: u64 = 1024 * 1024;

/// What a refused pull is, and the status it gets.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// 400: not an id of the list, or a model that cannot be downloaded.
    Invalid(String),
    /// 409: the state of the machine says no (installed, queued, disk, Ollama down).
    Conflict(String),
    /// 404: nothing to cancel or remove.
    Missing(String),
    /// 502: Ollama answered with an error.
    Upstream(String),
}

pub type Outcome<T> = Result<T, Refusal>;

/// Where Ollama keeps its models: `OLLAMA_MODELS`, else the user's folder, else the one of the
/// system service. The first that exists; else the home folder (the same disk, usually).
pub fn models_dir(env: Option<&str>, home: Option<&str>, exists: impl Fn(&str) -> bool) -> PathBuf {
    let mut candidates: Vec<String> = Vec::new();
    candidates.extend(env.filter(|e| !e.is_empty()).map(String::from));
    candidates.extend(home.map(|h| format!("{h}/.ollama/models")));
    candidates.push("/usr/share/ollama/.ollama/models".into());
    candidates.iter().find(|c| exists(c)).cloned().or_else(|| home.map(String::from)).unwrap_or_else(|| "/".into()).into()
}

/// `df -Pk <dir>`: the free space in MB (the "Available" column of the last line).
pub fn parse_df_free_mb(out: &str) -> Option<u64> {
    let kb: u64 = out.lines().rev().find(|l| !l.trim().is_empty())?.split_whitespace().nth(3)?.parse().ok()?;
    Some(kb / 1024)
}

/// Whether Ollama is on this machine: only then is its model folder on a disk we can look at.
pub fn is_local(base: &str) -> bool {
    ["//127.0.0.1", "//localhost", "//[::1]"].iter().any(|h| base.contains(h))
}

/// The bytes of a stream become lines; a line that is cut stays until the rest arrives.
#[derive(Default)]
pub struct Lines(Vec<u8>);

impl Lines {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.0.extend_from_slice(chunk);
        let mut out = Vec::new();
        while let Some(i) = self.0.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.0.drain(..=i).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1]).trim().to_string();
            if !text.is_empty() {
                out.push(text);
            }
        }
        out
    }
}

/// Progress over every layer seen so far: Ollama reports a total per layer, and new layers
/// appear as the pull goes on.
#[derive(Default)]
pub struct Progress {
    layers: HashMap<String, (u64, u64)>,
}

/// What one line of the stream says.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Progress,
    Success,
    Error(String),
    Status,
}

impl Progress {
    pub fn read(&mut self, line: &str) -> Line {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Line::Status };
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Line::Error(e.to_string());
        }
        if v.get("status").and_then(Value::as_str) == Some("success") {
            return Line::Success;
        }
        if let (Some(d), Some(total)) = (v.get("digest").and_then(Value::as_str), v.get("total").and_then(Value::as_u64)) {
            let done = v.get("completed").and_then(Value::as_u64).unwrap_or(0);
            self.layers.insert(d.to_string(), (total, done.min(total)));
            return Line::Progress;
        }
        Line::Status
    }

    pub fn done_total_mb(&self) -> (u64, u64) {
        let (t, d) = self.layers.values().fold((0, 0), |(t, d), (lt, ld)| (t + lt, d + ld));
        (d / MB, t / MB)
    }
}

impl LocalModels {
    async fn free_mb(&self) -> Option<u64> {
        match &self.disk {
            Disk::Fixed(v) => *v,
            Disk::Probe => {
                let home = std::env::var("HOME").ok();
                let dir = models_dir(std::env::var("OLLAMA_MODELS").ok().as_deref(), home.as_deref(), |p| std::path::Path::new(p).exists());
                let out = tokio::process::Command::new("df").arg("-Pk").arg(&dir).output().await.ok()?;
                parse_df_free_mb(&String::from_utf8_lossy(&out.stdout))
            }
        }
    }

    /// `POST /api/local-models/pull {id}`.
    pub async fn pull(self: &Arc<Self>, id: &str) -> Outcome<()> {
        let entry = self.entry(id).ok_or_else(|| Refusal::Invalid(format!("{id} is not on the list of recommended models: KAgent downloads only those (Settings -> Local models). Add other models in Ollama itself, then Fetch models in Settings -> Models.")))?.clone();
        if !entry.downloadable() {
            return Err(Refusal::Invalid(format!("{} cannot be downloaded: KAgent cannot use a local speech model yet", entry.name)));
        }
        {
            let p = self.pulls.lock().unwrap();
            if let Some(row) = p.rows.iter().find(|r| r.id == id && r.state != PullState::Failed) {
                return Err(Refusal::Conflict(format!("{id} is already {}", if row.state == PullState::Queued { "waiting in the queue" } else { "being downloaded" })));
            }
        }
        let base = self.base();
        let installed = ollama::tags(&base).await.map_err(|_| Refusal::Conflict(format!("Ollama is not reachable at {base}: start it (KAgent does not install Ollama), then try again")))?;
        if let Some(have) = installed.iter().find(|i| ollama::same_id(id, &i.id)) {
            if self.model_ref(&have.id).is_some() {
                return Err(Refusal::Conflict(format!("{id} is already installed")));
            }
            // Downloaded outside KAgent and never fetched: only the Models row is missing.
            self.register(id).await.map_err(Refusal::Upstream)?;
            self.emit_pulls();
            return Ok(());
        }
        if is_local(&base) {
            if let Some(free) = self.free_mb().await {
                let need = entry.size_mb + SPARE_MB;
                if free < need {
                    return Err(Refusal::Conflict(format!("not enough disk space for {id}: {free} MB are free where Ollama keeps its models and {need} MB are needed ({} MB for the model and {SPARE_MB} MB to spare). Free some space, then try again.", entry.size_mb)));
                }
            }
        }
        {
            let mut p = self.pulls.lock().unwrap();
            p.rows.retain(|r| r.id != id);
            p.rows.push(PullRow { id: id.into(), state: PullState::Queued, done_mb: 0, total_mb: entry.size_mb, error: None });
        }
        self.emit_pulls();
        self.kick();
        Ok(())
    }

    /// Start the worker unless one is going through the queue.
    fn kick(self: &Arc<Self>) {
        {
            let mut p = self.pulls.lock().unwrap();
            if p.running {
                return;
            }
            p.running = true;
        }
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                let next = {
                    let mut p = me.pulls.lock().unwrap();
                    let next = p.rows.iter_mut().find(|r| r.state == PullState::Queued).map(|r| {
                        r.state = PullState::Pulling;
                        r.id.clone()
                    });
                    if next.is_none() {
                        p.running = false;
                    }
                    next
                };
                let Some(id) = next else { break };
                me.emit_pulls();
                me.run(&id).await;
            }
        });
    }

    fn fail(&self, id: &str, why: String) {
        tracing::warn!("local model {id}: {why}");
        let mut p = self.pulls.lock().unwrap();
        p.cancel.remove(id);
        if let Some(r) = p.rows.iter_mut().find(|r| r.id == id) {
            r.state = PullState::Failed;
            r.error = Some(why);
        }
        drop(p);
        self.emit_pulls();
    }

    fn forget(&self, id: &str) {
        let mut p = self.pulls.lock().unwrap();
        p.cancel.remove(id);
        p.rows.retain(|r| r.id != id);
        drop(p);
        self.emit_pulls();
    }

    fn set_progress(&self, id: &str, done: u64, total: u64) {
        let mut p = self.pulls.lock().unwrap();
        if let Some(r) = p.rows.iter_mut().find(|r| r.id == id) {
            r.done_mb = done;
            if total > 0 {
                r.total_mb = total;
            }
        }
    }

    /// One pull: the stream of Ollama's `POST /api/pull` until `success`, an error, the end of
    /// the stream or a cancel.
    async fn run(self: &Arc<Self>, id: &str) {
        let token = CancellationToken::new();
        self.pulls.lock().unwrap().cancel.insert(id.to_string(), token.clone());
        let base = self.base();
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).build().unwrap_or_default();
        let sent = http.post(format!("{base}/api/pull")).json(&json!({"model": id, "stream": true})).send();
        let resp = tokio::select! {
            _ = token.cancelled() => return self.forget(id),
            r = sent => r,
        };
        let resp = match resp {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                let status = r.status();
                let body = r.text().await.unwrap_or_default();
                return self.fail(id, format!("Ollama answered {status}: {}", body.trim()));
            }
            Err(e) => return self.fail(id, format!("Ollama is not reachable at {base}: {e}")),
        };
        let mut stream = resp.bytes_stream();
        let (mut lines, mut progress) = (Lines::default(), Progress::default());
        let mut last = Instant::now() - EVENT_EVERY;
        loop {
            let chunk = tokio::select! {
                _ = token.cancelled() => return self.forget(id),
                c = stream.next() => c,
            };
            let Some(chunk) = chunk else { return self.fail(id, "Ollama closed the connection before the download finished; press Retry to resume".into()) };
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => return self.fail(id, format!("the download broke off: {e}; press Retry to resume")),
            };
            for line in lines.push(&chunk) {
                match progress.read(&line) {
                    Line::Error(e) => return self.fail(id, e),
                    Line::Success => {
                        self.forget(id);
                        if let Err(e) = self.register(id).await {
                            tracing::warn!("local model {id} downloaded but not added to Models: {e}");
                            let _ = self.events.send(AgentEvent::notice("warn", format!("{id} is downloaded, but adding it to Models failed: {e}. Press Fetch models in Settings -> Models.")));
                        }
                        self.emit_pulls();
                        return;
                    }
                    Line::Progress => {
                        let (d, t) = progress.done_total_mb();
                        self.set_progress(id, d, t);
                        if last.elapsed() >= EVENT_EVERY {
                            last = Instant::now();
                            self.emit_pulls();
                        }
                    }
                    Line::Status => {}
                }
            }
        }
    }

    /// `DELETE /api/local-models/pull/:id`: stop the running pull (Ollama keeps the partial
    /// layers, a later pull resumes), leave the queue, or dismiss a failed row.
    pub fn cancel(&self, id: &str) -> Outcome<()> {
        let (token, found) = {
            let p = self.pulls.lock().unwrap();
            (p.cancel.get(id).cloned(), p.rows.iter().any(|r| r.id == id))
        };
        if !found {
            return Err(Refusal::Missing(format!("{id} is not being downloaded or waiting: nothing to cancel")));
        }
        match token {
            Some(t) => t.cancel(),
            None => self.forget(id),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_is_cut_into_lines_wherever_the_chunks_end() {
        let mut l = Lines::default();
        assert_eq!(l.push(b"{\"status\":\"pulling man"), Vec::<String>::new());
        assert_eq!(l.push(b"ifest\"}\n{\"a\":1}\n{\"b\""), vec!["{\"status\":\"pulling manifest\"}", "{\"a\":1}"]);
        assert_eq!(l.push(b":2}\n\n"), vec!["{\"b\":2}"]);
    }

    #[test]
    fn progress_adds_up_the_layers() {
        let mut p = Progress::default();
        assert_eq!(p.read(r#"{"status":"pulling manifest"}"#), Line::Status);
        assert_eq!(p.read(r#"{"status":"pulling aaa","digest":"sha256:aaa","total":3145728000,"completed":1048576000}"#), Line::Progress);
        assert_eq!(p.read(r#"{"status":"pulling bbb","digest":"sha256:bbb","total":1048576,"completed":0}"#), Line::Progress);
        assert_eq!(p.done_total_mb(), (1000, 3001));
        p.read(r#"{"status":"pulling aaa","digest":"sha256:aaa","total":3145728000,"completed":3145728000}"#);
        assert_eq!(p.done_total_mb(), (3000, 3001));
        assert_eq!(p.read(r#"{"status":"success"}"#), Line::Success);
        assert_eq!(p.read(r#"{"error":"pull model manifest: file does not exist"}"#), Line::Error("pull model manifest: file does not exist".into()));
        assert_eq!(p.read("not json"), Line::Status);
    }

    #[test]
    fn ollamas_model_folder() {
        let has = |set: &'static [&'static str]| move |p: &str| set.contains(&p);
        assert_eq!(models_dir(Some("/data/models"), Some("/home/a"), has(&["/data/models", "/home/a/.ollama/models"])), PathBuf::from("/data/models"));
        assert_eq!(models_dir(None, Some("/home/a"), has(&["/home/a/.ollama/models"])), PathBuf::from("/home/a/.ollama/models"));
        assert_eq!(models_dir(None, Some("/home/a"), has(&["/usr/share/ollama/.ollama/models"])), PathBuf::from("/usr/share/ollama/.ollama/models"));
        assert_eq!(models_dir(Some(""), Some("/home/a"), has(&[])), PathBuf::from("/home/a"), "nothing exists yet: the home folder's disk");
    }

    // Recorded from `df -Pk /home` on the owner's machine (GNU coreutils, 2026-11-28).
    #[test]
    fn free_space_from_df() {
        let out = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/nvme0n1p2   960155124 856875876  61704900      94% /\n";
        assert_eq!(parse_df_free_mb(out), Some(60258));
        assert_eq!(parse_df_free_mb(""), None);
        assert_eq!(parse_df_free_mb("df: nope: No such file or directory\n"), None);
    }

    #[test]
    fn ollama_on_this_machine() {
        assert!(is_local("http://127.0.0.1:11434") && is_local("http://localhost:11434"));
        assert!(!is_local("http://10.0.0.5:11434"));
    }
}
