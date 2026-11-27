//! The model a conversation chose (picker.md section 4): stored in the session's meta, and
//! applied to the config of a turn after the job of a subagent (`apply_job`).

use super::children::UsableModels;
use super::state::SessionState;
use super::{Agent, AgentConfig};
use crate::protocol::AgentEvent;
use crate::session::{SessionMeta, SessionOrigin};

impl Agent {
    /// Where a conversation's choice is asked of (`ProviderStore::usable` in the daemon).
    /// Unset: the choice replaces `model_ref` alone and a level is sent as it is stored.
    pub fn set_usable_models(&self, source: UsableModels) {
        self.children.set_usable_models(source);
    }

    /// Store the choice of `id` (picker.md section 4.1): the arguments are the values
    /// the meta ends with. The file is written
    /// under the meta lock (the old values come back on an error), the event goes out after
    /// it, and nothing is written or sent when the meta would be the same.
    pub async fn set_model_choice(&self, id: &str, model: Option<String>, level: Option<String>) -> anyhow::Result<SessionMeta> {
        let state = self.state(id).await?;
        let mut meta = state.meta.lock().await;
        if meta.model == model && meta.reasoning_level == level {
            return Ok(meta.clone());
        }
        let before = (std::mem::replace(&mut meta.model, model), std::mem::replace(&mut meta.reasoning_level, level));
        if let Err(e) = self.store.save_meta(&meta).await {
            (meta.model, meta.reasoning_level) = before;
            return Err(e);
        }
        let meta = meta.clone();
        let _ = self.events.send(AgentEvent::SessionMetaChanged { session_id: id.into(), meta: meta.clone() });
        Ok(meta)
    }

    /// The choice of the session answers the turn (picker.md section 4.2). `updated_at` is left alone.
    pub(super) async fn apply_choice(&self, session_id: &str, state: &SessionState, config: &mut AgentConfig) {
        let (origin, model, level) = {
            let meta = state.meta.lock().await;
            (meta.origin, meta.model.clone(), meta.reasoning_level.clone())
        };
        if origin == SessionOrigin::Subagent {
            return;
        }
        let source = self.children.usable_models();
        let notice = |text: String| {
            let _ = self.events.send(AgentEvent::Notice { level: "warn".into(), text });
        };
        let default = config.model_ref.clone();
        let mut answering = None;
        if let Some(choice) = model.filter(|m| *m != default) {
            match source.as_ref().map(|ask| ask(&choice)) {
                None => config.model_ref = choice,
                Some(Ok(info)) => {
                    config.model_ref = choice;
                    config.context_tokens = info.context_tokens;
                    if info.max_output_tokens.is_some() {
                        config.options.max_output_tokens = info.max_output_tokens;
                    }
                    answering = Some(info.reasoning_levels);
                }
                Some(Err(reason)) => {
                    tracing::warn!(session_id, "the chosen model {choice} cannot answer: {reason}");
                    notice(format!("{choice} cannot answer ({reason}) - answering with {default}"));
                }
            }
        }
        config.options.reasoning_level = None;
        let Some(level) = level else { return };
        // The effort goes with the model that answers, never with one that was left.
        let levels = match (answering, source.as_ref()) {
            (Some(levels), _) => Some(levels),
            (None, None) => None,
            (None, Some(ask)) => Some(ask(&config.model_ref).map(|i| i.reasoning_levels).unwrap_or_default()),
        };
        match levels {
            Some(levels) if !levels.contains(&level) => {
                let offered = if levels.is_empty() { "none".to_string() } else { levels.join(", ") };
                notice(format!("effort '{level}' is not one {} offers ({offered}) - answering with its default", config.model_ref));
            }
            _ => config.options.reasoning_level = Some(level),
        }
    }

    /// A fallback switch keeps the level only when the fallback model offers it.
    pub(crate) fn level_for_fallback(&self, config: &mut AgentConfig) {
        let (Some(level), Some(ask)) = (config.options.reasoning_level.as_ref(), self.children.usable_models()) else { return };
        if !ask(&config.model_ref).is_ok_and(|info| info.reasoning_levels.contains(level)) {
            config.options.reasoning_level = None;
        }
    }
}
