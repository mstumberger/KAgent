//! `usage {provider?}` (usage.md section 6): the limits and the counters of the providers as text, so "how much do I
//! have left on Gemini?" has an answer. Read-only. The result is worded per `agent-loop.md` section B.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use kagent_core::tool::{CallAssessment, Tool, ToolContext, ToolContract, ToolOutput};
use serde_json::{json, Value};

use super::UsageHub;

pub type UsageCell = Arc<OnceLock<Arc<UsageHub>>>;

pub struct UsageTool {
    contract: ToolContract,
    cell: UsageCell,
}

impl UsageTool {
    pub fn new(cell: UsageCell) -> Self {
        let contract = ToolContract::read_only(
            "usage",
            "How much of each provider's limits is used, and what went through KAgent: per provider its limits (a name, used of total, a percentage, when it resets, and where the number comes from: the provider, or 'counted by KAgent' which does not include other apps using the same key) and today's and this month's requests and tokens. Use it for 'how much do I have left on Gemini?'. A local provider has no limits. Live voice is not counted. Give `provider` (its id or name) for one, none for all.",
            json!({"type":"object","properties":{"provider":{"type":"string","description":"The id or name of one provider; leave out for all."}}}),
        );
        Self { contract, cell }
    }
}

fn time_text(iso: &str, now: DateTime<Local>) -> String {
    let Ok(t) = DateTime::parse_from_rfc3339(iso) else { return String::new() };
    let t = t.with_timezone(&Local);
    if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%b %-d %H:%M").to_string()
    }
}

fn count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else {
        n.to_string()
    }
}

fn level_words(level: &str) -> &'static str {
    match level {
        "fine" => "fine",
        "close" => "close to the limit",
        "nearlyOut" => "nearly out",
        _ => "out",
    }
}

/// One limit as a line: "Requests per day 47 of 50 (94 %, nearly out), resets 09:00."
fn limit_line(l: &Value, now: DateTime<Local>) -> String {
    let name = l["name"].as_str().unwrap_or("Limit");
    let reset = l["resetsAt"].as_str().map(|r| time_text(r, now)).filter(|t| !t.is_empty()).map(|t| format!(", resets {t}")).unwrap_or_else(|| ", no reset time known".into());
    let mut line = match (l["used"].as_u64(), l["total"].as_u64()) {
        (Some(u), Some(t)) if t > 0 => format!("{name} {} of {} ({} %, {}){reset}", count(u), count(t), (u * 100 / t).min(100), level_words(l["level"].as_str().unwrap_or(""))),
        _ => format!("{name}: {}{reset}", level_words(l["level"].as_str().unwrap_or("out"))),
    };
    if l["source"] == "kagent" {
        line.push_str(" [counted by KAgent: other apps using this key are not included]");
    }
    line.push('.');
    line
}

fn totals(t: &Value) -> String {
    format!("{} requests ({} input and {} output tokens)", t["requests"], count(t["inputTokens"].as_u64().unwrap_or(0)), count(t["outputTokens"].as_u64().unwrap_or(0)))
}

fn one(p: &Value, now: DateTime<Local>) -> String {
    let mut out = vec![format!("{} ({}):", p["name"].as_str().unwrap_or("?"), p["id"].as_str().unwrap_or("?"))];
    let limits = p["limits"].as_array().cloned().unwrap_or_default();
    if limits.is_empty() {
        out.push(if p["local"] == true {
            "  No limits: a local provider.".into()
        } else {
            "  No limit known: the provider has reported none on its answers, and none was typed (Settings -> Models, the provider's Usage block).".into()
        });
    }
    out.extend(limits.iter().map(|l| format!("  {}", limit_line(l, now))));
    out.push(format!("  Through KAgent today: {}; this month: {}.", totals(&p["today"]), totals(&p["month"])));
    for m in p["models"].as_array().into_iter().flatten().take(8) {
        out.push(format!("    {}: today {}, this month {}", m["model"].as_str().unwrap_or("?"), totals(&m["today"]), totals(&m["month"])));
    }
    out.join("\n")
}

/// The answer for `filter` (an id or a name, any case) or for every provider, from the view of `GET /api/usage`.
pub fn describe(view: &Value, filter: Option<&str>, now: DateTime<Local>) -> ToolOutput {
    let all = view["providers"].as_array().cloned().unwrap_or_default();
    let chosen: Vec<&Value> = match filter.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => {
            let found: Vec<&Value> = all.iter().filter(|p| p["id"].as_str().is_some_and(|id| id.eq_ignore_ascii_case(f)) || p["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(f))).collect();
            if found.is_empty() {
                let names: Vec<&str> = all.iter().filter_map(|p| p["id"].as_str()).collect();
                return ToolOutput::not_done(&format!("no provider named {f}"), &format!("call usage without a provider to list them (providers: {})", if names.is_empty() { "none".into() } else { names.join(", ") }));
            }
            found
        }
        None => all.iter().collect(),
    };
    if chosen.is_empty() {
        return ToolOutput::ok("No providers are set up: nothing is used and no limit is known.");
    }
    let mut text = chosen.iter().map(|p| one(p, now)).collect::<Vec<_>>().join("\n");
    text.push_str("\nCounts are what went through KAgent (live voice is not counted).");
    ToolOutput::ok(text)
}

#[async_trait]
impl Tool for UsageTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let what = input.get("provider").and_then(Value::as_str).filter(|p| !p.is_empty()).map(|p| format!("read the usage and limits of {p}")).unwrap_or_else(|| "read the usage and limits of the providers".into());
        CallAssessment::from_contract(&self.contract, what, "usage".to_string())
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Some(hub) = self.cell.get() else { return Ok(ToolOutput::error("the usage module is not running")) };
        Ok(describe(&hub.view(), input.get("provider").and_then(Value::as_str), Local::now()))
    }
}

/// Only for the tests: a time as the tool writes it.
#[cfg(test)]
pub(crate) fn at_utc(s: &str) -> DateTime<Local> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc).with_timezone(&Local)
}
