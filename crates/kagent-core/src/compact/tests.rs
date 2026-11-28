use super::*;
use crate::context::{fit_to_budget, request_history, sanitize_history, HistorySummary};
use crate::protocol::ToolCall;
use serde_json::json;

/// A window where 1 percent is 1000 tokens, 3000 chars.
const WIDE: usize = 100_000;
/// One percent of it, in messages: `n` of `chars` chars (16 chars of framing more each).
fn flat(n: usize, chars: usize) -> Vec<Message> {
    (0..n)
        .map(|i| if i % 2 == 0 { Message::user(format!("q{i} {}", "x".repeat(chars))) } else { Message::assistant(format!("a{i} {}", "x".repeat(chars)), vec![]) })
        .collect()
}

/// Whole turns with a tool call and its result (the cut must not separate them).
fn tool_turns(n: usize, chars: usize) -> Vec<Message> {
    let mut v = Vec::new();
    for t in 0..n {
        v.push(Message::user(format!("q{t} {}", "x".repeat(chars))));
        let call = ToolCall { id: format!("c{t}"), name: "system_info".into(), input: json!({}) };
        v.push(Message::assistant("", vec![call.clone()]));
        v.push(Message::tool_result(&call, "y".repeat(chars), false));
        v.push(Message::assistant(format!("a{t}"), vec![]));
    }
    v
}

fn plan_with(tail: &[Message], summary: Option<&Message>, context_tokens: usize, budget_tokens: usize, last_input_tokens: u64, compaction: CompactionConfig) -> Plan {
    should_compact(&History { summary, tail: &sanitize_history(tail) }, &Trigger { compaction, context_tokens, budget_tokens, last_input_tokens })
}

fn plan(tail: &[Message], context_tokens: usize, budget_tokens: usize, last_input_tokens: u64) -> Plan {
    plan_with(tail, None, context_tokens, budget_tokens, last_input_tokens, CompactionConfig::default())
}

/// What a request holds after a cut of `tail` (tokens): the new summary and the rest.
fn after(tail: &[Message], cut: usize, reserve_chars: usize, overhead: usize) -> usize {
    overhead + reserve_chars.div_ceil(CHARS_PER_TOKEN) + tail[cut..].iter().map(message_chars).sum::<usize>().div_ceil(CHARS_PER_TOKEN)
}

/// The cut is at a user message and brings the request back to `target`, and no further than that.
fn back_to(tail: &[Message], tail_cut: usize, window: usize, reserve: usize, overhead: usize) {
    assert!(tail_cut >= 2 && tail[tail_cut].role == Role::User, "the cut {tail_cut} is at a user message");
    let target = window * 60 / 100;
    assert!(after(tail, tail_cut, reserve, overhead) <= target, "back to 60 percent: {}", after(tail, tail_cut, reserve, overhead));
    assert!(after(tail, tail_cut - 2, reserve, overhead) > target, "but no further than that: {tail_cut}");
}

#[test]
fn soft_line_of_a_wide_window() {
    // 79 percent of the window: nothing.
    assert_eq!(plan(&flat(79, 3_000), WIDE, usize::MAX, 0), Plan::Keep);
    // 81 percent: summarized back to 60 percent.
    let tail = flat(81, 3_000);
    let Plan::Summarize { tail_cut } = plan(&tail, WIDE, usize::MAX, 0) else { panic!("81 percent of the window is summarized") };
    back_to(&tail, tail_cut, WIDE, 8_000, 0);
    assert!(tail_cut > 20, "most of what passed 60 percent is summarized: {tail_cut}");
}

#[test]
fn a_small_window_summarizes_only_with_a_fast_model() {
    let (tail, small) = (flat(60, 3_000), 16_384);
    // Without a fast model there is no soft line, however long the history is by estimate...
    assert_eq!(plan(&tail, small, usize::MAX, 0), Plan::Keep, "a small local model does not summarize");
    // ...and the hard line keeps the old behaviour: trim with the note.
    assert_eq!(plan(&tail, small, 10_000, 0), Plan::Trim);
    assert_eq!(plan(&flat(6, 100), small, 10_000, 0), Plan::Keep, "what fits is sent as it is");

    // With a fast model the same history is summarized.
    let fast = CompactionConfig { fast_model_ref: "fast::f".into(), ..Default::default() };
    let Plan::Summarize { tail_cut } = plan_with(&tail, None, small, usize::MAX, 0, fast.clone()) else { panic!("a fast model summarizes for a small window") };
    back_to(&tail, tail_cut, small, 4_000, 0);
    // A window of 32k or more summarizes with the chat model itself.
    assert!(matches!(plan(&flat(99, 3_000), 32_768, usize::MAX, 0), Plan::Summarize { .. }));
}

#[test]
fn disabled_always_trims() {
    let (tail, off) = (flat(81, 3_000), CompactionConfig { enabled: false, fast_model_ref: "fast::f".into(), ..Default::default() });
    assert_eq!(plan_with(&tail, None, WIDE, usize::MAX, 0, off.clone()), Plan::Keep, "off: no soft line");
    assert_eq!(plan_with(&tail, None, WIDE, 5_000, 0, off), Plan::Trim, "off: the hard line trims");
}

#[test]
fn the_cut_never_separates_a_tool_call_from_its_results() {
    let tail = tool_turns(40, 3_000);
    let earlier = Message::user(summary_message(&"Earlier facts. ".repeat(200), 16, WIDE));
    for (budget, last, summary, what) in [
        (usize::MAX, 0, false, "the soft line"),
        (5_000, 0, false, "the hard line"),
        (usize::MAX, 0, true, "the soft line behind a summary"),
        (5_000, 0, true, "the hard line behind a summary"),
    ] {
        let Plan::Summarize { tail_cut } = plan_with(&tail, summary.then_some(&earlier), WIDE, budget, last, CompactionConfig::default()) else { panic!("{what} summarizes") };
        assert!(tail_cut > 0 && tail[tail_cut].role == Role::User, "{what}: the cut {tail_cut} is at a user message");
        // Every tool call of the summarized part has its results there too: whole turns are summarized.
        for (i, m) in tail[..tail_cut].iter().enumerate() {
            let results = &tail[i + 1..(i + 1 + m.tool_calls.len()).min(tail_cut)];
            assert!(results.iter().all(|r| r.role == Role::Tool), "{what}: the call at {i} is not cut from its results");
        }
    }
    // With no summarizer the same history is trimmed, also at user messages only.
    assert_eq!(plan_with(&tail, None, 16_384, 5_000, 0, CompactionConfig::default()), Plan::Trim);
    let (kept, dropped) = fit_to_budget(sanitize_history(&tail), 5_000);
    assert!(dropped > 0 && kept[0].role == Role::User && kept.iter().any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(&kept[1].tool_calls[0].id)));
}

#[test]
fn the_hard_line_summarizes_to_its_cut_point_with_room_for_the_summary() {
    // Ten messages are 31 percent of a 32k window: no soft line, but too long for the budget.
    let tail = flat(10, 3_000);
    let (_, dropped) = fit_to_budget(sanitize_history(&tail), 5_000);
    assert_eq!(dropped, 6, "`fit_to_budget` alone would drop four messages");
    // Everything up to that cut point, and two more for the summary that comes in front.
    assert_eq!(plan(&tail, 32_768, 5_000, 0), Plan::Summarize { tail_cut: 8 });
    let message = Message::user(summary_message("The user asked five questions.", 8, 32_768));
    assert_eq!(request_history(&tail, Some(HistorySummary { message: &message, upto: 8 }), 5_000).1, 0, "the request that follows is not trimmed again");
    // What fits the budget and the soft line is sent as it is.
    assert_eq!(plan(&tail, 32_768, usize::MAX, 0), Plan::Keep);
}

#[test]
fn the_provider_count_can_trigger_it_earlier_than_the_estimate() {
    // Fifty messages are half the window by estimate; the provider counted 85 percent of it.
    let tail = flat(50, 3_000);
    assert_eq!(plan(&tail, WIDE, usize::MAX, 0), Plan::Keep);
    let Plan::Summarize { tail_cut } = plan(&tail, WIDE, usize::MAX, 85_000) else { panic!("what the provider counted triggers the soft line") };
    // What it counted beyond the estimate is in every request, so it is in what remains.
    let overhead = 85_000 - tail.iter().map(message_chars).sum::<usize>().div_ceil(CHARS_PER_TOKEN);
    back_to(&tail, tail_cut, WIDE, 8_000, overhead);
    // Under the line by the provider's count too: nothing.
    assert_eq!(plan(&tail, WIDE, usize::MAX, 79_000), Plan::Keep);
}

#[test]
fn nothing_to_summarize_and_unknown_windows() {
    // An unknown window is never trimmed or compacted.
    assert_eq!(plan(&flat(99, 3_000), 0, 5_000, 999_999), Plan::Keep);
    // One user turn: nothing before it, however long.
    assert_eq!(plan(&flat(2, 100_000), WIDE, usize::MAX, 0), Plan::Keep);
    assert_eq!(plan(&flat(2, 100_000), WIDE, 5_000, 0), Plan::Keep, "the last user turn is always kept");
}

#[test]
fn a_count_from_before_a_compaction_summarizes_nothing_for_nothing() {
    let earlier = Message::user(summary_message("Earlier facts.", 40, WIDE));
    // A stale count (as after a compaction, until the loop resets it) is over the line...
    assert_eq!(plan_with(&flat(2, 300), Some(&earlier), WIDE, usize::MAX, 95_000, CompactionConfig::default()), Plan::Keep, "nothing before the last user message");
    // ...but summarizing one small turn in front of a summary would not make the request smaller.
    assert_eq!(plan_with(&flat(4, 300), Some(&earlier), WIDE, usize::MAX, 95_000, CompactionConfig::default()), Plan::Keep);
    // With something to gain, the cut is before the last user message.
    let tail = flat(6, 40_000);
    assert_eq!(plan_with(&tail, Some(&earlier), WIDE, usize::MAX, 95_000, CompactionConfig::default()), Plan::Summarize { tail_cut: 4 });
}

#[test]
fn a_compaction_by_hand_or_on_a_capacity_error_cuts_before_the_last_user_message() {
    let said = |texts: &[&str]| texts.iter().map(|t| Message::user(t.to_string())).collect::<Vec<_>>();
    assert_eq!(before_last_user(&said(&["one", "two"])), Some(1));
    assert_eq!(before_last_user(&said(&["only"])), None, "nothing before the last user message");
    assert_eq!(before_last_user(&[]), None);
    let turns = tool_turns(3, 10);
    assert_eq!(before_last_user(&sanitize_history(&turns)), Some(8), "whole turns: their results stay with their calls");

    // The same cut as an index into the stored messages, repaired messages included.
    let stored: Vec<Message> = std::iter::once(Message::tool_result(&ToolCall { id: "orphan".into(), name: "x".into(), input: json!({}) }, "lost", true)).chain(turns).collect();
    let (tail, from) = (sanitize_history(&stored[1..]), 1);
    let cut = before_last_user(&tail).unwrap();
    assert_eq!((cut, stored_index(&stored, from, &tail, cut)), (8, 9));
    assert!(stored[9].content.starts_with("q2"), "the user message the summary ends before: {}", stored[9].content);
    // Behind a summary of the first five stored messages: the cut counts user messages of the tail only.
    assert_eq!(stored_index(&stored, 5, &sanitize_history(&stored[5..]), 4), 9);
    assert_eq!(stored_index(&stored, 11, &sanitize_history(&stored[11..]), 0), 13, "no user message left: the end of the stored messages");
}

#[test]
fn the_summary_request_puts_the_old_summary_first_and_cuts_tool_results() {
    let call = ToolCall { id: "c".into(), name: "system_info".into(), input: json!({}) };
    let messages = vec![Message::user("q"), Message::assistant("", vec![call.clone()]), Message::tool_result(&call, "y".repeat(5_000), false)];
    let request = summary_request("fast::f", Some("Earlier: the user asked about swap."), &messages);
    assert_eq!(request.system, SUMMARY_SYSTEM);
    assert!(request.tools.is_empty(), "no tools");
    assert_eq!((request.options.max_output_tokens, request.options.reasoning_level.as_deref(), request.options.temperature), (Some(1_500), None, None), "max_output_tokens 1500, nothing else");
    assert_eq!(request.model_ref, "fast::f");
    assert_eq!(request.messages.len(), 5);
    assert_eq!(request.messages[0].role, Role::User);
    assert_eq!(request.messages[0].content, "Summary of the earlier conversation:\nEarlier: the user asked about swap.");
    assert_eq!((request.messages[1].content.as_str(), request.messages[2].content.as_str(), request.messages[2].tool_calls.len()), ("q", "", 1), "the call follows its user message");
    assert_eq!(request.messages[3].content.chars().count(), 1_500, "a tool result is cut to 1500 chars");
    // The request ends with a user message: one that ends with an assistant answer gets an empty answer from the local model.
    assert_eq!((request.messages[4].role, request.messages[4].content.as_str()), (Role::User, SUMMARY_ASK));
    assert_eq!(summary_messages(None, &messages)[0].content, "q", "without an old summary the messages to compact come first");
    assert_eq!((summary_messages(None, &messages).len(), summary_messages(Some("  "), &messages).len()), (4, 4), "a blank summary is none");
    assert!(summary_messages(None, &[]).is_empty(), "nothing to compact, no request");
}

#[test]
fn a_long_summary_is_cut_at_a_sentence() {
    let sentence = "The user asked about swap usage and the answer was 2 GB in use. ";
    let long = sentence.repeat(80);
    assert_eq!(summary_message(&format!(" {long} "), 12, 65_536), format!("Conversation so far (summary of the first 12 messages):\n{}", long.trim_end()), "a wide window keeps the summary whole, trimmed");
    assert_eq!(summary_message(&long, 12, 0), format!("Conversation so far (summary of the first 12 messages):\n{}", long.trim_end()), "an unknown window too");
    let small = summary_message(&long, 12, 32_000);
    let (head, body) = small.split_once('\n').unwrap();
    assert_eq!(head, "Conversation so far (summary of the first 12 messages):");
    assert!(body.len() <= 4_000 && body.len() + sentence.len() > 4_000, "as much as 4000 chars allows: {}", body.len());
    assert!(body.ends_with("2 GB in use.") && long.starts_with(body), "cut after a sentence: ...{}", &body[body.len() - 40..]);
    assert_eq!(summary_message("  short.  ", 3, 32_000), "Conversation so far (summary of the first 3 messages):\nshort.", "trimmed");
    assert_eq!(cut_summary("no end in this text at all", 10), "no end in", "no sentence end: a character boundary");
    assert_eq!(cut_summary("short", 10), "short");
}

#[test]
fn the_view_the_api_answers_and_the_capacity_errors() {
    assert_eq!(serde_json::to_value(CompactView { summary: "s".into(), kept_messages: 2 }).unwrap(), json!({"summary": "s", "keptMessages": 2}));
    assert_eq!(CompactionConfig::default(), CompactionConfig { enabled: true, threshold_pct: 80, fast_model_ref: String::new() });
    for e in ["HTTP 503 Service Unavailable", "429 Too Many Requests", "API key not valid", "rate limit exceeded", "403 Forbidden"] {
        assert!(is_capacity_error(e), "{e}");
    }
    for e in ["context_length_exceeded", "400 Bad Request", "connection refused"] {
        assert!(!is_capacity_error(e), "{e}");
    }
    for e in ["connection refused", "error sending request: dns error", "request timed out", "502 Bad Gateway", "504 Gateway Timeout"] {
        assert!(is_unreachable_error(e), "{e}");
    }
    for e in ["400 Bad Request", "429 quota", "error decoding response body: timed out"] {
        assert!(!is_unreachable_error(e), "{e}");
    }
}

/// agent-loop.md C.1: a note of the daemon starts no turn and is not summarized.
#[test]
fn notes_are_not_a_turn_and_not_in_a_summary() {
    let note = Message::user("[note from the daemon, not from the owner; it is not an approval]\n- The owner changed `k` from 1 to 2.").with_via(crate::protocol::Via::Note);
    let tail = vec![Message::user("q0"), Message::assistant("a0", vec![]), Message::user("q1"), Message::assistant("a1", vec![]), note.clone(), Message::assistant("a2", vec![])];
    assert_eq!(before_last_user(&tail), Some(2), "the cut is before the owner's last message, not before the note");
    let kept = without_notes(&tail);
    assert_eq!(kept.len(), 5);
    assert!(kept.iter().all(|m| m.via != crate::protocol::Via::Note));
    // A note before the owner's second message: the cut before that message is still the index of it.
    let with_early_note = vec![Message::user("q0"), Message::assistant("a0", vec![]), note.clone(), Message::user("q1"), Message::assistant("a1", vec![])];
    assert_eq!(stored_index(&with_early_note, 0, &with_early_note, 3), 3, "the notes are not counted among the owner's messages");
}

