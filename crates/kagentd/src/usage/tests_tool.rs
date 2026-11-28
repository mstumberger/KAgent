use kagent_core::tool::{Tool, ToolContext};
use kagent_providers::usage::{Limit, Observation, Source, Unit};
use serde_json::json;

use super::tool::{at_utc, describe, UsageCell, UsageTool};
use super::tests::hub;

fn now() -> chrono::DateTime<chrono::Local> {
    chrono::Local::now()
}

/// A view as `GET /api/usage` has it, with one provider at 94 % of a daily limit and one local.
fn view() -> serde_json::Value {
    let reset = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
    json!({"providers": [
        {"id": "gemini", "name": "Google Gemini", "local": false,
         "limits": [
           {"id": "requests", "name": "Requests per day", "used": 47, "total": 50, "unit": "requests", "resetsAt": reset, "source": "provider", "level": "nearlyOut"},
           {"id": "known-month-tokens", "name": "Tokens per month", "used": 120000, "total": 2000000, "unit": "tokens", "resetsAt": null, "source": "kagent", "level": "fine"},
           {"id": "rate", "name": "Rate limit", "used": null, "total": null, "unit": "requests", "resetsAt": null, "source": "provider", "level": "out"}],
         "today": {"requests": 12, "inputTokens": 30000, "outputTokens": 4000}, "month": {"requests": 340, "inputTokens": 900000, "outputTokens": 120000},
         "models": [{"model": "gemini-3.8-flash", "today": {"requests": 9, "inputTokens": 2400, "outputTokens": 800}, "month": {"requests": 90, "inputTokens": 52000, "outputTokens": 9000}}]},
        {"id": "ollama", "name": "Ollama (local)", "local": true, "limits": [],
         "today": {"requests": 3, "inputTokens": 100, "outputTokens": 50}, "month": {"requests": 3, "inputTokens": 100, "outputTokens": 50}, "models": []},
        {"id": "nvidia", "name": "NVIDIA", "local": false, "limits": [],
         "today": {"requests": 0, "inputTokens": 0, "outputTokens": 0}, "month": {"requests": 0, "inputTokens": 0, "outputTokens": 0}, "models": []}]})
}

#[test]
fn all_providers_in_words() {
    let out = describe(&view(), None, now());
    assert!(!out.is_error && out.kind == kagent_core::tool::ResultKind::Ok);
    let t = &out.content;
    assert!(t.contains("Google Gemini (gemini):"), "{t}");
    assert!(t.contains("Requests per day 47 of 50 (94 %, nearly out), resets "), "{t}");
    assert!(t.contains("Tokens per month 120k of 2.0M (6 %, fine), no reset time known [counted by KAgent: other apps using this key are not included]."), "{t}");
    assert!(t.contains("Rate limit: out, no reset time known."), "a limit with no size has no numbers: {t}");
    assert!(t.contains("Through KAgent today: 12 requests (30k input and 4000 output tokens); this month: 340 requests (900k input and 120k output tokens)."), "{t}");
    assert!(t.contains("gemini-3.8-flash: today 9 requests"), "{t}");
    assert!(t.contains("Ollama (local) (ollama):\n  No limits: a local provider."), "{t}");
    assert!(t.contains("NVIDIA (nvidia):\n  No limit known: the provider has reported none"), "{t}");
    assert!(t.contains("live voice is not counted"), "{t}");
}

#[test]
fn one_provider_by_id_or_name() {
    for f in ["gemini", "GEMINI", "google gemini", " gemini "] {
        let out = describe(&view(), Some(f), now());
        assert!(out.content.contains("Google Gemini (gemini):") && !out.content.contains("NVIDIA"), "{f}: {}", out.content);
    }
}

#[test]
fn an_unknown_provider_is_not_done_and_says_what_to_do() {
    let out = describe(&view(), Some("openai"), now());
    assert!(out.is_error && out.kind == kagent_core::tool::ResultKind::NotDone);
    assert_eq!(out.content, "not done: no provider named openai. Next: call usage without a provider to list them (providers: gemini, ollama, nvidia).");
    let empty = describe(&json!({"providers": []}), None, now());
    assert!(!empty.is_error && empty.content.contains("No providers are set up"));
}

#[test]
fn a_reset_on_another_day_names_the_day() {
    let out = describe(&json!({"providers": [{"id": "g", "name": "G", "local": false, "limits": [{"id": "r", "name": "Requests per day", "used": 5, "total": 10, "unit": "requests", "resetsAt": "2099-03-04T09:00:00Z", "source": "provider", "level": "fine"}],
        "today": {"requests": 0, "inputTokens": 0, "outputTokens": 0}, "month": {"requests": 0, "inputTokens": 0, "outputTokens": 0}, "models": []}]}), None, at_utc("2026-11-28T09:00:00Z"));
    assert!(out.content.contains("(50 %, fine), resets Mar "), "{}", out.content);
}

#[tokio::test]
async fn the_tool_is_read_only_in_group_agent_and_reads_the_hub() {
    let (_d, hub, _rx) = hub();
    let reset = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let limit = Limit::new("requests", "Requests", Some(94), Some(100), Unit::Requests, chrono::DateTime::parse_from_rfc3339(&reset).ok().map(|t| t.with_timezone(&chrono::Utc)), Source::Provider);
    hub.observe("gemini", Observation { status: 200, limits: vec![limit], seen: chrono::Utc::now() });
    let cell = UsageCell::default();
    let tool = UsageTool::new(cell.clone());
    assert_eq!(tool.contract().name, "usage");
    assert!(tool.contract().read_only && !tool.contract().needs_approval);
    assert_eq!(kagent_core::tools::group_of("usage"), "agent");
    let a = tool.assess(&json!({"provider": "gemini"}));
    assert!(a.read_only && !a.needs_approval && a.summary.contains("gemini"));
    let before = tool.call(json!({}), ToolContext::standalone(std::time::Duration::from_secs(5), 100_000)).await.unwrap();
    assert!(before.is_error && before.content.contains("not running"), "{}", before.content);
    cell.set(hub).ok();
    let out = tool.call(json!({"provider": "gemini"}), ToolContext::standalone(std::time::Duration::from_secs(5), 100_000)).await.unwrap();
    assert!(out.content.contains("Requests 94 of 100 (94 %, nearly out)"), "{}", out.content);
    assert!(!out.content.contains("SECRET"), "no key");
}
