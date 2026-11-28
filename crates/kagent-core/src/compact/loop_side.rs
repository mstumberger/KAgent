//! The loop's side of compaction: the history of the next request (summarized, or
//! trimmed when a summary cannot be made), a compaction by hand, and the capacity path.

use std::sync::atomic::Ordering;

use tokio_util::sync::CancellationToken;

use crate::agent::notice::{provider_key, provider_of, ErrorClass};
use crate::agent::state::SessionState;
use crate::agent::{Agent, AgentConfig};
use crate::compact::{before_last_user, without_notes, is_capacity_error, is_unreachable_error, should_compact, stored_index, summarize, summary_message, CompactError, CompactView, CompactionConfig, History, Plan, Step, Trigger, MIN_MANUAL_MESSAGES};
use crate::context::{self, sanitize_history, HistorySummary};
use crate::protocol::{AgentEvent, Message};

/// The stored messages, and the summary there is with the number of stored messages it covers.
async fn history_of(state: &SessionState) -> (Vec<Message>, Option<(String, usize)>) {
    let stored = state.messages.lock().await.clone();
    let meta = state.meta.lock().await;
    (stored, meta.summary.clone().filter(|s| !s.trim().is_empty()).map(|text| (text, meta.summary_upto.unwrap_or(0))))
}

impl Agent {
    /// The messages of the next model request (compaction.md sections 1, 4 and 6): the
    /// history is summarized or trimmed first. A failed summary trims instead.
    pub(crate) async fn request_history(
        &self,
        session_id: &str,
        state: &SessionState,
        config: &AgentConfig,
        budget_tokens: usize,
        cancel: &CancellationToken,
    ) -> anyhow::Result<(Vec<Message>, usize)> {
        let (stored, old) = history_of(state).await;
        let from = old.as_ref().map_or(0, |(_, upto)| *upto);
        let tail = sanitize_history(&stored[from.min(stored.len())..]);
        let compaction = self.compaction_config();
        let message_of = |text: &str, upto: usize| Message::user(summary_message(text, upto, config.context_tokens));
        let plan = should_compact(
            &History { summary: old.as_ref().map(|(text, upto)| message_of(text, *upto)).as_ref(), tail: &tail },
            &Trigger { compaction: compaction.clone(), context_tokens: config.context_tokens, budget_tokens, last_input_tokens: state.last_input_tokens.load(Ordering::Relaxed) },
        );
        let mut summary = old.as_ref().map(|(text, upto)| (text.clone(), *upto));
        if let Plan::Summarize { tail_cut } = plan {
            let upto = stored_index(&stored, from, &tail, tail_cut);
            match self.summarize_into(session_id, state, &config.model_ref, &summarizer_of(config, &compaction), old.as_ref().map(|(text, _)| text.as_str()), &without_notes(&tail[..tail_cut]), upto, cancel).await {
                Ok(text) => summary = Some((text, upto)),
                Err(e) => tracing::warn!(session_id, "history not summarized, trimming instead: {e:#}"),
            }
        }
        let message = summary.as_ref().map(|(text, upto)| (message_of(text, *upto), *upto));
        let summary = message.as_ref().map(|(message, upto)| HistorySummary { message, upto: *upto });
        let (messages, dropped) = context::request_history(&stored, summary, budget_tokens);
        if dropped > 0 {
            tracing::info!(session_id, dropped, "trimmed history to fit the context window");
        }
        Ok((messages, dropped))
    }

    /// Everything before the last user message, summarized into the session's summary
    /// (by hand, or once on a capacity error). `Ok(None)`: there is nothing to summarize.
    pub(crate) async fn compact_rest(&self, session_id: &str, state: &SessionState, config: &AgentConfig, cancel: &CancellationToken) -> Result<Option<CompactView>, String> {
        let (stored, old) = history_of(state).await;
        let from = old.as_ref().map_or(0, |(_, upto)| *upto);
        let tail = sanitize_history(&stored[from.min(stored.len())..]);
        let Some(cut) = before_last_user(&tail) else { return Ok(None) };
        let upto = stored_index(&stored, from, &tail, cut);
        let text = self
            .summarize_into(session_id, state, &config.model_ref, &summarizer_of(config, &self.compaction_config()), old.as_ref().map(|(text, _)| text.as_str()), &without_notes(&tail[..cut]), upto, cancel)
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(Some(CompactView { summary: text, kept_messages: stored.len() - upto }))
    }

    /// A compaction by hand (`POST /api/sessions/:id/compact`, compaction.md section 4):
    /// everything before the last user message. With nothing new before it, the summary
    /// there is, is the answer.
    pub async fn compact(&self, session_id: &str) -> Result<CompactView, CompactError> {
        let state = self.state(session_id).await?;
        let (stored, old) = history_of(&state).await;
        if stored.len() < MIN_MANUAL_MESSAGES {
            return Err(CompactError::TooShort(format!("{} stored messages; compaction needs {}", stored.len(), MIN_MANUAL_MESSAGES)));
        }
        let config = self.config();
        match self.compact_rest(session_id, &state, &config, &CancellationToken::new()).await {
            Ok(Some(view)) => Ok(view),
            Ok(None) => match old.filter(|(_, upto)| *upto > 0) {
                Some((text, upto)) => Ok(CompactView { summary: text, kept_messages: stored.len() - upto }),
                None => Err(CompactError::TooShort("nothing before the last message".into())),
            },
            Err(e) => Err(CompactError::Failed(e)),
        }
    }

    /// Ask the summarizer and store its answer as the summary of the first `upto` stored
    /// messages, written before any request uses it.
    async fn summarize_into(&self, session_id: &str, state: &SessionState, brain: &str, model_ref: &str, old: Option<&str>, messages: &[Message], upto: usize, cancel: &CancellationToken) -> anyhow::Result<String> {
        // The images of what is summarized become their descriptions, made now when there is none.
        let mut messages = messages.to_vec();
        self.imageless(session_id, &mut messages, brain, cancel).await;
        let text = summarize(self.model.clone(), model_ref, old, &messages, cancel.clone()).await?;
        {
            let mut meta = state.meta.lock().await;
            let previous = (meta.summary.take(), meta.summary_upto.take());
            (meta.summary, meta.summary_upto) = (Some(text.clone()), Some(upto));
            if let Err(e) = self.store().save_meta(&meta).await {
                (meta.summary, meta.summary_upto) = previous;
                return Err(e.context(format!("summary of session {session_id} not stored")));
            }
        }
        // The next prompt is not the one the provider last counted.
        state.last_input_tokens.store(0, Ordering::Relaxed);
        Ok(text)
    }

    /// Compaction.md section 1, third bullet: compact once and retry the same model
    /// once; only then switch to `fallbackModel`. With compaction switched off, or
    /// nothing to summarize, or a summary that fails: the fallback at once.
    pub(crate) async fn on_capacity_error(&self, session_id: &str, state: &SessionState, config: &mut AgentConfig, error: &anyhow::Error, recompacted: bool, fell_back: bool, cancel: &CancellationToken) -> Step {
        let text = format!("{error:#}");
        // A provider that cannot be reached at all: compacting would not help, the fallback is the way out.
        if is_unreachable_error(&text) && !is_capacity_error(&text) {
            if !fell_back && !config.fallback_model_ref.is_empty() && config.fallback_model_ref != config.model_ref {
                self.switch_to_fallback(session_id, config, error);
                return Step::Fallback;
            }
            return Step::Fail;
        }
        if !is_capacity_error(&text) {
            return Step::Fail;
        }
        if !recompacted && self.compaction_config().enabled {
            match self.compact_rest(session_id, state, config, cancel).await {
                Ok(Some(_)) => {
                    tracing::warn!(session_id, "chat model {} could not take the request ({error:#}); the history is compacted and the request retried", config.model_ref);
                    return Step::Retry;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(session_id, "history not summarized before the retry: {e}"),
            }
        }
        if !fell_back && !config.fallback_model_ref.is_empty() && config.fallback_model_ref != config.model_ref {
            self.switch_to_fallback(session_id, config, error);
            return Step::Fallback;
        }
        Step::Fail
    }

    /// The rest of the turn goes to `fallbackModel`: the chat model is overloaded or out of quota (429/503), or its key is broken.
    fn switch_to_fallback(&self, session_id: &str, config: &mut AgentConfig, error: &anyhow::Error) {
        tracing::warn!(session_id, "chat model {} unavailable ({error:#}); falling back to {}", config.model_ref, config.fallback_model_ref);
        let error_text = format!("{error:#}");
        let provider = provider_of(&config.model_ref);
        let key = provider_key(provider, ErrorClass::of(&error_text));
        self.failing.failed(&config.model_ref, key.clone());
        let _ = self.events.send(AgentEvent::keyed_notice("warn", fallback_notice(&config.model_ref, &config.fallback_model_ref, &error_text), key).in_session(session_id));
        config.model_ref = config.fallback_model_ref.clone();
        config.context_tokens = config.fallback_context_tokens;
        config.options.max_output_tokens = config.fallback_max_output;
        // The level of the choice goes only to a model that offers it (picker.md section 4.2).
        self.level_for_fallback(config);
    }
}

/// What the owner is told when a turn changes models, and why the first one could not
/// answer (models.md section 5): out of quota when the error says 429 or "quota", a
/// rejected key as before, busy otherwise.
pub(crate) fn fallback_notice(model: &str, fallback: &str, error: &str) -> String {
    format!("{model} {} - answering with {fallback}", ErrorClass::of(error).why())
}

/// The model that summarizes (compaction.md section 3): `fastModel` when it is set, else the chat model.
fn summarizer_of(config: &AgentConfig, compaction: &CompactionConfig) -> String {
    if compaction.fast_model_ref.is_empty() { config.model_ref.clone() } else { compaction.fast_model_ref.clone() }
}

