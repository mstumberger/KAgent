//! The `tasks` tool (automations.md section 4, tools.md section 23): the owner's task
//! list and scheduler. `list` and `history` are read-only; every other action is a MUT
//! call of low risk that needs no approval: a task runs later, under the permission
//! mode in force then.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Local};
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde_json::{json, Value};

use super::store::{chars, Task, TaskError, TaskInput, TaskRun, TaskStore};

type Clock = Arc<dyn Fn() -> DateTime<Local> + Send + Sync>;

const DESCRIPTION: &str = "The user's scheduled tasks and to-dos. add: {title, what, kind, schedule}. kind agent: KAgent runs `what` as a prompt when the schedule says, without the owner watching (a step that needs approval is skipped unless the permission mode allows it; the result is reported). kind remind: `what` is shown as a notification (prefer it for a plain reminder). schedule: words or cron, e.g. 'in 20m', '14:30', 'tomorrow 07:00', 'weekdays 08:30', 'every 30m', 'every 2 hours', 'every day at 09:00', 'mon,fri 18:00', '0 9 * * 1-5'; left out = a to-do, run on demand. For 'in N minutes' give delay_minutes instead of a schedule (KAgent computes the time). update: id and the fields to change. run: now. cancel: the running run. history: the last 10 runs. pause, resume, done, delete.";

/// `what` is one text or its lines: Ollama drops a call with a long text in a plain
/// string (S20).
fn what() -> Value {
    json!({"description": "the instruction of an agent task, or the text of a reminder. One text, or one line per item", "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]})
}

pub fn schema() -> Value {
    json!({"type":"object","required":["action"],"properties":{
        "action":{"type":"string","enum":["list","add","update","delete","run","pause","resume","done","cancel","history"]},
        "id":{"type":"string","description":"the task's id (list shows them)"},
        "title":{"type":"string","description":"a short name; empty = the first words of what"},
        "what": what(),
        "kind":{"type":"string","enum":["agent","remind"]},
        "schedule":{"type":"string","description":"in words ('in 20m', 'tomorrow 07:00', 'weekdays 08:30', 'every 30m') or a 5-field cron ('0 9 * * 1-5'); empty or left out = a to-do"},
        "delay_minutes":{"type":"integer","minimum":1,"maximum":525600,"description":"'in N minutes': the daemon computes the time"},
        "deliver":{"type":"array","items":{"type":"string","enum":["desktop","session","telegram"]},"description":"where a reminder or a result goes; left out = settings.tasks"},
        "on_approval":{"type":"string","enum":["deny","wait"],"description":"a step that needs approval: deny (skipped, the default) or wait (the card stays until it is answered)"},
        "session":{"type":"string","enum":["new","reuse"],"description":"a new session per run (the default), or one session for the task"},
        "timeout_min":{"type":"integer","minimum":1,"maximum":1440,"description":"how long a run may take (default 30)"}
    }})
}

pub struct TasksTool {
    contract: ToolContract,
    store: Arc<TaskStore>,
    now: Clock,
}

impl TasksTool {
    pub fn new(store: Arc<TaskStore>) -> Self {
        Self::build(store, Arc::new(Local::now))
    }

    #[cfg(test)]
    fn with_clock(store: Arc<TaskStore>, now: Clock) -> Self {
        Self::build(store, now)
    }

    fn build(store: Arc<TaskStore>, now: Clock) -> Self {
        let mut contract = ToolContract::read_only("tasks", DESCRIPTION, schema()).mutating(SideEffect::Workspace, Risk::Low);
        contract.needs_approval = false; // runs later under the permission mode in force then
        Self { contract, store, now }
    }
}

fn render(task: &Task) -> String {
    let next = task.next_run.map(|n| n.format("%a %d %b %H:%M").to_string()).unwrap_or_else(|| "-".into());
    let last = task.last_status.map(|s| s.to_string()).unwrap_or_else(|| "waiting".into());
    let state = if task.done { "done" } else if !task.enabled { "paused" } else { last.as_str() };
    // One line per task: a title or an instruction written on several lines is one line here.
    let one_line = |text: &str| chars(text, 80).replace('\n', " ");
    let last = task.last_result.as_ref().map(|r| format!(" | last: {}", chars(r, 120))).unwrap_or_default();
    let runs = task.total_runs();
    format!("{} [{}] {} ({}): {} | {} | next {} | {runs} run{}{last}", task.id, task.kind, one_line(&task.title), state, one_line(&task.what), task.schedule_text(), next, if runs == 1 { "" } else { "s" })
}

/// The last 10 runs, oldest first.
fn history(runs: &[TaskRun]) -> String {
    let shown: Vec<&TaskRun> = runs.iter().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect();
    if shown.is_empty() {
        return "no runs yet".into();
    }
    shown
        .iter()
        .map(|run| {
            let duration = run.duration_sec.map(|s| format!(" {s} s")).unwrap_or_default();
            let result = run.result.as_deref().map(|r| format!(": {}", chars(r, 120))).unwrap_or_default();
            format!("{} {}{duration}{result}", run.at.format("%a %d %b %H:%M"), run.status)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The input without the keys that name the action and the task: what changes a task.
fn patch(input: &Value) -> Result<TaskInput, TaskError> {
    let mut patch = input.clone();
    if let Value::Object(fields) = &mut patch {
        fields.remove("action");
        fields.remove("id");
    }
    TaskInput::parse(patch)
}

#[async_trait]
impl Tool for TasksTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let assessment = CallAssessment::from_contract(&self.contract, format!("tasks {action}"), format!("tasks {action}"));
        if matches!(action, "list" | "history") { assessment.as_read_only() } else { assessment }
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let id = || input.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let title = |id: &str| self.store.get(id).map(|t| t.title).unwrap_or_else(|| id.to_string());
        let now = (self.now)();
        match input.get("action").and_then(Value::as_str).unwrap_or("") {
            "list" => {
                let tasks = self.store.list();
                let text = if tasks.is_empty() { "no tasks".to_string() } else { tasks.iter().map(render).collect::<Vec<_>>().join("\n") };
                return Ok(ToolOutput::ok(text))
            }
            "history" => return Ok(self.store.runs(&id()).map(|runs| ToolOutput::ok(history(&runs))).unwrap_or_else(|e| ToolOutput::error(e.to_string()))),
            "run" => {
                return Ok(match self.store.request_run(&id()) {
                    Ok(()) => ToolOutput::ok(format!("\"{}\" runs at the next pass, within 15 seconds", title(&id()))),
                    Err(e) => ToolOutput::error(e.to_string()),
                })
            }
            "cancel" => {
                return Ok(match self.store.cancel(&id()) {
                    Ok(true) => ToolOutput::ok(format!("asked to cancel \"{}\"; its run ends as cancelled", title(&id()))),
                    Ok(false) => ToolOutput::error(format!("{} is not running", title(&id()))),
                    Err(e) => ToolOutput::error(e.to_string()),
                })
            }
            "delete" => return Ok(self.store.delete(&id()).map(|_| ToolOutput::ok(format!("deleted {}", id()))).unwrap_or_else(|e| ToolOutput::error(e.to_string()))),
            "add" => self.add(input, now),
            "update" => self.change(&id(), input, now),
            "pause" => self.change(&id(), json!({"enabled": false}), now),
            "resume" => self.change(&id(), json!({"enabled": true}), now),
            "done" => self.change(&id(), json!({"done": true}), now),
            other => return Ok(ToolOutput::error(format!("unknown action {other}"))),
        }
    }
}

impl TasksTool {
    fn add(&self, input: Value, now: DateTime<Local>) -> anyhow::Result<ToolOutput> {
        Ok(match patch(&input).and_then(|input| self.store.add(input, now)) {
            Ok(task) => ToolOutput::ok(format!("added {}", render(&task))),
            Err(e) => ToolOutput::error(e.to_string()),
        })
    }

    fn change(&self, id: &str, input: Value, now: DateTime<Local>) -> anyhow::Result<ToolOutput> {
        // The keys that name the action and the task are not fields of the task.
        let mut patch = input;
        if let Value::Object(fields) = &mut patch {
            fields.remove("action");
            fields.remove("id");
        }
        Ok(match self.store.update(id, patch, now) {
            Ok(task) => ToolOutput::ok(render(&task)),
            Err(e) => ToolOutput::error(e.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use chrono::TimeZone;
    use crate::sections::TasksSection;
    use crate::tasks::store::{RunStatus, SessionPolicy};

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
    }

    /// The tool over a temporary store, with a fixed clock.
    fn tool() -> (TasksTool, Arc<TaskStore>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (events, _receiver) = tokio::sync::broadcast::channel(64);
        let store = TaskStore::load(dir.path(), events, TasksSection::default());
        (TasksTool::with_clock(store.clone(), Arc::new(now)), store, dir)
    }

    async fn call(tool: &TasksTool, input: Value) -> (String, bool) {
        let out = tool.call(input, ToolContext::standalone(Duration::from_secs(5), 1 << 20)).await.unwrap();
        (out.content, out.is_error)
    }

    #[test]
    fn the_schema_has_both_forms_of_what_and_the_new_actions() {
        let schema = schema();
        assert_eq!(schema["properties"]["what"]["anyOf"], json!([{"type": "string"}, {"type": "array", "items": {"type": "string"}}]), "the S20 trap: a long text may come as lines");
        assert_eq!(schema["properties"]["action"]["enum"], json!(["list", "add", "update", "delete", "run", "pause", "resume", "done", "cancel", "history"]));
        assert_eq!(schema["properties"]["on_approval"]["enum"], json!(["deny", "wait"]));
        assert_eq!(schema["properties"]["session"]["enum"], json!(["new", "reuse"]));
        assert_eq!((schema["properties"]["delay_minutes"]["minimum"].clone(), schema["properties"]["delay_minutes"]["maximum"].clone()), (json!(1), json!(525600)));
        let plain = TasksTool::new(TaskStore::load(tempfile::tempdir().unwrap().path(), tokio::sync::broadcast::channel(4).0, TasksSection::default()));
        assert!(!plain.contract().needs_approval && !plain.contract().read_only, "MUT low without approval");
    }

    #[tokio::test]
    async fn add_from_words_a_delay_and_lines_then_list_and_history() {
        let (tool, store, _dir) = tool();
        let (out, error) = call(&tool, json!({"action": "add", "title": "Stretch", "what": "Stand up and stretch", "kind": "remind", "delay_minutes": 1, "deliver": ["desktop"]})).await;
        assert!(!error, "{out}");
        assert!(out.contains("added task_") && out.contains("(waiting): Stand up and stretch | once Mon 23 Nov 10:01 | next Mon 23 Nov 10:01"), "{out}");
        // The schedule in words, the instruction as lines, and the policy fields.
        let (out, error) = call(&tool, json!({"action": "add", "what": ["Run doctor.", "Reply NOTHING when the score is 90 or more."], "schedule": "weekdays 08:00", "on_approval": "wait", "session": "reuse", "timeout_min": 5})).await;
        assert!(!error, "{out}");
        assert!(out.contains("[agent] Run doctor. Reply NOTHING when the score is 90 or more. (waiting): Run doctor. Reply NOTHING when the score is 90 or more. | weekdays at 08:00 | next Tue 24 Nov 08:00 | 0 runs"), "one line per task: {out}");
        let id = store.list()[1].id.clone();
        let task = &store.list()[1];
        assert_eq!((task.on_approval, task.session, task.timeout_min), (crate::sections::TaskApproval::Wait, SessionPolicy::Reuse, 5));
        assert_eq!(task.title, "Run doctor.\nReply NOTHING when the score is 90 or more.");
        // list is the same line for each task.
        let (out, _) = call(&tool, json!({"action": "list"})).await;
        assert_eq!(out.lines().count(), 2);
        // What is refused says why.
        for (input, needle) in [
            (json!({"action": "add", "what": "x", "schedule": "sometimes"}), "Use e.g. 'in 20m', 'weekdays 08:30', '0 9 * * 1-5'"),
            (json!({"action": "add", "what": "x", "repeat": "weekly"}), "unknown field `repeat`"),
            (json!({"action": "add", "what": "x", "on_approval": "ignore"}), "unknown variant `ignore`"),
            (json!({"action": "add", "what": "x", "delay_minutes": 0}), "between 1 and 525600"),
            (json!({"action": "explode"}), "unknown action explode"),
            (json!({}), "unknown action "),
        ] {
            let (out, error) = call(&tool, input.clone()).await;
            assert!(error && out.contains(needle), "{input}: {out}");
        }
        assert_eq!(store.list().len(), 2, "nothing of the refused calls was stored");

        // history: the last 10 runs.
        let (out, error) = call(&tool, json!({"action": "history", "id": id})).await;
        assert!(!error && out == "no runs yet");
        for _ in 0..12 {
            let (out, _) = call(&tool, json!({"action": "run", "id": id})).await;
            assert!(out.starts_with("\"Run doctor."), "{out}");
            let claimed = store.claim(now(), 4);
            store.finish(&claimed[0].id, RunStatus::Ok, "NOTHING", Some("sess"), 4, now());
        }
        let (out, _) = call(&tool, json!({"action": "history", "id": &id})).await;
        assert_eq!(out.lines().count(), 10);
        assert!(out.starts_with("Mon 23 Nov 10:00 ok 4 s: NOTHING"), "{out}");
        assert!(call(&tool, json!({"action": "history", "id": "nope"})).await.1);
        // pause, resume, done, cancel; a short id is enough.
        let (out, _) = call(&tool, json!({"action": "pause", "id": &id[..9]})).await;
        assert!(out.contains("(paused)"), "{out}");
        let (out, _) = call(&tool, json!({"action": "resume", "id": &id})).await;
        assert!(out.contains("(ok)"), "{out}");
        let (out, error) = call(&tool, json!({"action": "cancel", "id": &id})).await;
        assert!(error && out == "Run doctor.\nReply NOTHING when the score is 90 or more. is not running", "{out}");
        call(&tool, json!({"action": "run", "id": &id})).await;
        let (out, error) = call(&tool, json!({"action": "cancel", "id": &id})).await;
        assert!(!error && out.starts_with("asked to cancel \"Run doctor."), "{out}");
        assert!(store.claim(now(), 4).is_empty(), "the run that was asked for is dropped");
        let (out, _) = call(&tool, json!({"action": "done", "id": &id})).await;
        assert!(out.contains("(done)"), "{out}");
        let (out, _) = call(&tool, json!({"action": "delete", "id": &id})).await;
        assert!(out.starts_with("deleted task_"), "{out}");
        assert_eq!(store.list().len(), 1);
    }

    #[tokio::test]
    async fn update_changes_only_the_named_fields() {
        let (tool, store, _dir) = tool();
        call(&tool, json!({"action": "add", "what": "ping", "kind": "remind", "schedule": "every 30m"})).await;
        let id = store.list()[0].id.clone();
        let (out, error) = call(&tool, json!({"action": "update", "id": &id, "title": "Ping the router", "schedule": "weekdays 08:30", "deliver": ["desktop"], "on_approval": "wait"})).await;
        assert!(!error, "{out}");
        let task = &store.list()[0];
        assert_eq!((task.title.as_str(), task.what.as_str(), task.schedule_text().as_str(), task.deliver.as_slice(), task.on_approval), ("Ping the router", "ping", "weekdays at 08:30", &["desktop".to_string()][..], crate::sections::TaskApproval::Wait));
        assert!(out.contains("(waiting): ping | weekdays at 08:30"), "{out}");
        // A field that is refused leaves the task as it was, whole.
        let (out, error) = call(&tool, json!({"action": "update", "id": &id, "title": "Changed", "schedule": "sometimes"})).await;
        assert!(error && out.contains("Use e.g. 'in 20m', 'weekdays 08:30', '0 9 * * 1-5'"), "{out}");
        assert_eq!(store.list()[0].title, "Ping the router");
        let (out, error) = call(&tool, json!({"action": "update", "id": "nope", "title": "x"})).await;
        assert!(error && out == "no task nope", "{out}");
    }

    #[test]
    fn list_and_history_are_read_only_the_rest_is_a_low_change() {
        let (tool, _store, _dir) = tool();
        for action in ["list", "history"] {
            let assessment = tool.assess(&json!({"action": action}));
            assert!(assessment.read_only, "{action}");
        }
        for action in ["add", "update", "delete", "run", "pause", "resume", "done", "cancel"] {
            let assessment = tool.assess(&json!({"action": action}));
            assert!(!assessment.read_only, "{action}");
        }
    }
}
