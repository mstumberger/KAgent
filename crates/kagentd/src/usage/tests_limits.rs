use chrono::{DateTime, Duration, Utc};
use kagent_core::model::CallSource;
use kagent_core::protocol::AgentEvent;
use kagent_providers::usage::{Level, Limit, Observation, Source, Unit, UsageSink};
use serde_json::json;
use tokio::sync::broadcast;

use super::limits::{tightest, LimitStore};
use super::tests::{at, hub, usage};

fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn limit(id: &str, used: u64, total: u64, reset: Option<&str>) -> Limit {
    Limit::new(id, id, Some(used), Some(total), Unit::Requests, reset.map(utc), Source::Provider)
}

fn seen(limits: Vec<Limit>, at: &str) -> Observation {
    Observation { status: 200, limits, seen: utc(at) }
}

#[test]
fn a_newer_answer_replaces_the_limit_with_the_same_id_and_keeps_the_others() {
    let s = LimitStore::default();
    let t = utc("2026-11-28T09:00:00Z");
    assert!(s.observe("a", vec![limit("requests", 1, 10, Some("2026-11-28T09:01:00Z")), limit("tokens", 5, 100, Some("2026-11-28T09:01:00Z"))], t), "new limits change the set");
    assert!(!s.observe("a", vec![limit("requests", 2, 10, Some("2026-11-28T09:01:00Z"))], t + Duration::seconds(5)), "the same level is no change");
    let got = s.limits("a", t + Duration::seconds(6));
    assert_eq!(got.iter().map(|l| (l.id.as_str(), l.used)).collect::<Vec<_>>(), [("requests", Some(2)), ("tokens", Some(5))]);
    assert!(s.observe("a", vec![limit("requests", 9, 10, Some("2026-11-28T09:01:00Z"))], t + Duration::seconds(7)), "fine to nearly out is a change");
    assert_eq!(s.checked("a"), Some(t + Duration::seconds(7)));
}

#[test]
fn a_limit_goes_when_its_reset_has_passed() {
    let s = LimitStore::default();
    s.observe("a", vec![limit("requests", 10, 10, Some("2026-11-28T09:00:30Z"))], utc("2026-11-28T09:00:00Z"));
    assert_eq!(s.limits("a", utc("2026-11-28T09:00:29Z")).len(), 1);
    assert!(s.limits("a", utc("2026-11-28T09:00:30Z")).is_empty(), "at the reset it is over");
    // An answer after the reset tells the change: the out limit went away.
    assert!(s.observe("a", vec![limit("tokens", 1, 100, None)], utc("2026-11-28T09:00:40Z")));
}

#[test]
fn a_limit_with_no_reset_goes_after_ten_minutes() {
    let s = LimitStore::default();
    let t = utc("2026-11-28T09:00:00Z");
    s.observe("a", vec![limit("quota", 5, 5, None)], t);
    assert_eq!(s.limits("a", t + Duration::minutes(9)).len(), 1);
    assert!(s.limits("a", t + Duration::minutes(10)).is_empty());
    s.observe("a", vec![limit("quota", 5, 5, None)], t + Duration::minutes(9));
    assert_eq!(s.limits("a", t + Duration::minutes(15)).len(), 1, "seen again: it stays");
}

#[test]
fn the_tightest_is_the_fullest_and_an_out_limit_wins() {
    let ls = vec![limit("a", 50, 100, None), limit("b", 95, 100, None), limit("c", 10, 10, None), limit("d", 1, 2, None)];
    assert_eq!(tightest(&ls).unwrap().id, "c");
    assert_eq!(tightest(&ls[..3]).unwrap().id, "c");
    assert_eq!(tightest(&ls[..2]).unwrap().id, "b");
    assert!(tightest(&[]).is_none());
    let unknown = Limit::new("rate", "Rate limit", None, None, Unit::Requests, None, Source::Provider);
    assert_eq!(tightest(&[limit("b", 95, 100, None), unknown]).unwrap().id, "rate", "out with no total counts as full");
}

#[tokio::test]
async fn a_change_of_level_is_told_at_once_and_a_repeat_is_not() {
    let (_d, hub, mut rx) = hub();
    hub.observed("gemini", seen(vec![limit("requests", 10, 100, Some("2026-11-28T10:00:00Z"))], "2026-11-28T09:00:00Z"));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::UsageChanged { provider }) if provider == "gemini"), "a limit appeared");
    hub.observed("gemini", seen(vec![limit("requests", 20, 100, Some("2026-11-28T10:00:00Z"))], "2026-11-28T09:00:05Z"));
    assert!(matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)), "fine to fine: nothing");
    hub.observed("gemini", seen(vec![limit("requests", 95, 100, Some("2026-11-28T10:00:00Z"))], "2026-11-28T09:00:06Z"));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::UsageChanged { .. })), "to nearly out, inside the 30 s, still at once");
    hub.observed("gemini", seen(vec![], "2026-11-28T09:00:07Z"));
    assert!(matches!(rx.try_recv(), Err(broadcast::error::TryRecvError::Empty)), "an answer with no limit says nothing");
}

#[test]
fn the_view_has_the_limits_and_the_tightest() {
    let (_d, hub, _rx) = hub();
    let now = chrono::Local::now();
    let reset = (Utc::now() + Duration::hours(1)).to_rfc3339();
    hub.observed("gemini", Observation { status: 200, limits: vec![limit("requests", 94, 100, Some(&reset)), limit("tokens", 1, 100, Some(&reset))], seen: Utc::now() });
    let v = hub.view_at(now);
    let g = &v["providers"][0];
    assert_eq!(g["limits"].as_array().unwrap().len(), 2);
    assert_eq!((g["tightest"]["id"].as_str(), g["tightest"]["level"].as_str(), g["tightest"]["source"].as_str()), (Some("requests"), Some("nearlyOut"), Some("provider")));
    assert!(g["checkedAt"].is_string());
    assert_eq!(hub.tightest_at("gemini", false, now).unwrap().id, "requests");
    assert!(v["providers"][1]["tightest"].is_null() && v["providers"][1]["limits"] == json!([]));
}

#[test]
fn a_local_provider_has_no_limits_whatever_it_says_or_the_owner_typed() {
    let (_d, hub, mut rx) = hub();
    hub.settings.write().unwrap().patch(json!({"usage": {"knownLimits": {"ollama": [{"window": "day", "unit": "requests", "total": 5}]}}})).unwrap();
    hub.observed("ollama", Observation { status: 429, limits: vec![limit("requests", 10, 10, None)], seen: Utc::now() });
    assert!(rx.try_recv().is_err(), "nothing was kept, nothing is told");
    let v = hub.view();
    assert_eq!((v["providers"][1]["limits"].clone(), v["providers"][1]["tightest"].clone()), (json!([]), json!(null)));
    assert!(hub.tightest_at("ollama", true, chrono::Local::now()).is_none());
}

#[test]
fn a_known_limit_is_counted_by_kagent_against_its_window() {
    let (_d, hub, _rx) = hub();
    hub.settings
        .write()
        .unwrap()
        .patch(json!({"usage": {"knownLimits": {"gemini": [
            {"window": "day", "unit": "requests", "total": 10},
            {"window": "month", "unit": "tokens", "total": 1000},
            {"window": "minute", "unit": "requests", "total": 2}]}}}))
        .unwrap();
    let t = at(2026, 11, 28, 9, 30);
    for _ in 0..8 {
        hub.count_at(t, "gemini", "m", CallSource::Chat, &usage(50, 10));
    }
    hub.count_at(t - chrono::Duration::days(1), "gemini", "m", CallSource::Chat, &usage(1, 1));
    let by = |id: &str| hub.provider_limits_at("gemini", false, t).into_iter().find(|l| l.id == id).unwrap();
    let day = by("known-day-requests");
    assert_eq!((day.used, day.total, day.level, day.source, day.unit), (Some(8), Some(10), Level::Close, Source::Kagent, Unit::Requests));
    assert_eq!(day.name, "Requests per day");
    assert!(day.resets().unwrap() > t.with_timezone(&Utc) && day.resets().unwrap() <= (t + chrono::Duration::hours(15)).with_timezone(&Utc), "the end of today, local time");
    let month = by("known-month-tokens");
    assert_eq!((month.used, month.unit), (Some(8 * 60 + 2), Unit::Tokens), "tokens are input and output, the month holds yesterday too");
    let minute = by("known-minute-requests");
    assert_eq!((minute.used, minute.level), (Some(8), Level::Out), "8 of 2 in this minute: out");
    assert_eq!(hub.view_at(t)["providers"][0]["knownLimits"].as_array().unwrap().len(), 3, "the typed limits are in the view");
    assert_eq!(hub.tightest_at("gemini", false, t).unwrap().id, "known-minute-requests");
    // An hour later it is a new minute: nothing in it.
    let later = hub.provider_limits_at("gemini", false, t + chrono::Duration::hours(1));
    assert_eq!(later.iter().find(|l| l.id == "known-minute-requests").unwrap().used, Some(0));
}

#[test]
fn no_key_or_header_value_is_in_the_view_with_limits_in_it() {
    let (_d, hub, _rx) = hub();
    hub.observed("gemini", Observation { status: 429, limits: vec![limit("requests", 5, 5, None)], seen: Utc::now() });
    let text = hub.view().to_string();
    for secret in ["SECRET", "sk-", "Bearer", "apiKey"] {
        assert!(!text.contains(secret), "{secret}");
    }
}

#[test]
fn the_provider_rows_gain_their_tightest_limit() {
    let (_d, hub, _rx) = hub();
    let reset = (Utc::now() + Duration::hours(1)).to_rfc3339();
    hub.observed("gemini", Observation { status: 200, limits: vec![limit("requests", 94, 100, Some(&reset))], seen: Utc::now() });
    let mut rows = vec![json!({"id": "gemini", "local": false}), json!({"id": "ollama", "local": true}), json!({"id": "other", "local": false})];
    hub.add_tightest(&mut rows, chrono::Local::now());
    assert_eq!((rows[0]["tightestLimit"]["id"].as_str(), rows[0]["tightestLimit"]["level"].as_str()), (Some("requests"), Some("nearlyOut")));
    assert!(rows[1]["tightestLimit"].is_null() && rows[2]["tightestLimit"].is_null());
}
