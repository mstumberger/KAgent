//! Scenarios S22 items 1 to 4 (compaction.md section 7) with a scripted model: a history
//! that overflows is summarized once and the request starts with the summary; a failing
//! summary falls back to the note; a small window without a fast model keeps the note;
//! the tokens the provider reports trigger the compaction; a capacity error is compacted
//! before the fallback model is used; a compaction by hand.

mod common;
use common::{ProbeTool, Spans};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{Agent, AgentConfig, AgentDeps, SendOutcome, TurnResult};
use kagent_core::compact::{CompactError, CompactionConfig, SUMMARY_SYSTEM};
use kagent_core::model::{ModelClient, ModelDelta, ModelOptions, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionEngine, PermissionMode, PermissionBroker};
use kagent_core::protocol::{AgentEvent, Message, Role, TurnStatus, Usage};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;

// ------------------------------------------------------------------ scripted model

/// One answer of the scripted model.
enum Reply {
    Text(&'static str),
    Call(&'static str),
    /// The request fails with this text.
    Fail(&'static str),
    /// A text answer whose usage reports so many prompt tokens.
    Reported(u64, &'static str),
}

/// A request the model was asked.
#[derive(Debug, Clone)]
struct Seen {
    model_ref: String,
    /// The summary request of compaction (no tools, its own system text).
    summary: bool,
    max_output: Option<u64>,
    messages: Vec<Message>,
}

/// Answers from a script, in order; records every request.
#[derive(Clone)]
struct Model {
    script: Arc<Mutex<Vec<Result<ModelResponse, String>>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

#[async_trait]
impl ModelClient for Model {
    async fn complete(&self, request: ModelRequest, deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        self.seen.lock().unwrap().push(Seen {
            model_ref: request.model_ref.clone(),
            summary: request.tools.is_empty() && request.system == SUMMARY_SYSTEM,
            max_output: request.options.max_output_tokens,
            messages: request.messages.clone(),
        });
        // The turn's text is what is streamed, not the response's text field.
        let reply = self.script.lock().unwrap().remove(0).map_err(anyhow::Error::msg)?;
        let _ = deltas.send(ModelDelta::Text(reply.text.clone()));
        Ok(reply)
    }
}

struct Rig {
    agent: Arc<Agent>,
    model: Model,
    store: SessionStore,
    events: broadcast::Receiver<AgentEvent>,
    session: String,
    _dir: tempfile::TempDir,
}

/// `common::config` for a window of `context_tokens` that reserves `max_output` tokens for the reply; a fallback model is set.
fn config(context_tokens: usize, max_output: u64) -> AgentConfig {
    AgentConfig {
        options: ModelOptions { max_output_tokens: Some(max_output), ..Default::default() },
        context_tokens,
        fallback_model_ref: "fake::backup".into(),
        ..common::config()
    }
}

async fn rig(script: Vec<Reply>, config: AgentConfig, compaction: CompactionConfig) -> Rig {
    let reply = |r: Reply| match r {
        Reply::Text(t) => Ok(ModelResponse { text: t.into(), ..Default::default() }),
        Reply::Call(tool) => Ok(ModelResponse { tool_calls: vec![common::call("1", tool)], ..Default::default() }),
        Reply::Reported(prompt, t) => Ok(ModelResponse { text: t.into(), usage: Usage { input_tokens: prompt, output_tokens: 5 }, ..Default::default() }),
        Reply::Fail(e) => Err(e.to_string()),
    };
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let session = store.create("t").await.unwrap().id;
    let (etx, erx) = broadcast::channel(256);
    let mut tools = ToolRegistry::new();
    let spans = Arc::new(Mutex::new(Spans::default()));
    tools.register(Arc::new(ProbeTool::ro("probe_a", &spans)));
    let model = Model { script: Arc::new(Mutex::new(script.into_iter().map(reply).collect())), seen: Arc::new(Mutex::new(Vec::new())) };
    let cfg = config;
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(model.clone()),
        tools: Arc::new(tools),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(etx.clone(), Duration::from_secs(120)),
        store: store.clone(),
        events: etx,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    agent.set_compaction_source(Arc::new(move || compaction.clone()));
    Rig { agent, model, store, events: erx, session, _dir: dir }
}

/// Send one message and wait for the turn; also what the turn emitted.
async fn turn(r: &Rig, text: &str) -> (TurnResult, Vec<AgentEvent>) {
    let mut events = r.events.resubscribe();
    let outcome = r.agent.send(&r.session, text.into(), Default::default()).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!("nothing runs, so the input starts a turn") };
    let result = handle.wait().await;
    let mut emitted = Vec::new();
    while let Ok(ev) = events.try_recv() {
        emitted.push(ev);
    }
    (result, emitted)
}

/// Finished turns: a question and an answer each.
fn history(turns: usize, q: usize, a: usize) -> Vec<Message> {
    (0..turns).flat_map(|i| [Message::user(format!("question {i} {}", "x".repeat(q))), Message::assistant(format!("answer {i} {}", "x".repeat(a)), vec![])]).collect()
}

/// Thirty finished turns of 25.5k tokens by estimate: under the soft line of a 32k window (80 percent) and over its budget once 8k output tokens are reserved.
const OVERFLOWING: fn() -> Vec<Message> = || history(30, 1_500, 1_000);
const SUMMARY_TEXT: &str = "SUMMARY: the user asked thirty questions.";

async fn too_short(r: &Rig) -> bool {
    matches!(r.agent.compact(&r.session).await, Err(CompactError::TooShort(_)))
}

impl Rig {
    fn seen(&self) -> Vec<Seen> {
        self.model.seen.lock().unwrap().clone()
    }
    async fn summary_upto(&self) -> Option<usize> {
        let meta = self.store.meta(&self.session).await.unwrap();
        assert_eq!(meta.summary_upto.is_some(), meta.summary.is_some(), "the two fields go together");
        meta.summary_upto
    }
}

// ------------------------------------------------------------------ scenarios

#[tokio::test]
async fn a_history_that_overflows_a_32k_window_is_summarized_once_and_the_request_starts_with_the_summary() {
    // Scenario S22 item 1.
    let r = rig(vec![Reply::Text(SUMMARY_TEXT), Reply::Call("probe_a"), Reply::Text("done"), Reply::Text("again")], config(32_768, 8_192), CompactionConfig::default()).await;
    r.store.append(&r.session, &OVERFLOWING()).await.unwrap();
    let (result, _) = turn(&r, "question 30").await;
    assert_eq!((result.status.clone(), result.text.as_str()), (TurnStatus::Completed, "done"), "{:?}", result.error);

    let seen = r.seen();
    assert_eq!(seen.iter().map(|s| s.summary).collect::<Vec<_>>(), [true, false, false], "one summarize call, then the two requests of the turn");
    let summary = &seen[0];
    assert_eq!((summary.model_ref.as_str(), summary.max_output), ("fake::test", Some(1_500)), "no fast model: the chat model summarizes, 1500 tokens at most");
    assert_eq!(summary.messages.len(), 15, "everything up to the cut point of `fit_to_budget` (room for the summary reserved), then the request for the summary");
    assert!(summary.messages[0].content.starts_with("question 0 ") && summary.messages[13].content.starts_with("answer 6 ") && summary.messages[14].content == kagent_core::compact::SUMMARY_ASK);
    for request in &seen[1..] {
        let first = &request.messages[0];
        assert_eq!(first.role, Role::User);
        assert!(first.content.starts_with(&format!("Conversation so far (summary of the first 14 messages):\n{SUMMARY_TEXT}")), "{}", first.content);
        assert!(request.messages[1].content.starts_with("question 7 "), "then the stored messages from 14 on");
        assert!(!request.messages.iter().any(|m| m.content.starts_with("[Note:")), "nothing was trimmed");
    }

    // The session file is unchanged by the compaction; the meta holds the summary.
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored.len(), 64, "60 before, then the question, the step with the tool call, its result and the answer: the compaction adds nothing");
    assert!(stored.iter().all(|m| !m.content.contains("SUMMARY") && !m.content.starts_with("Conversation so far")), "the synthetic message is never written");
    assert_eq!(r.store.meta(&r.session).await.unwrap().summary.as_deref(), Some(SUMMARY_TEXT));
    assert_eq!(r.summary_upto().await, Some(14));
    assert_eq!(stored[14].content.starts_with("question 7 "), true, "the summary ends before a user message");

    // The next turn sends the same summary and does not summarize again.
    let (result, _) = turn(&r, "question 31").await;
    assert_eq!(result.text, "again");
    let next = r.seen()[3..].to_vec();
    assert_eq!(next.iter().map(|s| s.summary).collect::<Vec<_>>(), [false]);
    assert!(next[0].messages[0].content.starts_with("Conversation so far (summary of the first 14 messages)"));
}

#[tokio::test]
async fn a_16k_window_without_a_fast_model_keeps_the_note_and_makes_no_summary() {
    // Scenario S22 item 2.
    let r = rig(vec![Reply::Text("done")], config(16_384, 8_192), CompactionConfig::default()).await;
    r.store.append(&r.session, &OVERFLOWING()).await.unwrap();
    let (result, _) = turn(&r, "question 30").await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = r.seen();
    assert_eq!(seen.len(), 1, "no summarize call");
    assert!(seen[0].messages[0].content.starts_with("[Note: "), "{}", seen[0].messages[0].content);
    assert!(seen[0].messages[0].content.contains("earlier messages were omitted to fit the context window"));
    assert_eq!(r.summary_upto().await, None);
    // With a fast model the same history is summarized.
    let fast = CompactionConfig { fast_model_ref: "fast::f".into(), ..Default::default() };
    let r = rig(vec![Reply::Text("SUMMARY"), Reply::Text("done")], config(16_384, 8_192), fast).await;
    r.store.append(&r.session, &OVERFLOWING()).await.unwrap();
    let (result, _) = turn(&r, "question 30").await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = r.seen();
    assert_eq!(seen[0].model_ref, "fast::f", "the fast model summarizes");
    assert!(seen[1].messages[0].content.starts_with("Conversation so far (summary of the first"), "no note");
    let upto = r.summary_upto().await.unwrap();
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert!(upto > 14 && stored[upto].role == Role::User && stored[upto].content.starts_with("question "), "the small window keeps less: the summary ends before {upto}");
}

#[tokio::test]
async fn a_failing_summarize_call_falls_back_to_the_note() {
    // Scenario S22 item 3.
    for (script, what) in [
        (vec![Reply::Fail("summary: connection reset"), Reply::Text("done")], "an error"),
        (vec![Reply::Text(""), Reply::Text("done")], "empty text"),
        (vec![Reply::Text("   \n"), Reply::Text("done")], "blank text"),
    ] {
        let r = rig(script, config(32_768, 8_192), CompactionConfig::default()).await;
        r.store.append(&r.session, &OVERFLOWING()).await.unwrap();
        let (result, _) = turn(&r, "question 30").await;
        assert_eq!((result.status.clone(), result.text.as_str()), (TurnStatus::Completed, "done"), "{what}: {:?}", result.error);
        let seen = r.seen();
        assert_eq!(seen.iter().map(|s| s.summary).collect::<Vec<_>>(), [true, false], "{what}: one try, then the request");
        assert!(seen[1].messages[0].content.starts_with("[Note: "), "{what}: the turn continues with the note");
        assert_eq!(r.summary_upto().await, None, "{what}: nothing is stored");
    }
}

#[tokio::test]
async fn a_capacity_error_compacts_once_before_the_fallback_model_is_used() {
    // Scenario S22 item 4.
    let r = rig(
        vec![Reply::Fail("503 Service Unavailable"), Reply::Text("SUMMARY: two questions were answered."), Reply::Fail("429 Too Many Requests"), Reply::Text("answer from the backup")],
        config(32_768, 8_192),
        CompactionConfig::default(),
    )
    .await;
    r.store.append(&r.session, &history(2, 10, 10)).await.unwrap();
    let (result, events) = turn(&r, "question 2").await;
    assert_eq!((result.status.clone(), result.text.as_str()), (TurnStatus::Completed, "answer from the backup"), "{:?}", result.error);

    let seen = r.seen();
    assert_eq!(seen.iter().map(|s| s.model_ref.as_str()).collect::<Vec<_>>(), ["fake::test", "fake::test", "fake::test", "fake::backup"], "the chat model, the summary, the retry, then the fallback");
    assert_eq!(seen.iter().map(|s| s.summary).collect::<Vec<_>>(), [false, true, false, false], "one compaction");
    assert_eq!(seen[2].messages[0].content, "Conversation so far (summary of the first 4 messages):\nSUMMARY: two questions were answered.", "the retry carries everything before the last user message, summarized");
    assert_eq!(seen[2].messages[1].content, "question 2");
    assert_eq!(r.summary_upto().await, Some(4));
    let notices: Vec<String> = events.iter().filter_map(|e| if let AgentEvent::Notice { text, .. } = e { Some(text.clone()) } else { None }).collect();
    assert_eq!(notices, ["fake::test is out of quota - answering with fake::backup"], "one notice, when the fallback comes: the retry failed with 429 (models.md section 5)");
}

#[tokio::test]
async fn a_capacity_error_with_nothing_to_compact_goes_to_the_fallback_at_once_and_fails_without_one() {
    // The first message of a session: nothing before it to summarize.
    let r = rig(vec![Reply::Fail("429 Too Many Requests"), Reply::Text("from the backup")], config(32_768, 8_192), CompactionConfig::default()).await;
    let (result, _) = turn(&r, "hello").await;
    assert_eq!(result.text, "from the backup");
    assert_eq!(r.seen().iter().map(|s| s.model_ref.as_str()).collect::<Vec<_>>(), ["fake::test", "fake::backup"], "no summary call, no retry");

    // A summary that fails does not hold the fallback up either.
    let r = rig(vec![Reply::Fail("503 Service Unavailable"), Reply::Fail("summary down"), Reply::Text("from the backup")], config(32_768, 8_192), CompactionConfig::default()).await;
    r.store.append(&r.session, &history(2, 10, 10)).await.unwrap();
    let (result, _) = turn(&r, "question 2").await;
    assert_eq!(result.text, "from the backup");
    assert_eq!(r.seen().iter().map(|s| (s.model_ref.as_str(), s.summary)).collect::<Vec<_>>(), [("fake::test", false), ("fake::test", true), ("fake::backup", false)]);

    // With compaction switched off a capacity error never summarizes: the fallback at once.
    let off = CompactionConfig { enabled: false, ..Default::default() };
    let r = rig(vec![Reply::Fail("503 Service Unavailable"), Reply::Text("from the backup")], config(32_768, 8_192), off).await;
    r.store.append(&r.session, &history(2, 10, 10)).await.unwrap();
    let (result, _) = turn(&r, "question 2").await;
    assert_eq!(result.text, "from the backup");
    assert_eq!(r.seen().iter().map(|s| (s.model_ref.as_str(), s.summary)).collect::<Vec<_>>(), [("fake::test", false), ("fake::backup", false)], "no summary call with compaction off");

    // No fallback model either: the error ends the turn, as before.
    let mut plain = config(32_768, 8_192);
    plain.fallback_model_ref = String::new();
    let r = rig(vec![Reply::Fail("503 Service Unavailable")], plain, CompactionConfig::default()).await;
    let (result, _) = turn(&r, "hello").await;
    assert_eq!(result.status, TurnStatus::Failed);
    assert!(result.error.as_deref().unwrap_or_default().contains("503"), "{:?}", result.error);
}

#[tokio::test]
async fn the_prompt_tokens_the_provider_reports_trigger_the_compaction() {
    // Compaction.md section 6: 8k tokens by estimate, 30k by what the provider counted, in a 32k window.
    let r = rig(
        vec![Reply::Reported(30_000, "first answer"), Reply::Text("SUMMARY: three questions."), Reply::Text("second answer"), Reply::Text("third answer")],
        config(32_768, 8_192),
        CompactionConfig::default(),
    )
    .await;
    r.store.append(&r.session, &history(3, 8_000, 0)).await.unwrap();
    let (result, _) = turn(&r, "question 3").await;
    assert_eq!(result.text, "first answer");
    let seen = r.seen();
    assert_eq!((seen.len(), seen[0].messages.len()), (1, 7), "a quarter of the window by estimate: sent as it is");

    // The count is over the soft line: everything before the last user message is summarized.
    let (result, _) = turn(&r, "question 4").await;
    assert_eq!(result.text, "second answer");
    let seen = r.seen();
    assert_eq!(seen[1..].iter().map(|s| s.summary).collect::<Vec<_>>(), [true, false]);
    assert_eq!(seen[2].messages[0].content, "Conversation so far (summary of the first 8 messages):\nSUMMARY: three questions.");
    assert_eq!(r.summary_upto().await, Some(8));

    // The count was reset with the compaction: the next request does not summarize again.
    let (result, _) = turn(&r, "question 5").await;
    assert_eq!(result.text, "third answer");
    assert_eq!(r.seen()[3..].iter().map(|s| s.summary).collect::<Vec<_>>(), [false], "one request, no summary");
}

#[tokio::test]
async fn a_compaction_by_hand_summarizes_everything_before_the_last_user_message() {
    // By hand it works with the automatic compaction switched off.
    let off = CompactionConfig { enabled: false, ..Default::default() };
    let r = rig(vec![Reply::Text("one"), Reply::Text("two"), Reply::Text("SUMMARY"), Reply::Text("after"), Reply::Text("SUMMARY, again"), Reply::Text("newer")], config(32_768, 8_192), off.clone()).await;
    assert!(too_short(&r).await, "an empty session");
    let _ = turn(&r, "one").await;
    assert!(too_short(&r).await, "fewer than four stored messages");
    let _ = turn(&r, "two").await;

    let view = r.agent.compact(&r.session).await.unwrap();
    assert_eq!((view.summary.as_str(), view.kept_messages), ("SUMMARY", 2), "the first turn is summarized, the last stays");
    assert_eq!(r.summary_upto().await, Some(2));
    let calls = r.seen().len();
    let again = r.agent.compact(&r.session).await.unwrap();
    assert_eq!((again.summary.as_str(), r.seen().len()), ("SUMMARY", calls), "nothing new before the last user message: the summary there is, without a model call");

    // The next request starts with it, and the automatic compaction stays off.
    let (result, _) = turn(&r, "three").await;
    assert_eq!(result.text, "after");
    assert_eq!(r.seen()[calls].messages[0].content, "Conversation so far (summary of the first 2 messages):\nSUMMARY");
    assert_eq!(r.seen()[calls].messages[1].content, "two");

    // Again: the old summary and what came after it go into the new one.
    let view = r.agent.compact(&r.session).await.unwrap();
    assert_eq!((view.summary.as_str(), view.kept_messages, r.summary_upto().await), ("SUMMARY, again", 2, Some(4)));
    let seen = r.seen();
    let summary = seen.iter().rev().find(|s| s.summary).unwrap();
    assert!(summary.messages[0].content.starts_with("Summary of the earlier conversation:\nSUMMARY"), "the old summary is the first message of the request");
    assert_eq!(summary.messages.len(), 4, "the old summary, a question and its answer, then the request for the summary");
    assert_eq!(summary.messages[3].content, kagent_core::compact::SUMMARY_ASK);

    // A turn that only called a tool: nothing before its last user message.
    let r = rig(vec![Reply::Call("probe_a"), Reply::Text("done")], config(32_768, 8_192), off.clone()).await;
    let _ = turn(&r, "one").await;
    assert!(too_short(&r).await, "four stored messages, but nothing before the last user message");

    // A summary that fails changes nothing and says so.
    let r = rig(vec![Reply::Text("one"), Reply::Text("two"), Reply::Fail("summary down")], config(32_768, 8_192), off).await;
    let _ = turn(&r, "one").await;
    let _ = turn(&r, "two").await;
    let error = r.agent.compact(&r.session).await.unwrap_err();
    assert!(matches!(&error, CompactError::Failed(e) if e.contains("summary down")), "{error}");
    assert_eq!(r.summary_upto().await, None);
}
