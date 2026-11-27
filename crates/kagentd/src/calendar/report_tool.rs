//! `calendar_report` (calendar.md section 3): the morning report's text, for "what's on today?".

use async_trait::async_trait;
use kagent_core::tool::{CallAssessment, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde_json::{json, Value};

use super::report::Day;
use super::tools::{failed, ready, CalendarCell, UNTRUSTED};

pub struct CalendarReport {
    contract: ToolContract,
    cell: CalendarCell,
}

impl CalendarReport {
    pub fn new(cell: CalendarCell) -> Self {
        let mut contract = ToolContract::read_only(
            "calendar_report",
            "The owner's morning report as text: today's (or tomorrow's) events, overlaps, when to leave, and KAgent's own reminders due. Use it for 'what's on today?', 'what is tomorrow like?' and 'repeat the morning report'; answer from it in the owner's language.",
            json!({"type":"object","properties":{"day":{"type":"string","enum":["today","tomorrow"],"description":"default today"}}}),
        );
        contract.side_effect = SideEffect::Network;
        Self { contract: contract.timeout(40_000, 90_000), cell }
    }
}

#[async_trait]
impl Tool for CalendarReport {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let day = input.get("day").and_then(Value::as_str).unwrap_or("today");
        CallAssessment::from_contract(&self.contract, format!("calendar report for {day}"), "calendar_report".to_string())
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let cal = match ready(&self.cell) {
            Ok(cal) => cal,
            Err(out) => return Ok(out),
        };
        let day = if input.get("day").and_then(Value::as_str) == Some("tomorrow") { Day::Tomorrow } else { Day::Today };
        Ok(match cal.compose(day).await {
            Ok(text) => ToolOutput::ok(format!("{UNTRUSTED}\n{text}")),
            Err(e) => failed(e),
        })
    }
}
