//! Scenarios S0-S1 items 5-7: the loop's parallel scheduling, denial handling,
//! cancellation and history repair, driven by a scripted fake model (the rig is in
//! `common/`); and the fallback of a chat model that cannot answer (S40).

mod common;
use common::*;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kagent_core::agent::{AgentConfig, SendOutcome, TurnOpts, TurnResult};
use kagent_core::model::ModelResponse;
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{AgentEvent, Message, Role, ToolCallStatus, TurnStatus};

// ------------------------------------------------------------------ scenarios

#[tokio::test]
async fn parallel_reads_then_denied_mutation() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let second = ModelResponse { text: "all done".into(), ..Default::default() };
    let first = ModelResponse {
        tool_calls: vec![call("1", "probe_a"), call("2", "probe_b"), call("3", "probe_c")],
        ..Default::default()
    };
    let mut r = rig(vec![first, second], spans.clone()).await;

    // Deny the approval as soon as it is asked.
    let broker = r.broker.clone();
    let mut erx = r.events.resubscribe();
    tokio::spawn(async move {
        let ev = wait_event(&mut erx, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await;
        if let AgentEvent::ApprovalRequested(req) = ev {
            broker.resolve(&req.request_id, ApprovalDecision::Deny { reason: None }, kagent_core::permission::ApprovalVia::Click);
        }
    });

    let outcome = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    assert!(matches!(outcome, SendOutcome::Started { .. }));
    let fin = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    let AgentEvent::TurnFinished { status, .. } = fin else { panic!() };
    assert_eq!(status, kagent_core::protocol::TurnStatus::Completed);

    // Two read-only probes overlapped (ran in parallel).
    let s = spans.lock().unwrap();
    let a = s.entries.iter().find(|e| e.0 == "probe_a").expect("probe_a ran");
    let b = s.entries.iter().find(|e| e.0 == "probe_b").expect("probe_b ran");
    assert!(b.1 < a.2 && a.1 < b.2, "read-only calls must overlap: a {:?}-{:?} b {:?}-{:?}", a.1, a.2, b.1, b.2);
    drop(s);

    // Second model request: assistant message followed by 3 tool results in call order.
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let tail = &requests[1][requests[1].len() - 4..];
    assert_eq!(tail[0].role, Role::Assistant);
    assert_eq!(tail[0].tool_calls.len(), 3);
    for (i, m) in tail[1..].iter().enumerate() {
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some(["1", "2", "3"][i]));
    }
    assert!(tail[3].is_error);
    assert_eq!(tail[3].content, "not executed: the user denied this action");
}

#[tokio::test]
async fn cancel_during_approval_leaves_no_dangling_call() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let second = ModelResponse { text: "unreached".into(), ..Default::default() };
    let first = ModelResponse { tool_calls: vec![call("1", "probe_c")], ..Default::default() };
    let mut r = rig(vec![first, second], spans).await;

    let _ = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap();
    let ev = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::ApprovalRequested(_))).await;
    let AgentEvent::ApprovalRequested(_req) = ev else { panic!() };
    // Cancel while the approval is pending; never resolve it.
    assert!(r.agent.cancel(&r.session).await);
    let fin = wait_event(&mut r.events, 30, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    let AgentEvent::TurnFinished { status, .. } = fin else { panic!() };
    assert_eq!(status, kagent_core::protocol::TurnStatus::Cancelled);

    let messages = r.store.load_messages(&r.session).await.unwrap();
    let assistant = messages.iter().find(|m| m.role == Role::Assistant && !m.tool_calls.is_empty()).expect("assistant with calls");
    for c in &assistant.tool_calls {
        let has_result = messages.iter().any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(&c.id));
        assert!(has_result, "call {} must have a result", c.id);
    }
    assert_eq!(r.model.requests.lock().unwrap().len(), 1, "no second model request after cancel");
}

#[tokio::test]
async fn sanitize_history_repairs_dangling_call() {
    let mut messages = vec![Message::user("hi")];
    let mut assistant = Message::assistant("working", vec![call("1", "probe_a")]);
    assistant.provider_state = None;
    messages.push(assistant);
    messages.push(Message::user("hello again"));

    let fixed = kagent_core::agent::sanitize_history(&messages);
    // user, assistant, synthetic tool result, user
    assert_eq!(fixed.len(), 4);
    assert_eq!(fixed[2].role, Role::Tool);
    assert_eq!(fixed[2].tool_call_id.as_deref(), Some("1"));
    assert!(fixed[2].is_error);

    // A clean history passes through unchanged.
    let with_result = kagent_core::agent::sanitize_history(&fixed);
    assert_eq!(with_result.len(), 4);
}

#[tokio::test]
async fn a_turn_block_reaches_every_request_of_its_turn_only_and_is_not_stored() {
    // Scenario S20 item 5, the loop's part: `/skill request` gives the turn the skill body.
    let block = "\nSkill 'alpha' (named by the user):\nRun the checks.\n";
    let spans = Arc::new(Mutex::new(Spans::default()));
    let first = ModelResponse { tool_calls: vec![call("1", "probe_a")], ..Default::default() };
    let second = ModelResponse { text: "done".into(), ..Default::default() };
    let r = rig(vec![first, second, ModelResponse { text: "plain".into(), ..Default::default() }], spans).await;

    let outcome = r.agent
        .send_with(&r.session, "/alpha check it".into(), Default::default(), TurnOpts { system_suffix: Some(block.to_string()), ..Default::default() })
        .await
        .unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!("nothing runs, so the turn starts") };
    let result = handle.wait().await;
    assert_eq!(result.status, kagent_core::protocol::TurnStatus::Completed, "{:?}", result.error);

    // Every model request of the turn carried the block, at the end of the prompt.
    let systems = r.model.systems.lock().unwrap().clone();
    assert_eq!(systems.len(), 2, "a tool call, then the answer");
    assert!(systems.iter().all(|s| s.ends_with(block)), "{systems:?}");
    // The stored user message is only the text; the body is nowhere in the session.
    let messages = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(messages[0].content, "/alpha check it");
    assert!(!messages.iter().any(|m| m.content.contains("Run the checks.")), "the body is not stored");

    // The block does not outlive its turn.
    let outcome = r.agent.send(&r.session, "and now?".into(), Default::default()).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!() };
    handle.wait().await;
    let systems = r.model.systems.lock().unwrap().clone();
    assert_eq!(systems.len(), 3, "two requests, then one for the plain turn");
    assert!(!systems[2].contains("Run the checks."), "{systems:?}");
}

#[tokio::test]
async fn a_block_of_input_queued_behind_a_running_turn_joins_its_next_request() {
    let block = "\nSkill 'beta' (named by the user):\nBeta steps.\n";
    let spans = Arc::new(Mutex::new(Spans::default()));
    // The first call runs 150 ms (`probe_a`): long enough to queue a second message behind it.
    let first = ModelResponse { tool_calls: vec![call("1", "probe_a")], ..Default::default() };
    let second = ModelResponse { text: "both done".into(), ..Default::default() };
    let r = rig(vec![first, second], spans).await;

    let outcome = r.agent.send(&r.session, "first".into(), Default::default()).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!() };
    // Let the turn make its first request (the tool call then runs 150 ms).
    let deadline = Instant::now() + Duration::from_secs(5);
    while r.model.systems.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the first request never came");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let queued = r.agent
        .send_with(&r.session, "/beta second".into(), Default::default(), TurnOpts { system_suffix: Some(block.to_string()), ..Default::default() })
        .await
        .unwrap();
    assert!(matches!(queued, SendOutcome::Queued), "a turn is running");
    let result = handle.wait().await;
    assert_eq!(result.status, kagent_core::protocol::TurnStatus::Completed, "{:?}", result.error);

    let systems = r.model.systems.lock().unwrap().clone();
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(systems.len(), 2, "the queued message steers the running turn; no third turn");
    assert!(!systems[0].contains("Beta steps."), "not before the message arrived");
    assert!(systems[1].ends_with(block), "{}", systems[1]);
    // Every request that sees the message has the block, and no other does.
    for (messages, system) in requests.iter().zip(&systems) {
        let seen = messages.iter().any(|m| m.role == Role::User && m.content == "/beta second");
        assert_eq!(system.contains("Beta steps."), seen);
    }
}

// Silence the unused warning for ToolCallStatus import used via full path.
#[allow(dead_code)]
fn _touch() -> Option<ToolCallStatus> {
    None
}

#[tokio::test]
async fn disabled_tool_group_is_refused_not_just_hidden() {
    let spans = Arc::new(Mutex::new(Spans::default()));
    let r = rig(vec![], spans.clone()).await;
    // probe_* tools map to group "other"; enabling only "system" disables them.
    r.agent.set_tool_groups(Some(vec!["system".into()]));
    let cancel = tokio_util::sync::CancellationToken::new();
    let (msg, status) = r
        .agent
        .execute_call(&r.session, "t", &call("x", "probe_a"), &cancel, kagent_core::tool::PrivilegeMethod::None)
        .await;
    assert_eq!(status, kagent_core::protocol::ToolCallStatus::Denied);
    assert!(msg.content.contains("disabled"), "{}", msg.content);
    assert!(spans.lock().unwrap().entries.is_empty(), "disabled tool must not run");
    // Re-enabling all groups lets it run.
    r.agent.set_tool_groups(None);
    let (_, status) = r
        .agent
        .execute_call(&r.session, "t", &call("y", "probe_a"), &cancel, kagent_core::tool::PrivilegeMethod::None)
        .await;
    assert_eq!(status, kagent_core::protocol::ToolCallStatus::Ok);
}

// ------------------------------------------------------------------ fallback (S40, models.md section 5)

/// One turn on a model that answers from `script`, with `fake::backup` as the fallback
/// when `with_fallback` (the daemon hands none when the fallback cannot be used). The
/// result, what the turn emitted, and the model that saw the requests.
async fn fallback_turn(script: Vec<Result<ModelResponse, String>>, with_fallback: bool) -> (TurnResult, Vec<AgentEvent>, ScriptedModel) {
    let cfg = AgentConfig {
        fallback_model_ref: if with_fallback { "fake::backup".into() } else { String::new() },
        fallback_context_tokens: 8_192,
        fallback_max_output: Some(999),
        ..common::config()
    };
    let r = common::rig_with(ScriptedModel::scripted(script), Arc::new(Mutex::new(Spans::default())), cfg).await;
    let mut events = r.events.resubscribe();
    let outcome = r.agent.send(&r.session, "hello".into(), Default::default()).await.unwrap();
    let SendOutcome::Started { handle, .. } = outcome else { panic!("nothing runs, so the input starts a turn") };
    let result = handle.wait().await;
    let mut emitted = Vec::new();
    while let Ok(ev) = events.try_recv() {
        emitted.push(ev);
    }
    (result, emitted, r.model.clone())
}

fn notices(events: &[AgentEvent]) -> Vec<String> {
    events.iter().filter_map(|e| if let AgentEvent::Notice { text, .. } = e { Some(text.clone()) } else { None }).collect()
}

#[tokio::test]
async fn a_capacity_error_switches_to_the_fallback_as_before() {
    let (result, events, model) = fallback_turn(vec![Err("503 Service Unavailable".into()), Ok(ModelResponse { text: "ok".into(), ..Default::default() })], true).await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    let seen = model.model_refs.lock().unwrap().clone();
    assert_eq!(seen, [("fake::test".to_string(), None), ("fake::backup".to_string(), Some(999))], "the rest of the turn goes to the fallback, with its output limit");
    assert_eq!(model.requests.lock().unwrap().len(), 2, "nothing to compact in a new session: the fallback at once");
    assert_eq!(notices(&events), ["fake::test is busy - answering with fake::backup"]);
}

#[tokio::test]
async fn without_a_fallback_the_turn_fails_with_the_models_own_error() {
    // Scenario S40 item 5: the fallback's provider is off, so the daemon hands no fallback.
    let error = "provider error 429: You exceeded your current quota, please check your plan and billing details";
    let (result, events, model) = fallback_turn(vec![Err(error.into())], false).await;
    assert_eq!(result.status, TurnStatus::Failed);
    let said = result.error.as_deref().unwrap_or_default();
    assert!(said.contains("exceeded your current quota"), "the model's own error: {said}");
    assert!(!said.contains("disabled"), "never the fallback's provider: {said}");
    assert_eq!(model.model_refs.lock().unwrap().iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["fake::test"], "one request");
    assert!(notices(&events).is_empty(), "nothing was switched to");
}

#[tokio::test]
async fn the_notice_of_a_switch_says_why_the_model_was_left() {
    let backup = " - answering with fake::backup";
    for (error, why) in [
        ("provider error 429: resource exhausted", "is out of quota"),
        ("You exceeded your current quota", "is out of quota"),
        // Quota is named first, a rejected key as before, busy otherwise.
        ("429: invalid api key", "is out of quota"),
        ("401 Unauthorized: invalid API key provided", "rejected the API key (Providers)"),
        ("503 Service Unavailable", "is busy"),
        ("The model is overloaded", "is busy"),
    ] {
        let (result, events, model) = fallback_turn(vec![Err(error.into()), Ok(ModelResponse { text: "ok".into(), ..Default::default() })], true).await;
        assert_eq!(result.status, TurnStatus::Completed, "{error}: {:?}", result.error);
        assert_eq!(model.model_refs.lock().unwrap().len(), 2, "{error}: the fallback answers");
        assert_eq!(notices(&events), [format!("fake::test {why}{backup}")], "{error}");
    }
}

/// v0.0.2 section 4: a model that answers with neither text nor a tool call is asked once
/// more; twice empty, the turn fails with a notice instead of "completed" with nothing.
#[tokio::test]
async fn an_empty_answer_is_asked_again_once_then_the_turn_fails_with_a_notice() {
    let empty = ModelResponse::default();
    // Empty, then an answer: the retry is invisible and the turn completes.
    let r = rig(vec![empty.clone(), ModelResponse { text: "here it is".into(), ..Default::default() }], Arc::new(Mutex::new(Spans::default()))).await;
    let SendOutcome::Started { handle, .. } = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    let result = handle.wait().await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    assert_eq!(r.model.requests.lock().unwrap().len(), 2, "asked twice");
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored.iter().filter(|m| m.role == Role::Assistant).map(|m| m.content.as_str()).collect::<Vec<_>>(), ["here it is"], "the empty answer is not stored");

    // Empty twice: failed, with the reason, and no third request.
    let r = rig(vec![empty.clone(), empty.clone(), empty], Arc::new(Mutex::new(Spans::default()))).await;
    let SendOutcome::Started { handle, .. } = r.agent.send(&r.session, "go".into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    let result = handle.wait().await;
    assert_eq!((result.status, result.error.as_deref()), (TurnStatus::Failed, Some(kagent_core::agent::NO_ANSWER)));
    assert_eq!(r.model.requests.lock().unwrap().len(), 2);
}

// ------------------------------------------------------------------ the conversation's own model (S41, picker.md section 4)

fn ok_text(text: &str) -> Result<ModelResponse, String> {
    Ok(ModelResponse { text: text.into(), ..Default::default() })
}

/// What the daemon's source knows: `fake::big` (4096 window, 512 output, efforts low and
/// high), the default `fake::test` (no efforts), `fake::backup` (efforts low only) and a
/// model that is switched off.
fn known_models() -> kagent_core::agent::children::UsableModels {
    use kagent_core::agent::children::ModelInfo;
    Arc::new(|model_ref: &str| match model_ref {
        "fake::big" => Ok(ModelInfo { context_tokens: 4096, max_output_tokens: Some(512), reasoning_levels: vec!["low".into(), "high".into()] }),
        "fake::test" => Ok(ModelInfo { context_tokens: 8192, max_output_tokens: Some(100), reasoning_levels: vec![] }),
        "fake::backup" => Ok(ModelInfo { context_tokens: 8192, max_output_tokens: Some(999), reasoning_levels: vec!["low".into()] }),
        "fake::off" => Err("model switched off"),
        _ => Err("unknown model"),
    })
}

/// One turn of a session that chose `model` and `level`, with the source when `source`.
async fn choice_turn(model: Option<&str>, level: Option<&str>, source: bool, script: Vec<Result<ModelResponse, String>>) -> (TurnResult, Vec<AgentEvent>, ScriptedModel) {
    let cfg = AgentConfig { fallback_model_ref: "fake::backup".into(), fallback_context_tokens: 8_192, fallback_max_output: Some(999), context_tokens: 8192, ..common::config() };
    let r = common::rig_with(ScriptedModel::scripted(script), Arc::new(Mutex::new(Spans::default())), cfg).await;
    if source {
        r.agent.set_usable_models(known_models());
    }
    r.agent.set_model_choice(&r.session, model.map(Into::into), level.map(Into::into)).await.unwrap();
    let before = r.store.meta(&r.session).await.unwrap().updated_at;
    let mut events = r.events.resubscribe();
    let SendOutcome::Started { handle, .. } = r.agent.send(&r.session, "hello".into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    let result = handle.wait().await;
    assert_eq!(r.store.meta(&r.session).await.unwrap().model.as_deref(), model, "the choice is kept");
    assert!(before <= r.store.meta(&r.session).await.unwrap().updated_at);
    let mut emitted = Vec::new();
    while let Ok(ev) = events.try_recv() {
        emitted.push(ev);
    }
    (result, emitted, r.model.clone())
}

#[tokio::test]
async fn a_choice_replaces_the_model_the_output_limit_and_the_level_of_the_request() {
    let (result, events, model) = choice_turn(Some("fake::big"), Some("high"), true, vec![ok_text("ok")]).await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    assert_eq!(model.model_refs.lock().unwrap().clone(), [("fake::big".to_string(), Some(512))]);
    assert_eq!(model.levels.lock().unwrap().clone(), [Some("high".to_string())]);
    assert!(notices(&events).is_empty(), "{:?}", notices(&events));
}

#[tokio::test]
async fn a_choice_that_cannot_be_used_makes_the_default_answer_and_a_notice_says_why() {
    let (result, events, model) = choice_turn(Some("fake::off"), Some("high"), true, vec![ok_text("ok")]).await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    assert_eq!(model.model_refs.lock().unwrap().clone(), [("fake::test".to_string(), None)]);
    assert_eq!(
        notices(&events),
        ["fake::off cannot answer (model switched off) - answering with fake::test", "effort 'high' is not one fake::test offers (none) - answering with its default"],
        "the model, then the effort that the default does not offer"
    );
    assert_eq!(model.levels.lock().unwrap().clone(), [None]);
}

#[tokio::test]
async fn a_level_the_model_does_not_offer_is_not_sent_and_a_notice_says_so() {
    let (result, events, model) = choice_turn(Some("fake::big"), Some("max"), true, vec![ok_text("ok")]).await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    assert_eq!(model.levels.lock().unwrap().clone(), [None]);
    assert_eq!(notices(&events), ["effort 'max' is not one fake::big offers (low, high) - answering with its default"]);
}

#[tokio::test]
async fn a_choice_equal_to_the_default_gives_no_notice_and_its_level_is_checked_against_the_default() {
    let (_, events, model) = choice_turn(Some("fake::test"), None, true, vec![ok_text("ok")]).await;
    assert!(notices(&events).is_empty());
    assert_eq!(model.model_refs.lock().unwrap().clone(), [("fake::test".to_string(), None)]);
    // A level with no model of its own is for the default model.
    let (_, events, model) = choice_turn(None, Some("low"), true, vec![ok_text("ok")]).await;
    assert_eq!(notices(&events).len(), 1, "the default offers no efforts");
    assert_eq!(model.levels.lock().unwrap().clone(), [None]);
}

#[tokio::test]
async fn without_a_source_the_choice_replaces_the_model_alone_and_the_level_is_sent_as_stored() {
    let (_, events, model) = choice_turn(Some("fake::big"), Some("high"), false, vec![ok_text("ok")]).await;
    assert_eq!(model.model_refs.lock().unwrap().clone(), [("fake::big".to_string(), None)], "the window and the limit of the default stay");
    assert_eq!(model.levels.lock().unwrap().clone(), [Some("high".to_string())]);
    assert!(notices(&events).is_empty());
}

#[tokio::test]
async fn a_fallback_that_does_not_offer_the_level_gets_none() {
    // `fake::big` is out of quota; `fake::backup` offers low only.
    let (result, _, model) = choice_turn(Some("fake::big"), Some("high"), true, vec![Err("429 quota".into()), ok_text("ok")]).await;
    assert_eq!(result.status, TurnStatus::Completed, "{:?}", result.error);
    assert_eq!(model.model_refs.lock().unwrap().iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["fake::big", "fake::backup"]);
    assert_eq!(model.levels.lock().unwrap().clone(), [Some("high".to_string()), None]);
    let (_, _, model) = choice_turn(Some("fake::big"), Some("low"), true, vec![Err("429 quota".into()), ok_text("ok")]).await;
    assert_eq!(model.levels.lock().unwrap().clone(), [Some("low".to_string()), Some("low".to_string())], "a fallback that offers it keeps it");
}
