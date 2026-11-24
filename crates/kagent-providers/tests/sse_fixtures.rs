//! Scenario S3.1: each SSE fixture (text + one tool call, arguments split across
//! chunks) parses into the expected ModelResponse.

use kagent_providers::adapters::{anthropic, openai_chat, openai_responses};
use kagent_providers::sse::SseParser;
use serde_json::json;

fn events(raw: &str) -> Vec<kagent_providers::sse::SseEvent> {
    // Feed in small chunks to prove chunk-boundary safety.
    let mut p = SseParser::new();
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    for chunk in bytes.chunks(17) {
        out.extend(p.feed(chunk));
    }
    out.extend(p.finish());
    out
}

#[test]
fn openai_chat_fixture() {
    let raw = include_str!("fixtures/openai_chat_tools.sse");
    let mut r = openai_chat::Reducer::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    for ev in events(raw) {
        if ev.data.trim() == "[DONE]" {
            continue;
        }
        r.feed(&ev, &tx);
    }
    while rx.try_recv().is_ok() {}
    let out = r.finish();
    assert_eq!(out.text, "The kernel is 7.0.0.43-generic.");
    assert_eq!(out.usage.input_tokens, 42);
    assert_eq!(out.usage.output_tokens, 17);
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].id, "call_abc123");
    assert_eq!(out.tool_calls[0].name, "system_info");
    assert_eq!(out.tool_calls[0].input, json!({"section": "full"}));
}

#[test]
fn anthropic_fixture() {
    let raw = include_str!("fixtures/anthropic_tools.sse");
    let mut r = anthropic::Reducer::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    for ev in events(raw) {
        r.feed(&ev, &tx);
    }
    let out = r.finish();
    assert_eq!(out.text, "Checking the system.");
    assert_eq!(out.usage.input_tokens, 25);
    assert_eq!(out.usage.output_tokens, 34);
    assert_eq!(out.stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].id, "toolu_1");
    assert_eq!(out.tool_calls[0].input, json!({"section": "full"}));
}

#[test]
fn responses_fixture() {
    let raw = include_str!("fixtures/responses_tools.sse");
    let mut r = openai_responses::Reducer::new();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    for ev in events(raw) {
        r.feed(&ev, &tx);
    }
    let out = r.finish();
    assert_eq!(out.text, "One moment, checking.");
    assert_eq!(out.usage.input_tokens, 15);
    assert_eq!(out.usage.output_tokens, 12);
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].id, "call_xyz");
    assert_eq!(out.tool_calls[0].input, json!({"section": "full"}));
}
