//! Context compaction (compaction.md): when a long conversation is summarized, what the
//! summary request and a compacted request look like, and what the loop does when the
//! model cannot take a request (compact once and retry; only then the fallback model).

use std::sync::Arc;

use anyhow::Context as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::context::{self, message_chars, CHARS_PER_TOKEN};
use crate::model::{ModelClient, ModelOptions, ModelRequest};
use crate::protocol::{Message, Role};
use crate::util::truncate_str;

/// `settings.compaction` and the fast model, as the daemon reads them for each request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionConfig {
    /// `compaction.enabled`: false always trims.
    pub enabled: bool,
    /// `compaction.thresholdPct` (50..=95): the share of the window that is summarized.
    pub threshold_pct: u8,
    /// `fastModel` when it is set and resolves, else empty: the chat model summarizes.
    pub fast_model_ref: String,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self { enabled: true, threshold_pct: 80, fast_model_ref: String::new() }
    }
}

/// A window this large summarizes with the chat model itself (compaction.md section 1).
pub const MIN_WINDOW_TOKENS: usize = 32_768;
/// Under this window a stored summary is cut to `SMALL_WINDOW_SUMMARY_CHARS`.
const WIDE_WINDOW_TOKENS: usize = 65_536;
const SMALL_WINDOW_SUMMARY_CHARS: usize = 4_000;
const TOOL_RESULT_CHARS: usize = 1_500;
/// The manual compaction needs this many stored messages.
pub(crate) const MIN_MANUAL_MESSAGES: usize = 4;

/// What the loop knows when it prepares a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub compaction: CompactionConfig,
    /// The window of the model the request goes to (0 = unknown).
    pub context_tokens: usize,
    /// What `fit_to_budget` gets for the history.
    pub budget_tokens: usize,
    /// The prompt tokens of the last model response (compaction.md section 6); 0 = none.
    pub last_input_tokens: u64,
}

impl Trigger {
    /// Whether a summary can be made at all: the window is large enough, or a fast model is set.
    fn summarizer(&self) -> bool {
        !self.compaction.fast_model_ref.is_empty() || self.context_tokens >= MIN_WINDOW_TOKENS
    }
}

/// What a model request would carry before compaction.
#[derive(Debug, Clone, Copy)]
pub struct History<'a> {
    /// The summary message of the earlier part, when the session has one.
    pub summary: Option<&'a Message>,
    /// The repaired stored messages the summary does not cover (empty without stored messages).
    pub tail: &'a [Message],
}

/// What the loop does with the history before a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// It fits: send it as it is.
    Keep,
    /// No summarizer, or compaction is off: `fit_to_budget` drops the oldest turns and notes it.
    Trim,
    /// Summarize `tail[..tail_cut]`, the old summary with it. The cut is at a user message of the tail.
    Summarize { tail_cut: usize },
}

/// The longest a summary is in a request of a `window`-token model (chars).
fn reserve_chars(window: usize) -> usize {
    if window < WIDE_WINDOW_TOKENS { SMALL_WINDOW_SUMMARY_CHARS } else { 8_000 }
}

/// Compaction.md section 1. The soft line: when the estimate (`max` of chars/3 and what
/// the provider last counted) passes `thresholdPct` of the window, the oldest whole user
/// turns are summarized until what remains, the new summary and the provider's overhead
/// counted, is back to `thresholdPct - 20`. The hard line: when `fit_to_budget` would
/// still drop messages, everything up to its cut point is summarized too (with room
/// asked for the new summary, so the request that follows is not trimmed again).
pub fn should_compact(h: &History<'_>, t: &Trigger) -> Plan {
    if t.context_tokens == 0 {
        return Plan::Keep; // an unknown window is never trimmed
    }
    let window = t.context_tokens;
    let head = usize::from(h.summary.is_some());
    let full: Vec<Message> = h.summary.into_iter().cloned().chain(h.tail.iter().cloned()).collect();
    let hard = context::fit_keeping(full, t.budget_tokens, head).1;
    if !t.compaction.enabled || !t.summarizer() {
        return if hard > 0 { Plan::Trim } else { Plan::Keep };
    }
    let sizes: Vec<usize> = h.tail.iter().map(message_chars).collect();
    let estimate = (h.summary.map(message_chars).unwrap_or(0) + sizes.iter().sum::<usize>()).div_ceil(CHARS_PER_TOKEN);
    let used = estimate.max(t.last_input_tokens as usize);
    let mut cut = 0;
    if (used as u64) * 100 > (window as u64) * u64::from(t.compaction.threshold_pct) {
        let reserve = reserve_chars(window).div_ceil(CHARS_PER_TOKEN);
        // What the provider counted beyond the estimate (system prompt, tools) is in every request.
        let overhead = used - estimate;
        let target = window * (usize::from(t.compaction.threshold_pct) - 20) / 100;
        let mut suffix = vec![0; sizes.len() + 1];
        for i in (0..sizes.len()).rev() {
            suffix[i] = suffix[i + 1] + sizes[i];
        }
        let after = |c: usize| overhead + reserve + suffix[c].div_ceil(CHARS_PER_TOKEN);
        // Not the first message: something real must be summarized. Nothing reaches the
        // target: the last user message, when that still makes the request smaller.
        let users: Vec<usize> = (1..h.tail.len()).filter(|&i| h.tail[i].role == Role::User).collect();
        if let Some(&last) = users.last() {
            cut = users.into_iter().find(|&c| after(c) <= target).unwrap_or(last);
            if after(cut) >= used {
                cut = 0;
            }
        }
    }
    if cut == 0 && hard == 0 {
        return Plan::Keep;
    }
    let mut candidate = Vec::with_capacity(1 + h.tail.len() - cut);
    candidate.push(Message::user("x".repeat(reserve_chars(window))));
    candidate.extend(h.tail[cut..].iter().cloned());
    cut += context::fit_keeping(candidate, t.budget_tokens, 1).1;
    Plan::Summarize { tail_cut: cut }
}

/// The cut of a compaction by hand or on a capacity error (compaction.md sections 1
/// and 4): everything before the last user message. `None` when that is nothing.
pub fn before_last_user(tail: &[Message]) -> Option<usize> {
    let last = tail.iter().rposition(|m| m.role == Role::User)?;
    (last > 0).then_some(last)
}

/// The index in `stored` of the user message a cut of the tail ends before. `tail` is
/// `sanitize_history(&stored[from..])`, which keeps every user message.
pub fn stored_index(stored: &[Message], from: usize, tail: &[Message], tail_cut: usize) -> usize {
    let rank = tail[..tail_cut].iter().filter(|m| m.role == Role::User).count();
    stored.iter().enumerate().skip(from).filter(|(_, m)| m.role == Role::User).map(|(i, _)| i).nth(rank).unwrap_or(stored.len())
}

/// The system text of the summary request (compaction.md section 3).
pub const SUMMARY_SYSTEM: &str = "Summarize the conversation so far for an assistant who continues it without the original messages. Keep, in third person and in the language of the conversation: facts established (names, paths, numbers, commands and their results), decisions and their reasons, what the user asked for and corrected, what was tried and what happened, open tasks and todo items, approvals given or refused. Leave out greetings and repetition. At most 1200 words. Output the summary only.";

/// How the summary is asked for: the last message of the request (compaction.md section 3).
pub const SUMMARY_ASK: &str = "Summarize the conversation above now.";

/// The messages of the summary request: the summary so far as a first user message,
/// then the messages to compact, each tool result cut to 1500 chars, then one user
/// message asking for the summary. Without that last message the request would end
/// with an assistant answer, and the local model (qwen2.5 on Ollama) then answers with
/// nothing: found with the real server, 2 of 2 runs.
pub fn summary_messages(old_summary: Option<&str>, messages: &[Message]) -> Vec<Message> {
    let mut out: Vec<Message> = old_summary.filter(|s| !s.trim().is_empty()).map(|s| vec![Message::user(format!("Summary of the earlier conversation:\n{s}"))]).unwrap_or_default();
    out.extend(messages.iter().cloned().map(|mut m| {
        if m.role == Role::Tool {
            m.content = truncate_str(&m.content, TOOL_RESULT_CHARS).to_string();
        }
        m
    }));
    if !out.is_empty() {
        out.push(Message::user(SUMMARY_ASK));
    }
    out
}

/// The summary request: no tools, `max_output_tokens` 1500.
pub fn summary_request(model_ref: &str, old_summary: Option<&str>, messages: &[Message]) -> ModelRequest {
    ModelRequest {
        model_ref: model_ref.to_string(),
        system: SUMMARY_SYSTEM.into(),
        messages: summary_messages(old_summary, messages),
        tools: vec![],
        options: ModelOptions { max_output_tokens: Some(1_500), ..Default::default() },
    }
}

/// Ask for a summary of `messages` (and the summary there is). An error says why: the
/// request failed (cancel included) or the answer was empty.
pub async fn summarize(model: Arc<dyn ModelClient>, model_ref: &str, old_summary: Option<&str>, messages: &[Message], cancel: CancellationToken) -> anyhow::Result<String> {
    anyhow::ensure!(!model_ref.trim().is_empty() && !messages.is_empty(), "nothing to summarize without a model and messages");
    let (tx, _rx) = mpsc::unbounded_channel();
    let response = model.complete(summary_request(model_ref, old_summary, messages), tx, cancel).await.context("the summary request failed")?;
    let text = response.text.trim();
    anyhow::ensure!(!text.is_empty(), "the model returned no summary");
    Ok(text.to_string())
}

/// The synthetic user message a compacted request starts with (compaction.md section 4).
pub fn summary_message(summary: &str, upto: usize, context_tokens: usize) -> String {
    let cap = if context_tokens > 0 && context_tokens < WIDE_WINDOW_TOKENS { SMALL_WINDOW_SUMMARY_CHARS } else { usize::MAX };
    format!("Conversation so far (summary of the first {upto} messages):\n{}", cut_summary(summary.trim(), cap))
}

/// Cut to at most `max_chars`, after the last sentence end of the second half; else at a character boundary.
pub fn cut_summary(text: &str, max_chars: usize) -> &str {
    let head = truncate_str(text, max_chars);
    let bytes = head.as_bytes();
    let end = ((head.len() / 2)..head.len()).rev().find(|&i| matches!(bytes[i], b'.' | b'!' | b'?' | b'\n') && bytes.get(i + 1).is_none_or(u8::is_ascii_whitespace));
    match end {
        Some(i) => head[..=i].trim_end(),
        None => head.trim_end(),
    }
}

/// The answer of `POST /api/sessions/:id/compact` (api.md section 6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactView {
    pub summary: String,
    /// The stored messages the summary does not cover.
    pub kept_messages: usize,
}

/// Why a compaction by hand did not happen.
#[derive(Debug, thiserror::Error)]
pub enum CompactError {
    /// Fewer than four stored messages, or nothing before the last user message (400).
    #[error("{0}")]
    TooShort(String),
    /// The summary call failed (502).
    #[error("the summary failed: {0}")]
    Failed(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// What the loop does after the model could not take a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    /// The history was compacted: the same model is retried.
    Retry,
    /// `config` now names the fallback model.
    Fallback,
    /// The error stands.
    Fail,
}

/// Provider errors worth retrying: capacity or quota, and a broken key or account (the
/// user still gets an answer, plus a notice naming the cause). Not bad input: the
/// fallback would fail the same way.
pub fn is_capacity_error(e: &str) -> bool {
    let e = e.to_lowercase();
    [
        "503", "429", "overloaded", "high demand", "unavailable", "quota", "rate limit", "resource_exhausted",
        "api key not valid", "valid api key", "invalid api key", "invalid_api_key", "unauthenticated", "permission_denied", "401 unauthorized", "403 forbidden",
    ]
    .iter()
    .any(|m| e.contains(m))
}

/// The provider cannot be reached at all (notices.md class `down`): connection refused, a DNS
/// failure, a timeout before any byte, a 502/504 from a gateway. A break in the middle of an
/// answer (reading the body) is not that: it stays an error of the turn.
pub fn is_unreachable_error(e: &str) -> bool {
    let lower = e.to_lowercase();
    let mid_answer = ["decoding response body", "reading response body", "stream"].iter().any(|m| lower.contains(m));
    !mid_answer && crate::agent::notice::ErrorClass::of(e) == crate::agent::notice::ErrorClass::Down
}

mod loop_side;

#[cfg(test)]
mod tests;
