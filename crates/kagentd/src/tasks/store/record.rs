//! The task record (automations.md section 1) and what adds or changes one: the types of
//! the file, the input of the API and of the `tasks` tool with its checks, and how a
//! record from before S28 is read.

use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::sections::{check_deliver, check_timeout, TaskApproval, TasksSection};
use crate::tasks::schedule::{self, Schedule};

/// The largest `delayMinutes` (a year).
const MAX_DELAY_MINUTES: u32 = 525_600;

const SAY_WHAT: &str = "say what to do (or the text of the reminder)";

/// How a run ended (api.md `TaskRun`), or that it is still going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// While the task runs.
    Running,
    Ok,
    Failed,
    /// Its time was too far past when the daemon got to it.
    Missed,
    Timeout,
    Cancelled,
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Missed => "missed",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        })
    }
}

/// `new`: a session per run; `reuse`: one session for the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionPolicy {
    #[default]
    New,
    Reuse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRun {
    /// When the run started; of a missed run, when it should have.
    pub at: DateTime<Local>,
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

impl TaskRun {
    pub(super) fn running(at: DateTime<Local>) -> Self {
        Self { at, status: RunStatus::Running, duration_sec: None, session_id: None, result: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    /// What KAgent should do (agent), or the text of the reminder.
    pub what: String,
    /// `agent` (run `what` as a prompt) or `remind` (show `what`).
    pub kind: String,
    /// None: a to-do, run on demand.
    #[serde(default)]
    pub schedule: Option<Schedule>,
    /// Where a reminder or a result goes: `desktop`, `session`, `telegram`.
    #[serde(default = "default_deliver")]
    pub deliver: Vec<String>,
    /// What a step that needs approval does in an unattended run.
    #[serde(default)]
    pub on_approval: TaskApproval,
    #[serde(default)]
    pub session: SessionPolicy,
    /// How long a run may take.
    #[serde(default = "default_timeout")]
    pub timeout_min: u32,
    pub enabled: bool,
    pub created_at: DateTime<Local>,
    #[serde(default)]
    pub next_run: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_run: Option<DateTime<Local>>,
    #[serde(default)]
    pub last_status: Option<RunStatus>,
    #[serde(default)]
    pub last_result: Option<String>,
    #[serde(default)]
    pub last_session: Option<String>,
    /// The one session of a task with `session: reuse`.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The last runs, oldest first.
    #[serde(default)]
    pub runs: Vec<TaskRun>,
    /// Runs from before the history existed (a file of before S28 kept a count).
    #[serde(default)]
    pub runs_before: u32,
    /// A one-off that ran, or a to-do marked done.
    #[serde(default)]
    pub done: bool,
}

fn default_deliver() -> Vec<String> {
    TasksSection::default().deliver
}

fn default_timeout() -> u32 {
    TasksSection::default().timeout_min
}

impl Task {
    /// The schedule in a few words, for lists.
    pub fn schedule_text(&self) -> String {
        self.schedule.as_ref().map(Schedule::describe).unwrap_or_else(|| schedule::ON_DEMAND.to_string())
    }

    /// Runs so far: those the history holds and the count from before it existed.
    pub fn total_runs(&self) -> u32 {
        self.runs_before + u32::try_from(self.runs.len()).unwrap_or(u32::MAX)
    }
}

/// The instruction of an agent task, or the text of a reminder: one text, or its lines.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum What {
    Text(String),
    Lines(Vec<String>),
}

impl What {
    fn joined(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Lines(lines) => lines.join("\n"),
        }
    }
}

/// What adds a task (the API and the `tasks` tool), or changes one: then a field that
/// is left out keeps its value. The tool names the policy fields in snake_case.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskInput {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub what: Option<What>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Words, a cron expression, or a schedule as the daemon stores it. `when` is the
    /// name the API had before S28.
    #[serde(default, alias = "when", deserialize_with = "present")]
    pub schedule: Option<Value>,
    #[serde(default, alias = "delay_minutes")]
    pub delay_minutes: Option<u32>,
    #[serde(default)]
    pub deliver: Option<Vec<String>>,
    #[serde(default, alias = "on_approval")]
    pub on_approval: Option<TaskApproval>,
    #[serde(default)]
    pub session: Option<SessionPolicy>,
    #[serde(default, alias = "timeout_min")]
    pub timeout_min: Option<u32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub done: Option<bool>,
}

/// A schedule that is present but null removes the schedule (a to-do again): `Option`
/// alone would read the null as absent.
fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

impl TaskInput {
    /// A request body or a tool input. What is not a known field, or not of its type, is
    /// refused with the reason.
    pub fn parse(value: Value) -> Result<Self, TaskError> {
        serde_json::from_value(value).map_err(|e| TaskError::Invalid(format!("invalid task: {e}")))
    }

    /// A new task: a field that is left out takes its value from `defaults`
    /// (settings.tasks). `delayMinutes` is a `once` the daemon computes from `now`.
    pub fn into_task(self, defaults: &TasksSection, now: DateTime<Local>) -> Result<Task, TaskError> {
        let what = self.what.as_ref().map(What::joined).unwrap_or_default();
        let what = what.trim().to_string();
        if what.is_empty() {
            return Err(TaskError::Invalid(SAY_WHAT.into()));
        }
        let kind = kind_of(self.kind.as_deref())?;
        let schedule = schedule_of(self.schedule.as_ref(), self.delay_minutes, now)?;
        let next_run = schedule.as_ref().and_then(|s| s.next_run(now));
        let deliver = self.deliver.unwrap_or_else(|| defaults.deliver.clone());
        check_deliver(&deliver)?;
        let timeout_min = self.timeout_min.unwrap_or(defaults.timeout_min);
        check_timeout(timeout_min)?;
        let title = self.title.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| chars(&what, 60).to_string());
        Ok(Task {
            id: new_id(),
            title,
            what,
            kind: kind.to_string(),
            schedule,
            deliver,
            on_approval: self.on_approval.unwrap_or(defaults.on_approval),
            session: self.session.unwrap_or_default(),
            timeout_min,
            enabled: self.enabled.unwrap_or(true),
            created_at: schedule::stamp(now),
            next_run,
            last_run: None,
            last_status: None,
            last_result: None,
            last_session: None,
            session_id: None,
            runs: Vec::new(),
            runs_before: 0,
            done: self.done == Some(true),
        })
    }

    /// Change a task: only the fields that are present.
    pub fn apply(self, task: &mut Task, now: DateTime<Local>) -> Result<(), TaskError> {
        if let Some(title) = self.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            task.title = title.to_string();
        }
        if let Some(what) = self.what.as_ref().map(What::joined) {
            let what = what.trim().to_string();
            if what.is_empty() {
                return Err(TaskError::Invalid(SAY_WHAT.into()));
            }
            task.what = what;
        }
        if self.kind.is_some() {
            task.kind = kind_of(self.kind.as_deref())?.to_string();
        }
        if self.schedule.is_some() || self.delay_minutes.is_some() {
            task.schedule = schedule_of(self.schedule.as_ref(), self.delay_minutes, now)?;
            task.next_run = task.schedule.as_ref().and_then(|s| s.next_run(now));
            task.done = false;
        }
        if let Some(deliver) = self.deliver {
            check_deliver(&deliver)?;
            task.deliver = deliver;
        }
        if let Some(policy) = self.on_approval {
            task.on_approval = policy;
        }
        if let Some(policy) = self.session {
            task.session = policy;
            if policy == SessionPolicy::New {
                task.session_id = None;
            }
        }
        if let Some(minutes) = self.timeout_min {
            check_timeout(minutes)?;
            task.timeout_min = minutes;
        }
        if let Some(done) = self.done {
            task.done = done;
        }
        if let Some(enabled) = self.enabled {
            task.enabled = enabled;
            // Resumed after its time: the next slot from now on (nothing is replayed, and
            // nothing counts as missed).
            if enabled && task.next_run.is_none_or(|n| n <= now) {
                task.next_run = task.schedule.as_ref().and_then(|s| s.next_run(now));
            }
        }
        Ok(())
    }
}

fn kind_of(kind: Option<&str>) -> Result<&'static str, TaskError> {
    match kind {
        None | Some("agent") => Ok("agent"),
        Some("remind") => Ok("remind"),
        Some(other) => Err(TaskError::Invalid(format!("kind must be agent or remind, not '{other}'"))),
    }
}

fn schedule_of(raw: Option<&Value>, delay: Option<u32>, now: DateTime<Local>) -> Result<Option<Schedule>, TaskError> {
    let blank = matches!(raw, Some(Value::String(text)) if text.trim().is_empty());
    let raw = if blank { None } else { raw };
    match (raw, delay) {
        (None | Some(Value::Null), None) => Ok(None),
        (Some(Value::String(text)), None) => schedule::parse(text, now).map_err(TaskError::Invalid),
        (Some(value), None) => {
            let schedule = Schedule::from_value(value.clone())?;
            if let Schedule::Once { at } = &schedule {
                if *at <= now {
                    return Err(TaskError::Invalid(format!("that time is in the past: {}", at.format("%a %d %b %H:%M"))));
                }
            }
            Ok(Some(schedule))
        }
        (None | Some(Value::Null), Some(minutes)) => Ok(Some(delayed(minutes, now)?)),
        (Some(_), Some(_)) => Err(TaskError::Invalid("give either schedule or delayMinutes, not both".into())),
    }
}

/// `delayMinutes`: a `once` the daemon computes (the model never does date arithmetic).
fn delayed(minutes: u32, now: DateTime<Local>) -> Result<Schedule, TaskError> {
    if !(1..=MAX_DELAY_MINUTES).contains(&minutes) {
        return Err(TaskError::Invalid(format!("delayMinutes must be between 1 and {MAX_DELAY_MINUTES}, not {minutes}")));
    }
    Ok(Schedule::Once { at: schedule::stamp(now + Duration::minutes(i64::from(minutes))) })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    NotFound(String),
    Invalid(String),
    /// The task is doing it already.
    Busy(String),
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "no task {id}"),
            Self::Invalid(why) | Self::Busy(why) => write!(f, "{why}"),
        }
    }
}

impl From<String> for TaskError {
    fn from(why: String) -> Self {
        Self::Invalid(why)
    }
}

/// The first `n` characters of a text.
pub(crate) fn chars(text: &str, n: usize) -> &str {
    kagent_core::util::truncate_str(text, n)
}

fn new_id() -> String {
    format!("task_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

pub(super) fn from_stored(record: Value, defaults: &TasksSection) -> Result<Task, String> {
    let Value::Object(mut fields) = record else {
        return Err("a task must be an object".into());
    };
    let id = fields.get("id").and_then(Value::as_str).unwrap_or("?").to_string();
    // Before the history, `runs` was a count (a number); it is kept beside the history.
    if let Some(count) = fields.get("runs").and_then(Value::as_u64) {
        let before = count + fields.get("runsBefore").and_then(Value::as_u64).unwrap_or(0);
        fields.insert("runsBefore".into(), json!(u32::try_from(before).unwrap_or(u32::MAX)));
        fields.remove("runs");
    }
    // A policy field the record does not have comes from settings.tasks.
    for (key, value) in [
        ("deliver", json!(defaults.deliver)),
        ("onApproval", json!(defaults.on_approval)),
        ("timeoutMin", json!(defaults.timeout_min)),
        ("session", json!(SessionPolicy::New)),
    ] {
        if !fields.contains_key(key) {
            fields.insert(key.into(), value);
        }
    }
    serde_json::from_value(Value::Object(fields)).map_err(|e| format!("task {id}: {e}"))
}
