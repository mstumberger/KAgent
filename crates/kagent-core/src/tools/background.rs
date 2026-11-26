//! Background shell tasks (agents.md section 4, tools.md section 23): `shell` with
//! `background: true` starts the command in its own process group and answers at once. Its
//! output goes to `<data>/task-out/<taskId>.log` (the last 1 MiB), `task_output` reads it,
//! `task_stop` and the kill switch kill the group, and the session that started the task
//! is told when it ended, as a subagent's parent is told.
//!
//! One registry serves the daemon (`Background::shared()`); a test makes its own.

use std::collections::{HashMap, VecDeque};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::Command;

mod shutdown;

use super::exec::kill_group;

/// At most this many tasks run at once; a further one is refused (agents.md section 4).
pub const MAX_RUNNING: usize = 20;
/// The output file keeps its last 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The file may grow this far beyond the cap before it is cut back (each cut rewrites it).
const SLACK_BYTES: u64 = MAX_FILE_BYTES;
/// Ended tasks kept in memory, the latest ones (their files stay on disk).
const KEPT_ENDED: usize = 200;
/// The output a completion notification carries (as a subagent's does).
pub const MAX_OUTPUT_CHARS: usize = 4_000;
/// How long a task may run: the specs name no limit, so it is a long one.
const MAX_RUNTIME: Duration = Duration::from_secs(86_400 * 30);

/// `running`, `done` (exit 0), `failed` (another exit code), `killed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
    Killed,
}

impl TaskStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }
}

/// What `task_output` knows about a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub id: String,
    pub status: TaskStatus,
    pub exit_code: Option<i32>,
    /// Seconds since it started.
    pub seconds: u64,
    pub output_file: String,
}

impl TaskInfo {
    /// The first line of what `task_output` says about the task.
    pub fn head(&self) -> String {
        match self.status {
            TaskStatus::Running => format!("[Task {} running for {} s]", self.id, self.seconds),
            TaskStatus::Killed => format!("[Task {} killed, after {} s]", self.id, self.seconds),
            other => format!("[Task {} {}, exit {}, after {} s]", self.id, other.label(), self.exit_text(), self.seconds),
        }
    }

    fn exit_text(&self) -> String {
        self.exit_code.map_or_else(|| "signal".into(), |c| c.to_string())
    }

    /// The message the starting session gets when the task ended (agents.md section 4).
    pub fn notice(&self, tail: &str) -> String {
        let mut text = format!("[Task {} finished, exit {}]\n", self.id, self.exit_text());
        if !tail.trim().is_empty() {
            text.push_str(tail.trim_end());
            text.push('\n');
        }
        text
    }
}

/// Who tells a session something (the agent stores it and may start a turn).
pub type Sink = Arc<dyn Fn(String, String) + Send + Sync>;

struct State {
    status: TaskStatus,
    started: Instant,
    pid: Option<u32>,
    exit_code: Option<i32>,
    /// The task holds one of the `MAX_RUNNING` slots.
    counted: bool,
}

impl Inner {
    /// Give back the slot a task took (once).
    fn release(&self, state: &mut State) {
        if std::mem::take(&mut state.counted) {
            self.running.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

struct Task {
    id: String,
    session: String,
    file: PathBuf,
    /// Opened for appending: a cut back to the cap leaves it at the end.
    out: tokio::sync::Mutex<tokio::fs::File>,
    state: Mutex<State>,
}

impl Task {
    fn info(&self, state: &State) -> TaskInfo {
        TaskInfo { id: self.id.clone(), status: state.status, exit_code: state.exit_code, seconds: state.started.elapsed().as_secs(), output_file: self.file.display().to_string() }
    }

    /// Add output to the file; past the cap it is cut back to its last 1 MiB.
    async fn append(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut file = self.out.lock().await;
        file.write_all(bytes).await?;
        if file.metadata().await?.len() > MAX_FILE_BYTES + SLACK_BYTES {
            cut_back(&mut file, &self.file).await?;
        }
        Ok(())
    }

    /// The last `chars` characters of the output.
    async fn tail(&self, chars: usize) -> String {
        let Ok(mut file) = tokio::fs::File::open(&self.file).await else { return String::new() };
        let len = file.metadata().await.map_or(0, |m| m.len());
        let want = (chars as u64 * 4).min(len);
        if file.seek(SeekFrom::End(-(want as i64))).await.is_err() {
            return String::new();
        }
        let mut bytes = Vec::with_capacity(want as usize + 4);
        if file.read_to_end(&mut bytes).await.is_err() {
            return String::new();
        }
        // The read may begin inside a character: that one is not shown.
        let text = String::from_utf8_lossy(&bytes);
        let text = text.trim_start_matches('\u{FFFD}');
        let start = text.char_indices().rev().nth(chars.saturating_sub(1)).map_or(0, |(i, _)| i);
        text[start..].to_string()
    }
}

/// Keep the last 1 MiB of the file; a whole first line when one starts near the cut.
async fn cut_back(file: &mut tokio::fs::File, path: &Path) -> std::io::Result<()> {
    let bytes = tokio::fs::read(path).await?;
    let kept = &bytes[bytes.len().saturating_sub(MAX_FILE_BYTES as usize)..];
    let skip = kept.iter().position(|b| *b == b'\n').filter(|i| *i < 4096).map_or(0, |i| i + 1);
    file.set_len(0).await?;
    file.write_all(&kept[skip..]).await
}

struct Inner {
    dir: Mutex<Option<PathBuf>>,
    sink: Mutex<Option<Sink>>,
    tasks: Mutex<HashMap<String, Arc<Task>>>,
    /// Ended tasks, oldest first: the memory of them is kept to the latest `KEPT_ENDED`.
    ended: Mutex<VecDeque<String>>,
    /// Started and not yet ended; a slot is reserved before the command is spawned.
    running: AtomicUsize,
}

/// The background tasks of this process.
#[derive(Clone)]
pub struct Background(Arc<Inner>);

impl Background {
    /// A registry of its own, writing into `dir` (tests).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self(Arc::new(Inner { dir: Mutex::new(Some(dir.into())), sink: Mutex::new(None), tasks: Mutex::new(HashMap::new()), ended: Mutex::new(VecDeque::new()), running: AtomicUsize::new(0) }))
    }

    /// The registry the daemon's `shell` tool uses.
    pub fn shared() -> Self {
        static SHARED: OnceLock<Background> = OnceLock::new();
        SHARED.get_or_init(|| Self(Arc::new(Inner { dir: Mutex::new(None), sink: Mutex::new(None), tasks: Mutex::new(HashMap::new()), ended: Mutex::new(VecDeque::new()), running: AtomicUsize::new(0) }))).clone()
    }

    /// Where the output files go (`<data>/task-out`), and who tells a session that a task
    /// ended. The daemon sets both once its agent exists.
    pub fn configure(&self, dir: impl Into<PathBuf>, sink: Sink) {
        *self.0.dir.lock().unwrap() = Some(dir.into());
        *self.0.sink.lock().unwrap() = Some(sink);
    }

    /// Start `command` for `session` in its own process group and answer at once with its
    /// id and output file. Nothing starts (and the answer says why) when `MAX_RUNNING`
    /// tasks run already, when no output folder is set, or when the command cannot run.
    pub async fn start(&self, session: &str, command: &str, cwd: Option<String>) -> Result<TaskInfo, String> {
        let Some(dir) = self.0.dir.lock().unwrap().clone() else {
            return Err("background tasks are not available: their output folder is not set".into());
        };
        let reserved = self.0.running.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < MAX_RUNNING).then_some(n + 1));
        if reserved.is_err() {
            return Err(format!("too many background tasks: {MAX_RUNNING} are running. Stop one with task_stop, or wait for one to end"));
        }
        let (task, child) = match spawn(&dir, session, command, cwd).await {
            Ok(started) => started,
            Err(e) => {
                self.0.running.fetch_sub(1, Ordering::SeqCst);
                return Err(format!("the background task could not start: {e}"));
            }
        };
        let info = task.info(&task.state.lock().unwrap());
        self.0.tasks.lock().unwrap().insert(task.id.clone(), task.clone());
        let sink = self.0.sink.lock().unwrap().clone();
        let inner = self.0.clone();
        tokio::spawn(async move { supervise(inner, task, child, sink).await });
        Ok(info)
    }

    /// What a task is now (no output read).
    pub fn info(&self, id: &str) -> Option<TaskInfo> {
        let task = self.0.tasks.lock().unwrap().get(id).cloned()?;
        let info = task.info(&task.state.lock().unwrap());
        Some(info)
    }

    /// A task by id: what it is and the last `tail` characters of its output.
    pub async fn output(&self, id: &str, tail: usize) -> Option<(TaskInfo, String)> {
        let task = self.0.tasks.lock().unwrap().get(id).cloned()?;
        let info = task.info(&task.state.lock().unwrap());
        Some((info, task.tail(tail).await))
    }

    pub fn running(&self) -> usize {
        self.0.running.load(Ordering::SeqCst)
    }

    /// `task_stop`: TERM to the process group, KILL after 2 s. False when it is not running.
    pub fn stop(&self, id: &str) -> bool {
        let Some(task) = self.0.tasks.lock().unwrap().get(id).cloned() else { return false };
        let mut state = task.state.lock().unwrap();
        if state.status != TaskStatus::Running {
            return false;
        }
        state.status = TaskStatus::Killed;
        if let Some(pid) = state.pid.take() {
            // The path of a cancelled tool call (S6): TERM, then KILL after 2 s.
            kill_group(pid, "TERM");
        }
        // Stopped: its slot is free at once, whatever its pipes still do.
        self.0.release(&mut state);
        true
    }

    /// The kill switch: every running task. How many.
    pub fn stop_all(&self) -> usize {
        let running: Vec<String> = self
            .0
            .tasks
            .lock()
            .unwrap()
            .values()
            .filter(|t| t.state.lock().unwrap().status == TaskStatus::Running)
            .map(|t| t.id.clone())
            .collect();
        running.iter().filter(|id| self.stop(id)).count()
    }

}

/// Spawn the command and make its record. The caller enters it in the registry.
async fn spawn(dir: &Path, session: &str, command: &str, cwd: Option<String>) -> std::io::Result<(Arc<Task>, tokio::process::Child)> {
    std::fs::create_dir_all(dir)?;
    let id = format!("bg_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let file = dir.join(format!("{id}.log"));
    let out = tokio::fs::OpenOptions::new().create(true).append(true).open(&file).await?;
    // As `exec` starts a tool's command: own process group, reaped with the daemon.
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C.UTF-8")
        .env("SYSTEMD_PAGER", "")
        .env("PAGER", "cat")
        .kill_on_drop(true)
        .process_group(0);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let child = cmd.spawn()?;
    let task = Arc::new(Task {
        id,
        session: session.to_string(),
        file,
        out: tokio::sync::Mutex::new(out),
        state: Mutex::new(State { status: TaskStatus::Running, started: Instant::now(), pid: child.id(), exit_code: None, counted: true }),
    });
    Ok((task, child))
}

/// Read the output of the pipe into the file until the command's side of it closes.
fn copy<P: AsyncRead + Unpin + Send + 'static>(task: Arc<Task>, mut pipe: Option<P>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 16 * 1024];
        while let Some(reader) = pipe.as_mut() {
            match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if task.append(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// Read the output of the task into its file until the command ends, then record how.
async fn supervise(inner: Arc<Inner>, task: Arc<Task>, mut child: tokio::process::Child, sink: Option<Sink>) {
    let readers = (copy(task.clone(), child.stdout.take()), copy(task.clone(), child.stderr.take()));
    // A task that outruns the limit is stopped like one the owner stops.
    let status = match tokio::time::timeout(MAX_RUNTIME, child.wait()).await {
        Ok(status) => status.ok(),
        Err(_) => {
            if let Some(pid) = task.state.lock().unwrap().pid {
                kill_group(pid, "TERM");
            }
            tokio::time::timeout(Duration::from_secs(3), child.wait()).await.ok().and_then(Result::ok)
        }
    };
    let _ = readers.0.await;
    let _ = readers.1.await;
    // The file is final before the task is said to have ended: a reader that sees the
    // new state finds the whole output, never a file cut back to nothing for a moment.
    {
        let mut file = task.out.lock().await;
        let _ = file.flush().await;
        if file.metadata().await.is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
            let _ = cut_back(&mut file, &task.file).await;
        }
    }
    let code = status.and_then(|s| s.code());
    let info = {
        let mut state = task.state.lock().unwrap();
        state.pid = None;
        state.exit_code = code;
        if state.status == TaskStatus::Running {
            state.status = if code == Some(0) { TaskStatus::Done } else { TaskStatus::Failed };
        }
        inner.release(&mut state);
        task.info(&state)
    };
    {
        let mut ended = inner.ended.lock().unwrap();
        ended.push_back(task.id.clone());
        while ended.len() > KEPT_ENDED {
            if let Some(old) = ended.pop_front() {
                inner.tasks.lock().unwrap().remove(&old);
            }
        }
    }
    // A killed task tells nobody: it was stopped on purpose, as a cancelled subagent.
    if info.status != TaskStatus::Killed {
        if let Some(sink) = sink {
            let tail = task.tail(MAX_OUTPUT_CHARS).await;
            sink(task.session.clone(), info.notice(&tail));
        }
    }
}

#[cfg(test)]
mod tests;
