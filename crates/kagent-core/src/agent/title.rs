//! Session names: a placeholder ("", "New session", "Voice session") is replaced by a
//! short summary of the first meaningful user message. A quick name from the text is
//! set at once; the chat model then refines it into 3-6 words in the same language.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::model::{ModelClient, ModelOptions, ModelRequest};
use crate::protocol::{AgentEvent, Message};
use crate::util::truncate_str;

pub fn is_placeholder(title: &str) -> bool {
    let t = title.trim();
    t.is_empty() || t == super::DEFAULT_TITLE || t == "Voice session"
}

/// Fillers and mis-heard fragments ("Ok.", "app", "Hey") are not a topic.
pub fn meaningful(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.len() >= 3 || (words.len() == 2 && text.trim().len() >= 12)
}

/// First sentence, without greetings, cut at a word boundary (~48 chars).
pub fn quick_title(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    if !meaningful(line) {
        return None;
    }
    let mut s = line;
    for g in ["hey kagent,", "hey kagent", "hi,", "hello,", "hey,", "ok,", "okay,", "so,", "ja,", "zdravo,"] {
        if s.to_lowercase().starts_with(g) {
            s = s[g.len()..].trim_start();
        }
    }
    let first = s.split(['.', '?', '!']).next().unwrap_or(s).trim();
    let first = if first.split_whitespace().count() >= 2 { first } else { s };
    let mut out = String::new();
    for w in first.split_whitespace() {
        if out.len() + w.len() + 1 > 48 {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    let mut c = out.chars();
    let titled = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => return None,
    };
    Some(titled)
}

/// Clean a model's answer into a title: one line, no quotes/labels, max 60 chars.
pub fn clean_summary(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.trim_start_matches("Title:").trim_start_matches("title:").trim();
    let line = line.trim_matches(|c: char| c == '"' || c == '\'' || c == '*' || c == '`' || c == '.' || c.is_whitespace());
    let words = line.split_whitespace().count();
    if line.is_empty() || words > 9 || line.len() > 70 {
        return None;
    }
    Some(truncate_str(line, 60).to_string())
}

/// Ask the model for a short title (3-6 words, the text's language). None on failure.
pub async fn summarize(model: Arc<dyn ModelClient>, model_ref: &str, text: &str) -> Option<String> {
    if model_ref.trim().is_empty() {
        return None;
    }
    let req = ModelRequest {
        model_ref: model_ref.to_string(),
        system: "You name conversations. Reply with only a title of 3 to 6 words in the same language as the text: no quotes, no trailing period.".into(),
        messages: vec![Message::user(format!("Conversation starts with:\n{}", truncate_str(text, 600)))],
        tools: vec![],
        options: ModelOptions { max_output_tokens: Some(24), ..Default::default() },
    };
    let (tx, _rx) = mpsc::unbounded_channel();
    let r = tokio::time::timeout(std::time::Duration::from_secs(20), model.complete(req, tx, CancellationToken::new())).await.ok()?.ok()?;
    clean_summary(&r.text)
}

/// Give session `sid` the title `title`. A session in memory is changed under its meta's
/// lock and saved from there, so the save holds every change made meanwhile (its todo list,
/// its message count); one that is not loaded is read and saved at once.
async fn apply_title(sessions: &super::Sessions, store: &crate::session::SessionStore, refined: &Refined, sid: &str, title: &str) {
    let loaded = sessions.lock().unwrap().get(sid).cloned();
    match loaded {
        Some(state) => {
            let mut meta = state.meta.lock().await;
            let before = std::mem::replace(&mut meta.title, title.to_string());
            if let Err(e) = store.save_meta(&meta).await {
                tracing::warn!(session_id = sid, "the title could not be saved: {e:#}");
                meta.title = before;
            }
        }
        None => {
            // Loaded while this ran: the title reaches its meta at the next save.
            refined.lock().unwrap().insert(sid.to_string(), title.to_string());
            if let Ok(mut meta) = store.meta(sid).await {
                meta.title = title.to_string();
                let _ = store.save_meta(&meta).await;
            }
            if sessions.lock().unwrap().get(sid).is_none() {
                refined.lock().unwrap().remove(sid);
            }
        }
    }
}

impl super::Agent {
    /// Ask the model for a short title in the background; the meta, the file and the UI
    /// get it as soon as it is ready.
    pub(super) fn refine_title(&self, session_id: &str, text: String) {
        let (model, model_ref) = (self.model.clone(), self.config().model_ref);
        let (refined, sessions, store, events, sid) = (self.refined.clone(), self.sessions.clone(), self.store.clone(), self.events.clone(), session_id.to_string());
        tokio::spawn(async move {
            let Some(t) = summarize(model, &model_ref, &text).await else { return };
            apply_title(&sessions, &store, &refined, &sid, &t).await;
            let _ = events.send(AgentEvent::SessionUpdated { session_id: sid, title: t });
        });
    }

    /// Name sessions that still have a placeholder title (once, at startup).
    pub async fn backfill_titles(&self) {
        let Ok(list) = self.store.list().await else { return };
        for meta in list.into_iter().filter(|m| is_placeholder(&m.title) && m.message_count > 0) {
            let Ok(msgs) = self.store.load_messages(&meta.id).await else { continue };
            let Some(first) = msgs.iter().find(|m| m.is_user_turn() && meaningful(&m.content)) else { continue };
            let Some(q) = quick_title(&first.content) else { continue };
            // Never the copy of the list: it is older than what was saved since.
            apply_title(&self.sessions, &self.store, &self.refined, &meta.id, &q).await;
            self.refine_title(&meta.id, first.content.clone());
        }
    }
}

/// Titles finished in the background, applied to the in-memory session meta at its next
/// save (so a later save does not overwrite them).
pub type Refined = Arc<Mutex<HashMap<String, String>>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert!(is_placeholder("") && is_placeholder("New session") && is_placeholder("Voice session") && !is_placeholder("Swap usage"));
        assert_eq!(quick_title("Ok."), None);
        assert_eq!(quick_title("app"), None);
        assert_eq!(quick_title("What is using swap?").as_deref(), Some("What is using swap"));
        assert_eq!(quick_title("Hey KAgent, check the console I have opened with Claude Code and write to it").as_deref(), Some("Check the console I have opened with Claude Code"));
        assert_eq!(quick_title("okay, can we change this offloading? and more").as_deref(), Some("Can we change this offloading"));
        assert_eq!(clean_summary("\"Swap usage and reboot advice.\"\n").as_deref(), Some("Swap usage and reboot advice"));
        assert_eq!(clean_summary("Title: Signal message to Miha").as_deref(), Some("Signal message to Miha"));
        assert_eq!(clean_summary("Here is a much longer explanation that is clearly not a title at all, sorry"), None);
    }
}
