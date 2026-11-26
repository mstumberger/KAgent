//! `task_output` and `task_stop` (tools.md section 21): read and stop the background shell
//! tasks of `tools::background`.

use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::agents::unasking;
use super::background::{Background, TaskStatus};
use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput};

const OUTPUT_DESCRIPTION: &str = "Read the output of a background shell task (the taskId the shell tool answered with background: true): its state and the last `tail` characters of its output. wait_sec waits for the task to end, at most 30 s. The session is told when a task ends on its own.";
const STOP_DESCRIPTION: &str = "Kill a running background shell task (the taskId the shell tool answered with background: true). A killed task reports nothing.";

/// `task_id`, `wait_sec` and `tail`, as tools.md names them (snake_case, unlike the settings).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputArgs {
    task_id: String,
    #[serde(default)]
    wait_sec: Option<u64>,
    #[serde(default)]
    tail: Option<usize>,
}

pub struct TaskOutput {
    contract: ToolContract,
    tasks: Background,
}

impl TaskOutput {
    pub fn new(tasks: Background) -> Self {
        // wait_sec is at most 30 (tools.md section 21).
        Self {
            contract: ToolContract::read_only(
                "task_output",
                OUTPUT_DESCRIPTION,
                json!({"type":"object","required":["task_id"],"properties":{
                    "task_id":{"type":"string","description":"the id the shell tool answered (bg_...)"},
                    "wait_sec":{"type":"integer","description":"wait up to this many seconds for the task to end; default 0, at most 30"},
                    "tail":{"type":"integer","description":"the last characters of the output; default 4000, 100 to 20000"}
                }}),
            )
            .timeout(35_000, 35_000),
            tasks,
        }
    }
}

#[async_trait]
impl Tool for TaskOutput {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Ok(args) = serde_json::from_value::<OutputArgs>(input) else {
            return Ok(ToolOutput::error("task_output needs task_id: the id the shell tool answered (bg_...)"));
        };
        // Outside their range, the nearest end (as the web tools read theirs).
        let wait = Duration::from_secs(args.wait_sec.unwrap_or(0).min(30));
        let tail = args.tail.unwrap_or(4_000).clamp(100, 20_000);
        let deadline = Instant::now() + wait;
        loop {
            let Some((info, text)) = self.tasks.output(&args.task_id, tail).await else {
                return Ok(ToolOutput::error(format!(
                    "no background task '{}': unknown, or ended long ago and forgotten. The shell tool answers the id when background is true",
                    args.task_id
                )));
            };
            let left = deadline.saturating_duration_since(Instant::now());
            if info.status != TaskStatus::Running || left.is_zero() || ctx.cancel.is_cancelled() {
                let mut out = info.head();
                if text.trim().is_empty() {
                    out.push_str("\n(no output)");
                } else {
                    out.push('\n');
                    out.push_str(text.trim_end());
                }
                return Ok(ToolOutput::ok(out));
            }
            tokio::select! {
                _ = tokio::time::sleep(left.min(Duration::from_millis(200))) => {}
                // The turn is cancelled: say how the task is now, at once.
                _ = ctx.cancel.cancelled() => {}
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopArgs {
    task_id: String,
}

pub struct TaskStop {
    contract: ToolContract,
    tasks: Background,
}

impl TaskStop {
    pub fn new(tasks: Background) -> Self {
        Self {
            contract: unasking(
                "task_stop",
                STOP_DESCRIPTION,
                json!({"type":"object","required":["task_id"],"properties":{"task_id":{"type":"string","description":"the id the shell tool answered (bg_...)"}}}),
            ),
            tasks,
        }
    }
}

#[async_trait]
impl Tool for TaskStop {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> crate::tool::CallAssessment {
        let id = input.get("task_id").and_then(Value::as_str).unwrap_or("");
        crate::tool::CallAssessment::from_contract(&self.contract, format!("kill background task {id}"), "task_stop".into())
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Ok(args) = serde_json::from_value::<StopArgs>(input) else {
            return Ok(ToolOutput::error("task_stop needs task_id: the id the shell tool answered (bg_...)"));
        };
        if self.tasks.stop(&args.task_id) {
            Ok(ToolOutput::ok(format!("[Task {} killed]", args.task_id)))
        } else {
            Ok(ToolOutput::error(format!("background task {} is not running: unknown, ended, or killed already", args.task_id)))
        }
    }
}

/// Every argument `task_output` and `task_stop` take, as JSON says it: `task_id` in
/// snake_case, as tools.md section 21 names it.
#[test]
fn the_task_arguments_are_snake_case() {
    let id = json!({"task_id": "bg_1a2b3c4d", "wait_sec": 5, "tail": 100});
    let args: OutputArgs = serde_json::from_value(id).unwrap();
    assert_eq!((args.task_id.as_str(), args.wait_sec, args.tail), ("bg_1a2b3c4d", Some(5), Some(100)));
    assert!(serde_json::from_value::<OutputArgs>(json!({"task_id": "bg_1a2b3c4d", "taskId": "x"})).is_err(), "an unknown key is refused");
    assert!(serde_json::from_value::<OutputArgs>(json!({"taskId": "x"})).is_err(), "camelCase is not the name of the field");
    let stop: StopArgs = serde_json::from_value(json!({"task_id": "bg_1a2b3c4d"})).unwrap();
    assert_eq!(stop.task_id, "bg_1a2b3c4d");
    let contract = TaskStop::new(Background::new(std::env::temp_dir())).contract().clone();
    assert!(!contract.read_only && !contract.needs_approval && contract.concurrency_safe && contract.risk == crate::tool::Risk::Low, "MUT, risk low, no approval (tools.md section 21)");
}
