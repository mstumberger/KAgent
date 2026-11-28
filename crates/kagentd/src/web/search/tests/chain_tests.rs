//! The chain of engines (web.md section 3.1), against the loopback fake and a fake clock.

use std::time::Duration;

use async_trait::async_trait;
use futures::future::join_all;
use kagent_core::tool::ToolContext;
use serde_json::json;

use super::*;
use crate::web::chain::Clock;
use crate::web::page_reader::{PageReader, ReaderError, Rendered};

/// A reader that answers what a test says, and counts its calls.
struct FakeReader(std::sync::Mutex<Vec<String>>, Result<String, ReaderError>);

#[async_trait]
impl PageReader for FakeReader {
    async fn render(&self, url: &str) -> Result<Rendered, ReaderError> {
        self.0.lock().unwrap().push(url.to_string());
        self.1.clone().map(|html| Rendered { url: url.into(), html })
    }
}

const DUCK_THEN_BRAVE: &str = "/challenge";

fn chain_of(list: &[&str]) -> Value {
    json!({"search": {"engines": list}})
}

/// Scenario R1: a burst of five searches all answer, one after the other, 3 s apart.
#[tokio::test]
async fn a_burst_of_five_searches_all_answer() {
    let fake = fake().await;
    let (web, clock) = build_clocked(&fake, chain_of(&["duckduckgo"]), None, BOTH);
    let web = Arc::new(web);
    let queries = ["one", "two", "three", "four", "five"];
    let answers = join_all(queries.iter().map(|q| web.search_hits(q, 3))).await;
    for (q, answer) in queries.iter().zip(&answers) {
        let (engine, hits) = answer.as_ref().unwrap_or_else(|e| panic!("{q}: {e}"));
        assert_eq!((*engine, hits.len()), (Engine::DuckDuckGo, 3), "{q}");
    }
    assert_eq!(fake.seen("/html/").len(), 5);
    assert!(clock.now_ms() >= 12_000, "the fifth waited for its slot: {}", clock.now_ms());
}

/// Scenario R1: a challenge moves down the chain, the engine cools, the cooldown doubles and ends.
#[tokio::test]
async fn a_challenge_moves_down_the_chain_and_cools_the_engine() {
    let fake = fake().await;
    let (web, clock) = build_clocked(&fake, chain_of(&["duckduckgo", "brave"]), Some(KEY), (DUCK_THEN_BRAVE, "/brave"));
    let web = Arc::new(web);
    let out = call(&web, json!({"query": "first"})).await;
    assert!(!out.is_error && out.content.contains("Results of Brave for \"first\":"), "the engine that answered is named: {}", out.content);
    assert_eq!(fake.seen("/challenge").len(), 1);
    assert_eq!(web.chain.cooling(Engine::DuckDuckGo), Some(5));

    // Within the cooldown DuckDuckGo is not asked again.
    let out = call(&web, json!({"query": "second"})).await;
    assert!(out.content.contains("Results of Brave for \"second\""), "{}", out.content);
    assert_eq!(fake.seen("/challenge").len(), 1, "it cools");
    // After it, it is tried again, fails again, and the cooldown is twice as long.
    clock.advance(5 * 60_000);
    call(&web, json!({"query": "third"})).await;
    assert_eq!(fake.seen("/challenge").len(), 2, "the cooldown ended");
    assert_eq!(web.chain.cooling(Engine::DuckDuckGo), Some(10), "and doubled");
    clock.advance(10 * 60_000);
    assert_eq!(web.chain.cooling(Engine::DuckDuckGo), None);
}

/// Scenario R1: the cache answers without a request, whatever the case and the spacing of the query.
#[tokio::test]
async fn the_cache_answers_without_a_request() {
    let fake = fake().await;
    let (web, clock) = build_clocked(&fake, chain_of(&["duckduckgo"]), None, BOTH);
    let web = Arc::new(web);
    let first = call(&web, json!({"query": "Linux  Kernel"})).await;
    assert!(!first.content.contains("(from the cache)"));
    let again = call(&web, json!({"query": " linux kernel ", "limit": 1})).await;
    assert!(again.content.contains("Results of DuckDuckGo for \"linux kernel\" (from the cache):"), "{}", again.content);
    assert!(again.content.contains("[1] ") && !again.content.contains("[2] "), "the smaller limit cuts the kept results");
    assert_eq!(fake.seen("/html/").len(), 1);
    clock.advance(16 * 60_000);
    call(&web, json!({"query": "linux kernel"})).await;
    assert_eq!(fake.seen("/html/").len(), 2, "after 15 minutes it asks again");
}

/// Scenario R1: a fourth search of a session within 10 s is refused before it runs.
#[tokio::test]
async fn a_fourth_search_within_ten_seconds_is_refused() {
    let fake = fake().await;
    let (web, clock) = build_clocked(&fake, chain_of(&["duckduckgo"]), None, BOTH);
    let web = Arc::new(web);
    for q in ["a", "b", "c"] {
        assert!(!call(&web, json!({"query": q})).await.is_error);
    }
    let out = call(&web, json!({"query": "d"})).await;
    assert!(out.is_error && out.content.starts_with("not done: a fourth search within 10 seconds was not run. Next: read the results you already have first"), "{}", out.content);
    assert_eq!(fake.seen("/html/").len(), 3, "it did not run");
    // Pacing moved the clock past the window by itself (3 s apart): ask again, in another session as well.
    clock.advance(10_000);
    assert!(!call(&web, json!({"query": "d"})).await.is_error);
    let mut other = ToolContext::standalone(Duration::from_secs(20), 10_000);
    other.session_id = "another".into();
    for q in ["e", "f", "g"] {
        assert!(!web.search_tool().call(json!({"query": q}), other.clone()).await.unwrap().is_error);
    }
}

/// An old settings file with the single engine means that engine, then the fallbacks.
#[tokio::test]
async fn an_old_single_engine_setting_falls_back_to_duckduckgo() {
    let fake = fake().await;
    // Brave has no key here: it is skipped without a cooldown and DuckDuckGo answers.
    let web = Arc::new(build(&fake, json!({"search": {"engine": "brave"}}), None, BOTH));
    let out = call(&web, json!({"query": "linux kernel"})).await;
    assert!(out.content.contains("Results of DuckDuckGo"), "{}", out.content);
    assert_eq!(web.chain.cooling(Engine::Brave), None, "a missing key is no failure");
    let web = Arc::new(build(&fake, json!({"search": {"engine": "brave"}}), Some(KEY), BOTH));
    assert!(call(&web, json!({"query": "linux kernel"})).await.content.contains("Results of Brave"), "the old engine is first");
}

/// The last fallback: DuckDuckGo's page read by the reader and parsed by the same parser.
#[tokio::test]
async fn the_reader_is_the_last_fallback_and_a_challenge_there_fails_the_chain() {
    let fake = fake().await;
    let reader = Arc::new(FakeReader(Default::default(), Ok(RESULTS.into())));
    let (web, _) = build_clocked(&fake, chain_of(&["duckduckgo", "reader"]), None, (DUCK_THEN_BRAVE, "/brave"));
    let web = Arc::new(web.with_page_reader(reader.clone()));
    let out = call(&web, json!({"query": "linux kernel"})).await;
    assert!(out.content.contains("Results of the reader (DuckDuckGo) for \"linux kernel\":") && out.content.contains("[1] The Linux Kernel Archives"), "{}", out.content);
    assert_eq!(reader.0.lock().unwrap().len(), 1);
    assert!(reader.0.lock().unwrap()[0].ends_with("/html/?q=linux+kernel"), "{:?}", reader.0.lock().unwrap());

    let blocked = Arc::new(FakeReader(Default::default(), Err(ReaderError::Challenge("https://x.example/".into()))));
    let (web, _) = build_clocked(&fake, chain_of(&["duckduckgo", "reader"]), None, (DUCK_THEN_BRAVE, "/brave"));
    let web = Arc::new(web.with_page_reader(blocked));
    let out = call(&web, json!({"query": "linux kernel"})).await;
    assert!(failed_with(&out, "DuckDuckGo: DuckDuckGo asked for a human check") && out.content.contains("the reader (DuckDuckGo): DuckDuckGo asked for a human check"), "{}", out.content);
    // No reader at all: the engine is skipped, without a cooldown.
    let (web, _) = build_clocked(&fake, chain_of(&["reader"]), None, BOTH);
    let out = call(&Arc::new(web), json!({"query": "x"})).await;
    assert!(failed_with(&out, "the reader is off or not available"), "{}", out.content);
}
