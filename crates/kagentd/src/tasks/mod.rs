//! Scheduled tasks and the task list (docs/specs/tasks.md). A task is either a reminder
//! (a notification) or an agent job (a prompt KAgent runs unattended in its own session
//! and reports on). Tasks without a schedule are to-dos run on demand. Stored in
//! `<data>/tasks.json`; a runner checks every 5 s. The `tasks` tool and the Tasks page
//! use the same store; unattended runs follow the current permission mode (in `ask`,
//! approval cards wait in the UI).

pub mod schedule;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local};
use kagent_core::protocol::{AgentEvent, Role, Via};
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use schedule::Schedule;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    /// What KAgent should do (agent job), or the reminder text.
    pub what: String,
    /// `agent` (run `what` as a prompt) or `remind` (show `what`).
    pub kind: String,
    #[serde(default)]
    pub schedule: Option<Schedule>,
    pub enabled: bool,
    pub created_at: DateTime<Local>,
    #[serde(default)]
    pub next_run: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_run: Option<DateTime<Local>>,
    /// `running` | `ok` | `failed`.
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default)]
    pub last_session: Option<String>,
    #[serde(default)]
    pub runs: u32,
    /// A one-off that ran, or a to-do marked done.
    #[serde(default)]
    pub done: bool,
    /// "Run now" pressed: runs once even when paused or a to-do.
    #[serde(default)]
    pub run_requested: bool,
}

impl Task {
    pub fn when(&self) -> String {
        match &self.schedule {
            Some(s) => s.describe(),
            None => "to-do (run on demand)".into(),
        }
    }
}

pub struct TaskStore {
    path: PathBuf,
    tasks: Mutex<Vec<Task>>,
    events: broadcast::Sender<AgentEvent>,
}

impl TaskStore {
    pub fn load(data: &std::path::Path, events: broadcast::Sender<AgentEvent>) -> Arc<Self> {
        let path = data.join("tasks.json");
        let tasks: Vec<Task> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Arc::new(Self { path, tasks: Mutex::new(tasks), events })
    }

    fn save(&self, tasks: &[Task]) {
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(tasks).unwrap_or_default()).and_then(|_| std::fs::rename(&tmp, &self.path)).is_err() {
            tracing::warn!("could not save {}", self.path.display());
        }
        let _ = self.events.send(AgentEvent::Notice { level: "tasks".into(), text: "tasks changed".into() });
    }

    pub fn list(&self) -> Vec<Task> {
        self.tasks.lock().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks.lock().unwrap().iter().find(|t| t.id == id || t.id.starts_with(id)).cloned()
    }

    /// Add from text inputs (`when` in schedule.rs syntax).
    pub fn add(&self, title: &str, what: &str, kind: &str, when: &str) -> Result<Task, String> {
        let what = what.trim();
        if what.is_empty() {
            return Err("say what to do (or the reminder text)".into());
        }
        let kind = if kind == "remind" { "remind" } else { "agent" };
        let now = Local::now();
        let schedule = schedule::parse(when, now)?;
        let title = if title.trim().is_empty() { what.chars().take(60).collect::<String>() } else { title.trim().to_string() };
        let t = Task {
            id: format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            title,
            what: what.to_string(),
            kind: kind.into(),
            next_run: schedule.as_ref().and_then(|s| s.next_after(now)),
            schedule,
            enabled: true,
            created_at: now,
            last_run: None,
            last_status: None,
            last_result: None,
            last_session: None,
            runs: 0,
            done: false,
            run_requested: false,
        };
        let mut all = self.tasks.lock().unwrap();
        all.push(t.clone());
        self.save(&all);
        Ok(t)
    }

    /// Change fields; `when` re-parses the schedule.
    pub fn update(&self, id: &str, patch: &Value) -> Result<Task, String> {
        let mut all = self.tasks.lock().unwrap();
        let t = all.iter_mut().find(|t| t.id == id || t.id.starts_with(id)).ok_or_else(|| format!("no task {id}"))?;
        let s = |k: &str| patch.get(k).and_then(Value::as_str);
        if let Some(v) = s("title") {
            t.title = v.trim().to_string();
        }
        if let Some(v) = s("what") {
            t.what = v.trim().to_string();
        }
        if let Some(v) = s("kind") {
            t.kind = if v == "remind" { "remind".into() } else { "agent".into() };
        }
        if let Some(v) = s("when") {
            t.schedule = schedule::parse(v, Local::now())?;
            t.next_run = t.schedule.as_ref().and_then(|s| s.next_after(Local::now()));
            t.done = false;
        }
        if let Some(v) = patch.get("enabled").and_then(Value::as_bool) {
            t.enabled = v;
            if v && t.next_run.is_none() {
                t.next_run = t.schedule.as_ref().and_then(|s| s.next_after(Local::now()));
            }
        }
        if let Some(v) = patch.get("done").and_then(Value::as_bool) {
            t.done = v;
        }
        let out = t.clone();
        self.save(&all);
        Ok(out)
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut all = self.tasks.lock().unwrap();
        let before = all.len();
        all.retain(|t| !(t.id == id || (id.len() >= 4 && t.id.starts_with(id))));
        let changed = all.len() != before;
        if changed {
            self.save(&all);
        }
        changed
    }

    /// Ask the runner to run a task now (also to-dos and paused tasks).
    pub fn run_now(&self, id: &str) -> Result<Task, String> {
        let mut all = self.tasks.lock().unwrap();
        let t = all.iter_mut().find(|t| t.id == id || t.id.starts_with(id)).ok_or_else(|| format!("no task {id}"))?;
        if t.last_status.as_deref() == Some("running") {
            return Err("already running".into());
        }
        t.run_requested = true;
        t.done = false;
        let out = t.clone();
        self.save(&all);
        Ok(out)
    }

    /// Due tasks, marked running (so a slow run is not started twice).
    fn take_due(&self, now: DateTime<Local>) -> Vec<Task> {
        let mut all = self.tasks.lock().unwrap();
        let mut due = Vec::new();
        for t in all.iter_mut() {
            if t.last_status.as_deref() == Some("running") {
                continue;
            }
            let scheduled = t.enabled && !t.done && t.next_run.is_some_and(|n| n <= now);
            if t.run_requested || scheduled {
                t.run_requested = false;
                t.last_status = Some("running".into());
                due.push(t.clone());
            }
        }
        if !due.is_empty() {
            self.save(&all);
        }
        due
    }

    fn finish(&self, id: &str, ok: bool, result: String, session: Option<String>) {
        let mut all = self.tasks.lock().unwrap();
        if let Some(t) = all.iter_mut().find(|t| t.id == id) {
            let now = Local::now();
            t.last_run = Some(now);
            t.last_status = Some(if ok { "ok" } else { "failed" }.into());
            t.last_result = Some(kagent_core::util::truncate_str(&result, 2_000).to_string());
            t.last_session = session.or(t.last_session.take());
            t.runs += 1;
            match t.schedule.as_ref() {
                // Repeating: next slot. One-off: done once its time has passed.
                Some(s) if s.repeats() => t.next_run = s.next_after(now),
                Some(_) => {
                    t.next_run = t.next_run.filter(|n| *n > now);
                    t.done = t.next_run.is_none();
                }
                // To-do run on demand: done after a successful run.
                None => t.done = ok,
            }
        }
        self.save(&all);
    }
}

fn notify(title: &str, body: &str) {
    if std::env::var_os("KAGENT_NO_NOTIFY").is_some() {
        return;
    }
    let _ = std::process::Command::new("notify-send").args(["-a", "KAgent", "-i", "appointment-soon", title, body]).spawn();
}

/// Run one task to completion.
async fn execute(st: &crate::AppState, t: &Task) -> (bool, String, Option<String>) {
    if t.kind == "remind" {
        notify(&format!("Reminder: {}", t.title), &t.what);
        return (true, format!("reminded: {}", t.what), None);
    }
    let session = match st.agent.store().create(&format!("Task: {}", t.title)).await {
        Ok(m) => m.id,
        Err(e) => return (false, format!("could not create a session: {e:#}"), None),
    };
    let prompt = format!(
        "Scheduled task \"{}\" (runs unattended, the user is not watching). Do it now without asking questions; if something needs approval it will wait for the user. Finish with a one or two sentence result.\n\n{}",
        t.title, t.what
    );
    if let Err(e) = st.agent.send(&session, prompt, Via::Text).await {
        return (false, format!("could not start: {e:#}"), Some(session));
    }
    // Wait for the turn (approvals in `ask` mode may take a while).
    let deadline = std::time::Instant::now() + Duration::from_secs(45 * 60);
    tokio::time::sleep(Duration::from_secs(1)).await;
    while st.agent.is_running(&session).await && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let msgs = st.agent.messages(&session).await.unwrap_or_default();
    let answer = msgs.iter().rev().find(|m| m.role == Role::Assistant && !m.content.trim().is_empty()).map(|m| m.content.trim().to_string());
    match answer {
        Some(a) => {
            notify(&format!("Task done: {}", t.title), &kagent_core::util::truncate_str(&a, 300));
            (true, a, Some(session))
        }
        None => {
            notify(&format!("Task failed: {}", t.title), "no result; open the task's session");
            (false, "the agent gave no answer (see the task's session)".into(), Some(session))
        }
    }
}

/// Background runner: every 5 s start due tasks (each in its own tokio task).
pub fn start(st: Arc<crate::AppState>) {
    // Tasks marked running by a daemon that stopped mid-run are not running anymore.
    {
        let mut all = st.tasks.tasks.lock().unwrap();
        for t in all.iter_mut().filter(|t| t.last_status.as_deref() == Some("running")) {
            t.last_status = Some("failed".into());
            t.last_result = Some("interrupted by a KAgent restart".into());
        }
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            for t in st.tasks.take_due(Local::now()) {
                let st = st.clone();
                tokio::spawn(async move {
                    tracing::info!(task = t.id.as_str(), "running task \"{}\"", t.title);
                    let (ok, result, session) = execute(&st, &t).await;
                    st.tasks.finish(&t.id, ok, result, session);
                });
            }
        }
    });
}

fn render(t: &Task) -> String {
    let next = t.next_run.map(|n| n.format("%a %d %b %H:%M").to_string()).unwrap_or_else(|| "-".into());
    let state = if t.done { "done" } else if !t.enabled { "paused" } else { t.last_status.as_deref().unwrap_or("waiting") };
    format!("{}  [{}] {} ({}): {} | {} | next {} | runs {}{}", t.id, t.kind, t.title, state, t.what.chars().take(80).collect::<String>(), t.when(), next, t.runs, t.last_result.as_ref().map(|r| format!(" | last: {}", r.chars().take(120).collect::<String>())).unwrap_or_default())
}

pub struct TasksTool {
    contract: ToolContract,
    store: Arc<TaskStore>,
}

impl TasksTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        let mut contract = ToolContract::read_only(
            "tasks",
            "The user's task list and scheduler. add: a job KAgent runs later or repeatedly (kind agent: `what` is the instruction, e.g. 'check for security updates and tell me'), or a reminder (kind remind: `what` is shown as a notification). `when`: 'in 20m', '14:30', 'tomorrow 07:00', '2026-11-25 09:00', 'every 30m', 'hourly', 'daily 09:00', 'weekdays 08:30', 'mon,fri 18:00'; empty = a to-do run on demand. list, update (id + fields), delete, run (now), pause, resume, done.",
            json!({"type":"object","required":["action"],"properties":{
                "action":{"type":"string","enum":["list","add","update","delete","run","pause","resume","done"]},
                "id":{"type":"string"},
                "title":{"type":"string"},
                "what":{"type":"string"},
                "kind":{"type":"string","enum":["agent","remind"]},
                "when":{"type":"string"}
            }}),
        )
        .mutating(SideEffect::Workspace, Risk::Low);
        contract.needs_approval = false; // runs later under the permission mode in force then
        Self { contract, store }
    }
}

#[async_trait]
impl Tool for TasksTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let a = CallAssessment::from_contract(&self.contract, format!("tasks {action}"), format!("tasks {action}"));
        if action == "list" { a.as_read_only() } else { a }
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let g = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let id = g("id");
        let res = match g("action").as_str() {
            "list" => {
                let all = self.store.list();
                return Ok(ToolOutput::ok(if all.is_empty() { "no tasks".into() } else { all.iter().map(render).collect::<Vec<_>>().join("\n") }));
            }
            "add" => self.store.add(&g("title"), &g("what"), &g("kind"), &g("when")),
            "update" => self.store.update(&id, &input),
            "pause" => self.store.update(&id, &json!({"enabled": false})),
            "resume" => self.store.update(&id, &json!({"enabled": true})),
            "done" => self.store.update(&id, &json!({"done": true})),
            "run" => self.store.run_now(&id),
            "delete" => return Ok(if self.store.delete(&id) { ToolOutput::ok(format!("deleted {id}")) } else { ToolOutput::error(format!("no task {id}")) }),
            a => return Ok(ToolOutput::error(format!("unknown action {a}"))),
        };
        Ok(match res {
            Ok(t) => ToolOutput::ok(render(&t)),
            Err(e) => ToolOutput::error(e),
        })
    }
}

pub mod http {
    use super::*;
    use crate::AppState;
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::Json;

    type R = Result<Json<Value>, (StatusCode, Json<Value>)>;
    fn bad(e: String) -> (StatusCode, Json<Value>) {
        (StatusCode::BAD_REQUEST, Json(json!({"error": {"code": "invalid_input", "message": e}})))
    }

    pub async fn list(State(st): State<Arc<AppState>>) -> Json<Value> {
        let tasks: Vec<Value> = st.tasks.list().iter().map(|t| {
            let mut v = serde_json::to_value(t).unwrap_or(Value::Null);
            v["when"] = json!(t.when());
            v
        }).collect();
        Json(json!({"tasks": tasks}))
    }
    pub async fn add(State(st): State<Arc<AppState>>, Json(b): Json<Value>) -> R {
        let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        st.tasks.add(&s("title"), &s("what"), &s("kind"), &s("when")).map(|t| Json(json!(t))).map_err(bad)
    }
    pub async fn update(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(b): Json<Value>) -> R {
        st.tasks.update(&id, &b).map(|t| Json(json!(t))).map_err(bad)
    }
    pub async fn delete(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Json<Value> {
        Json(json!({"ok": st.tasks.delete(&id)}))
    }
    pub async fn run(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> R {
        st.tasks.run_now(&id).map(|t| Json(json!(t))).map_err(bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_lifecycle() {
        let d = tempfile::tempdir().unwrap();
        let (tx, _rx) = broadcast::channel(8);
        let s = TaskStore::load(d.path(), tx.clone());
        let t = s.add("", "check for security updates", "agent", "every 30m").unwrap();
        assert_eq!(t.title, "check for security updates");
        assert!(t.next_run.is_some() && t.enabled);
        let todo = s.add("Clean Downloads", "move old files to trash", "agent", "").unwrap();
        assert!(todo.next_run.is_none() && todo.schedule.is_none());
        assert!(s.add("x", "", "agent", "").is_err());
        assert!(s.add("x", "y", "agent", "whenever").is_err());
        // Nothing due yet; run_now makes the to-do due.
        assert!(s.take_due(Local::now()).is_empty());
        s.run_now(&todo.id).unwrap();
        let due = s.take_due(Local::now() + chrono::Duration::seconds(1));
        assert_eq!(due.len(), 1);
        assert!(s.run_now(&todo.id).is_err(), "already running");
        s.finish(&todo.id, true, "moved 12 files".into(), Some("sess".into()));
        let after = s.get(&todo.id).unwrap();
        assert!(after.done && after.runs == 1 && after.last_result.as_deref() == Some("moved 12 files"));
        // A paused repeating task still runs on "run now", and keeps its schedule.
        s.update(&t.id, &json!({"enabled": false})).unwrap();
        assert!(!s.get(&t.id).unwrap().enabled);
        s.run_now(&t.id).unwrap();
        assert_eq!(s.take_due(Local::now()).len(), 1);
        s.finish(&t.id, true, "no updates".into(), None);
        let r = s.get(&t.id).unwrap();
        assert!(!r.done && r.next_run.is_some() && !r.enabled, "still repeating, still paused");
        // Persisted.
        let again = TaskStore::load(d.path(), tx);
        assert_eq!(again.list().len(), 2);
        assert!(again.delete(&t.id));
        assert_eq!(again.list().len(), 1);
    }
}
