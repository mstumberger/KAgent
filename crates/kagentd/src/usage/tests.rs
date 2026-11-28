use std::sync::Arc;

use chrono::{Local, TimeZone};
use kagent_core::model::CallSource;
use kagent_core::protocol::{AgentEvent, Usage};
use kagent_providers::usage::UsageSink;
use kagent_providers::ProviderStore;
use tokio::sync::broadcast;

use super::store::{Counters, Window};
use super::UsageHub;

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Local> {
    Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().expect("a local time that exists")
}

fn usage(i: u64, o: u64) -> Usage {
    Usage { input_tokens: i, output_tokens: o }
}

#[test]
fn an_answer_is_added_to_the_minute_the_day_and_the_month() {
    let c = Counters::memory().unwrap();
    let t = at(2026, 11, 28, 9, 30);
    c.record(t, "gemini", "flash", "chat", 100, 20).unwrap();
    c.record(t, "gemini", "flash", "chat", 50, 5).unwrap();
    c.record(t, "gemini", "pro", "chat", 10, 1).unwrap();
    c.record(t, "nvidia", "x", "chat", 999, 999).unwrap();
    for w in [Window::Minute, Window::Day, Window::Month] {
        let totals = c.totals("gemini", w, t);
        assert_eq!((totals.requests, totals.input_tokens, totals.output_tokens), (3, 160, 26), "{w:?}");
    }
    let per = c.per_model("gemini", t);
    assert_eq!(per.iter().map(|m| (m.model.as_str(), m.today.requests, m.month.input_tokens)).collect::<Vec<_>>(), [("flash", 2, 150), ("pro", 1, 10)], "the busiest first");
}

#[test]
fn who_used_it_is_kept_apart() {
    let c = Counters::memory().unwrap();
    let t = at(2026, 11, 28, 9, 30);
    for source in ["chat", "chat", "task", "subagent", "voice", "other"] {
        c.record(t, "gemini", "flash", source, 1, 1).unwrap();
    }
    assert_eq!(c.by_source("gemini", t), [("chat".to_string(), 2), ("other".to_string(), 1), ("subagent".to_string(), 1), ("task".to_string(), 1), ("voice".to_string(), 1)]);
    assert_eq!(c.totals("gemini", Window::Day, t).requests, 6, "the total is all of them");
}

#[test]
fn the_next_minute_day_and_month_start_new_rows() {
    let c = Counters::memory().unwrap();
    c.record(at(2026, 11, 30, 23, 59), "gemini", "flash", "chat", 10, 1).unwrap();
    // The next minute, the same day.
    assert_eq!(c.totals("gemini", Window::Minute, at(2026, 11, 30, 23, 59)).requests, 1);
    // Across midnight: a new day and a new month (November ends on the 30th).
    let next = at(2026, 12, 1, 0, 0);
    assert_eq!(c.totals("gemini", Window::Minute, next).requests, 0);
    assert_eq!(c.totals("gemini", Window::Day, next).requests, 0, "today is empty after midnight");
    assert_eq!(c.totals("gemini", Window::Month, next).requests, 0, "and the month is a new one");
    c.record(next, "gemini", "flash", "chat", 5, 1).unwrap();
    assert_eq!(c.totals("gemini", Window::Day, next).input_tokens, 5);
    assert_eq!(c.totals("gemini", Window::Day, at(2026, 11, 30, 12, 0)).input_tokens, 10, "yesterday is kept");
    assert_eq!(c.totals("gemini", Window::Month, at(2026, 11, 2, 12, 0)).requests, 1);
    // A year ends: December to January.
    c.record(at(2026, 12, 31, 23, 59), "gemini", "flash", "chat", 1, 1).unwrap();
    assert_eq!(c.totals("gemini", Window::Month, at(2027, 1, 1, 0, 0)).requests, 0);
}

#[test]
fn rows_older_than_13_months_are_pruned() {
    let c = Counters::memory().unwrap();
    c.record(at(2025, 9, 15, 10, 0), "gemini", "flash", "chat", 1, 1).unwrap();
    c.record(at(2025, 10, 15, 10, 0), "gemini", "flash", "chat", 1, 1).unwrap();
    c.record(at(2026, 11, 28, 10, 0), "gemini", "flash", "chat", 1, 1).unwrap();
    // 13 months before 2026-11-28 is 2025-10-28: September's rows go, October 15th's day and minute go, its month (2025-10) stays.
    let gone = c.prune(at(2026, 11, 28, 12, 0));
    assert!(gone >= 3, "{gone}");
    assert_eq!(c.totals("gemini", Window::Month, at(2025, 9, 15, 0, 0)).requests, 0);
    assert_eq!(c.totals("gemini", Window::Day, at(2025, 10, 15, 0, 0)).requests, 0);
    assert_eq!(c.totals("gemini", Window::Month, at(2025, 10, 15, 0, 0)).requests, 1, "the month of the cutoff is kept");
    assert_eq!(c.totals("gemini", Window::Day, at(2026, 11, 28, 0, 0)).requests, 1);
    assert_eq!(c.rows(Window::Minute), 1);
}

fn hub() -> (tempfile::TempDir, Arc<UsageHub>, broadcast::Receiver<AgentEvent>) {
    let dir = tempfile::tempdir().unwrap();
    let providers = Arc::new(ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap());
    providers.add_from_template("gemini", None, None, Some("sk-SECRET-KEY-1234".into()), Default::default(), None).unwrap();
    providers.add_from_template("ollama", None, Some("http://127.0.0.1:11434/v1".into()), None, Default::default(), None).unwrap();
    let (tx, rx) = broadcast::channel(16);
    (dir, UsageHub::new(Counters::memory().unwrap(), providers, tx), rx)
}

#[test]
fn the_view_lists_every_provider_with_its_counters() {
    let (_d, hub, _rx) = hub();
    let t = at(2026, 11, 28, 9, 30);
    hub.count_at(t, "gemini", "gemini-3.8-flash", CallSource::Chat, &usage(100, 10));
    let v = hub.view_at(t);
    let rows = v["providers"].as_array().unwrap();
    assert_eq!(rows.iter().map(|p| p["id"].as_str().unwrap()).collect::<Vec<_>>(), ["gemini", "ollama"], "every provider, in the providers' order");
    let g = &rows[0];
    assert_eq!((g["today"]["requests"].as_u64(), g["today"]["inputTokens"].as_u64(), g["month"]["outputTokens"].as_u64()), (Some(1), Some(100), Some(10)));
    assert_eq!(g["models"][0]["model"], "gemini-3.8-flash");
    assert_eq!((g["local"].as_bool(), rows[1]["local"].as_bool()), (Some(false), Some(true)));
    assert_eq!(rows[1]["today"]["requests"], 0, "a provider that was not used has zeros");
}

#[test]
fn no_key_and_no_token_is_in_the_view() {
    let (_d, hub, _rx) = hub();
    hub.count_at(at(2026, 11, 28, 9, 30), "gemini", "m", CallSource::Chat, &usage(1, 1));
    let text = hub.view().to_string();
    for secret in ["SECRET", "sk-", "apiKey", "apiKeyMasked", "Bearer", "token\""] {
        assert!(!text.contains(secret), "{secret} is in {text}");
    }
}

#[tokio::test]
async fn the_event_comes_at_most_every_30_seconds() {
    let (_d, hub, mut rx) = hub();
    let t = at(2026, 11, 28, 9, 30);
    let seen = |rx: &mut broadcast::Receiver<AgentEvent>| {
        let mut n = 0;
        while let Ok(ev) = rx.try_recv() {
            assert!(matches!(ev, AgentEvent::UsageChanged { ref provider } if provider == "gemini"));
            n += 1;
        }
        n
    };
    hub.count_at(t, "gemini", "m", CallSource::Chat, &usage(1, 1));
    assert_eq!(seen(&mut rx), 1, "the first answer");
    hub.count_at(t + chrono::Duration::seconds(10), "gemini", "m", CallSource::Chat, &usage(1, 1));
    hub.count_at(t + chrono::Duration::seconds(29), "gemini", "m", CallSource::Chat, &usage(1, 1));
    assert_eq!(seen(&mut rx), 0, "nothing inside 30 s");
    hub.count_at(t + chrono::Duration::seconds(31), "gemini", "m", CallSource::Chat, &usage(1, 1));
    assert_eq!(seen(&mut rx), 1, "and again after 30 s");
    hub.count_at(t + chrono::Duration::seconds(32), "ollama", "m", CallSource::Chat, &usage(1, 1));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::UsageChanged { provider }) if provider == "ollama"), "each provider has its own clock");
}

#[test]
fn the_sink_counts_under_the_source_of_the_turn() {
    let (_d, hub, _rx) = hub();
    hub.counted("gemini", "m", CallSource::Task, &usage(7, 3));
    let now = Local::now();
    assert_eq!(hub.counters.by_source("gemini", now), [("task".to_string(), 1)]);
}
