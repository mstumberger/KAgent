//! The tool `screen_watch` (screen-watch.md section 4, tools.md): `start`, `status`, `stop`.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{Mode, ScreenWatch, StartArgs, Target, Watch};

pub type WatchCell = Arc<OnceLock<Arc<ScreenWatch>>>;

pub struct ScreenWatchTool {
    contract: ToolContract,
    cell: WatchCell,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    action: String,
    goal: Option<String>,
    minutes: Option<u32>,
    every_sec: Option<u32>,
    mode: Option<Mode>,
    target: Option<Target>,
}

impl ScreenWatchTool {
    pub fn new(cell: WatchCell) -> Self {
        let schema = json!({"type":"object","required":["action"],"properties":{
            "action":{"type":"string","enum":["start","status","stop"]},
            "goal":{"type":"string","description":"start: what to wait for, at most 200 characters, e.g. 'the build in the terminal finishes'"},
            "minutes":{"type":"integer","minimum":1,"maximum":120,"description":"start: how long to watch (default 20, at most 120)"},
            "everySec":{"type":"integer","minimum":5,"maximum":60,"description":"start: seconds between looks (default 10)"},
            "mode":{"type":"string","enum":["once","repeat"],"description":"start: once ends at the first hit (default); repeat reports every new hit, at most one per 30 s (for 'watch for errors')"},
            "target":{"type":"string","enum":["window","monitor"],"description":"start: the window that is active now (default) or the monitor"}
        }});
        let contract = ToolContract::read_only(
            "screen_watch",
            "Watch the screen for the owner for a goal and a time: a local vision model looks at a frame every few seconds and KAgent tells the owner when the goal is met or the time is up. Use it when asked to 'tell me when the build finishes' or 'watch for errors for 20 minutes'. Only on the owner's request, one watch at a time; no frame is stored and none leaves the machine. The chat shows a chip while it runs. start needs a goal; status says how it is; stop ends it ('stop watching').",
            schema,
        )
        .mutating(SideEffect::None, Risk::Medium)
        .timeout(30_000, 60_000);
        let mut contract = contract;
        contract.always_ask = true;
        Self { contract, cell }
    }
}

/// How a watch is described to the model: what it watches, until when, and how it goes.
pub fn describe(w: &Watch) -> String {
    let note = if w.last_note.is_empty() { "nothing yet" } else { w.last_note.as_str() };
    let paused = if w.paused { " The screen is locked: it is paused." } else { "" };
    format!("Screen watch {} of {} until {} ({} mode, a look every {} s), goal: {}. {} looks so far; last note: {note}.{paused}", w.id, w.what(), w.until, w.mode.name(), w.every_sec, w.goal, w.looks)
}

/// What `status` says about a watch that ended.
pub fn describe_ended(w: &Watch) -> String {
    let why = w.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default();
    let note = if w.last_note.is_empty() { "none" } else { w.last_note.as_str() };
    format!("No screen watch is running. The last one ended as {}{why} after {} looks: goal \"{}\", last note: {note}.", w.state.name(), w.looks, w.goal)
}

#[async_trait]
impl Tool for ScreenWatchTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let parsed: Option<Input> = serde_json::from_value(input.clone()).ok();
        match parsed.as_ref().map(|i| i.action.as_str()) {
            Some("start") => {
                let i = parsed.as_ref().unwrap();
                let minutes = i.minutes.unwrap_or(20);
                let what = if i.target == Some(Target::Monitor) {
                    "the monitor".to_string()
                } else {
                    super::capture::active_title_blocking().map(|t| format!("\"{t}\"")).unwrap_or_else(|| "the active window".into())
                };
                let goal = i.goal.as_deref().unwrap_or("").trim();
                CallAssessment::from_contract(&self.contract, format!("watch {what} for {minutes} minutes: {goal}"), "screen_watch start".into())
            }
            _ => CallAssessment::from_contract(&self.contract, "screen watch status or stop".into(), "screen_watch".into()).as_read_only(),
        }
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Some(sw) = self.cell.get() else { return Ok(ToolOutput::error("the screen watch is not running in this daemon")) };
        let i: Input = match serde_json::from_value(input) {
            Ok(i) => i,
            Err(e) => return Ok(ToolOutput::error(format!("screen_watch: {e}. Use {{\"action\": \"start\", \"goal\": \"...\"}}, \"status\" or \"stop\"."))),
        };
        Ok(match i.action.as_str() {
            "start" => {
                let args = StartArgs { session_id: ctx.session_id.clone(), goal: i.goal.unwrap_or_default(), minutes: i.minutes, every_sec: i.every_sec, mode: i.mode, target: i.target };
                match sw.start(args).await {
                    Ok(w) => ToolOutput::ok(format!("Watching {} until {} ({} minutes): {}. I will tell the owner when it is met; the chat shows a chip while it runs.", w.what(), w.until, w.minutes, w.goal)),
                    Err(e) => ToolOutput::error(e),
                }
            }
            "status" => match (sw.current(), sw.last_ended()) {
                (Some(w), _) => ToolOutput::ok(describe(&w)),
                (None, Some(w)) => ToolOutput::ok(describe_ended(&w)),
                (None, None) => ToolOutput::ok("no screen watch"),
            },
            "stop" => match sw.stop() {
                Some(w) => ToolOutput::ok(format!("Stopped the screen watch after {} looks (goal: {}).", w.looks, w.goal)),
                None => ToolOutput::ok("no screen watch is running"),
            },
            other => ToolOutput::error(format!("unknown action '{other}': use start, status or stop")),
        })
    }
}
