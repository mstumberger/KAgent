//! The turns of the chat (bots.md section 3): the session, the placeholder and the
//! answer that replaces it, and voice notes. Part of `Bot`; the clock is passed in.

use std::sync::Arc;

use kagent_core::protocol::TurnStatus;

use super::{format, Bot, Working, NO_ANSWER, SESSION_TITLE, TURNS_PER_MINUTE, WORKING};
use crate::bots::telegram_api::{safe_file_path, unparsable, ApiError, Markup, Voice};
use crate::bots::TurnEnd;

/// What the chat gets for a turn that ended: the answer, or why there is none.
pub fn final_text(result: &TurnEnd) -> String {
    let text = result.text.trim();
    match result.status {
        TurnStatus::Completed if text.is_empty() => NO_ANSWER.to_string(),
        TurnStatus::Completed => text.to_string(),
        TurnStatus::Cancelled => format!("Stopped.{}", if text.is_empty() { String::new() } else { format!("\n\n{text}") }),
        TurnStatus::Failed => format!("The turn failed: {}", result.error.as_deref().map(str::trim).filter(|e| !e.is_empty()).unwrap_or("see the session in KAgent")),
    }
}

/// The answer as messages: no directive line, split for Telegram; one message when there is no text.
fn split_answer(markdown: &str) -> Vec<String> {
    let parts = format::split(&format::drop_directives(markdown), format::MAX_MESSAGE);
    if parts.is_empty() { vec![NO_ANSWER.to_string()] } else { parts }
}

fn placeholder(queued: usize) -> String {
    match queued {
        1 => format!("{WORKING} (1 more message queued)"),
        n => format!("{WORKING} ({n} more messages queued)"),
    }
}

impl Bot {
    /// The session of the chat, reused until `/new`; a new one when the old one is gone.
    async fn session(&self) -> anyhow::Result<String> {
        let known = self.inner.lock().unwrap().session.clone();
        if let Some(id) = known {
            if self.agent.session_exists(&id).await {
                return Ok(id);
            }
        }
        let id = self.agent.new_session(SESSION_TITLE).await?;
        self.inner.lock().unwrap().session = Some(id.clone());
        Ok(id)
    }

    /// Free text: a turn in the chat's session. While one runs, the message queues as steering.
    pub(super) async fn turn(self: &Arc<Self>, chat: i64, text: &str) {
        self.turn_with(chat, text, Vec::new()).await
    }

    /// `turn` with the pictures of the message (a Telegram photo): they are images of that message.
    pub(super) async fn turn_with(self: &Arc<Self>, chat: i64, text: &str, photos: Vec<Vec<u8>>) {
        let wait = self.inner.lock().unwrap().turns.reserve(self.clock.now());
        if !wait.is_zero() {
            return self.say(chat, &format!("That is more than {TURNS_PER_MINUTE} messages in a minute. Wait {} s, please.", wait.as_secs().max(1))).await;
        }
        let text = format::cut(text.trim(), format::MAX_INPUT);
        let session = match self.session().await {
            Ok(session) => session,
            Err(e) => return self.say(chat, &format!("Could not start a session: {e:#}")).await,
        };
        if self.inner.lock().unwrap().working.contains_key(&session) {
            return self.steer(chat, &session, text, photos).await;
        }
        // The chat counts as busy from here, before anything is awaited: a message that
        // comes next queues as steering, and a turn that ends at once (no model is
        // selected) cannot outrun its placeholder.
        self.inner.lock().unwrap().working.insert(session.clone(), Working::default());
        let placed = match self.api().send_message(chat, WORKING, Markup::None, None).await {
            Ok(sent) => Some(sent.message_id),
            Err(e) => {
                tracing::warn!("telegram: no placeholder: {e}");
                None
            }
        };
        if let Some(waiting) = self.inner.lock().unwrap().working.get_mut(&session) {
            waiting.message = placed;
        }
        if let Err(e) = self.hand_over(&session, text, photos).await {
            self.inner.lock().unwrap().working.remove(&session);
            return self.replace(chat, placed, &format!("Could not start: {e:#}")).await;
        }
        // A message that queued while the placeholder went out says so on it now.
        if self.inner.lock().unwrap().working.get(&session).is_some_and(|waiting| waiting.queued > 0) {
            self.note(chat, &session).await;
        }
        let bot = self.clone();
        tokio::spawn(async move { bot.answer(chat, session).await });
    }

    /// A message of a session whose turn runs: the agent takes it as steering, and the
    /// placeholder says so.
    async fn steer(&self, chat: i64, session: &str, text: &str, photos: Vec<Vec<u8>>) {
        if let Err(e) = self.hand_over(session, text, photos).await {
            return self.say(chat, &format!("Could not add the message: {e:#}")).await;
        }
        if let Some(waiting) = self.inner.lock().unwrap().working.get_mut(session) {
            waiting.queued += 1;
        }
        self.note(chat, session).await;
    }

    /// The message to the agent: text, and the pictures when there are some.
    async fn hand_over(&self, session: &str, text: &str, photos: Vec<Vec<u8>>) -> anyhow::Result<()> {
        if photos.is_empty() {
            self.agent.send(session, text).await
        } else {
            self.agent.send_images(session, text, photos).await
        }
    }

    /// The placeholder of the session's turn says how many messages queued behind it.
    async fn note(&self, chat: i64, session: &str) {
        let (message, queued) = match self.inner.lock().unwrap().working.get(session) {
            Some(waiting) => (waiting.message, waiting.queued),
            None => return,
        };
        let Some(message) = message else { return };
        if queued == 0 {
            return;
        }
        if let Err(e) = self.api().edit_message_text(chat, message, &placeholder(queued), Markup::None, None).await {
            tracing::warn!("telegram: the placeholder could not say a message queued: {e}");
        }
    }

    /// Wait for the session's turn to end, then replace its placeholder with the answer.
    async fn answer(self: Arc<Self>, chat: i64, session: String) {
        let result = self.agent.wait_idle(&session).await;
        let placed = self.inner.lock().unwrap().working.remove(&session).and_then(|waiting| waiting.message);
        if let Err(e) = self.deliver(chat, placed, &final_text(&result)).await {
            tracing::warn!("telegram: the answer could not be delivered: {e}");
        }
    }

    /// The answer in the chat: its first part replaces the placeholder, the rest are new messages.
    async fn deliver(&self, chat: i64, placed: Option<i64>, markdown: &str) -> Result<(), ApiError> {
        let mut edit = placed;
        for part in split_answer(markdown) {
            let sent = match edit {
                Some(message) => self.api().edit_message_text(chat, message, &format::to_html(&part), Markup::Html, None).await,
                None => self.api().send_message(chat, &format::to_html(&part), Markup::Html, None).await,
            };
            match sent {
                Ok(_) => edit = None,
                // Telegram would not parse the HTML: the same text, with no tag in it.
                Err(e) if unparsable(&e) => {
                    let text = format::to_plain(&part);
                    match edit {
                        Some(message) => self.api().edit_message_text(chat, message, &text, Markup::None, None).await?,
                        None => self.api().send_message(chat, &text, Markup::None, None).await?,
                    };
                    edit = None;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// One plain text in the chat: it replaces the message `placed`, or is a new message.
    async fn replace(&self, chat: i64, placed: Option<i64>, text: &str) {
        let sent = match placed {
            Some(message) => self.api().edit_message_text(chat, message, text, Markup::None, None).await,
            None => self.api().send_message(chat, text, Markup::None, None).await,
        };
        if let Err(e) = sent {
            tracing::warn!("telegram: not sent: {e}");
        }
    }

    /// A photo (vision.md section 2): downloaded, then an image of the message that starts the turn; the
    /// caption is its text. The window's limits apply (the store checks them), and so do its readers.
    pub(super) async fn photo(self: &Arc<Self>, chat: i64, m: &crate::bots::telegram_api::Message) {
        let Some(best) = m.best_photo() else {
            return self.say(chat, "That photo is over 20 MB, which is more than Telegram lets a bot download. Send a smaller one.").await;
        };
        let bytes: Result<Vec<u8>, String> = async {
            let file = self.api().get_file(&best.file_id).await.map_err(|e| e.to_string())?;
            let path = file.file_path.as_deref().and_then(safe_file_path).ok_or("Telegram gave no plain file path")?;
            self.api().download(path).await.map_err(|e| e.to_string())
        }
        .await;
        match bytes {
            Ok(bytes) => self.turn_with(chat, m.caption.as_deref().unwrap_or(""), vec![bytes]).await,
            Err(why) => {
                tracing::warn!("telegram: a photo was not downloaded: {why}");
                self.say(chat, "The photo could not be downloaded. Try again.").await
            }
        }
    }

    /// A voice note: transcribed when a Gemini chat model is enabled; the transcript is handled as text.
    pub(super) async fn voice(self: &Arc<Self>, chat: i64, voice: &Voice) {
        let Some(transcriber) = self.agent.transcriber() else {
            return self.say(chat, "Voice notes need a Gemini model. Enable one in Settings > Models, or type instead.").await;
        };
        let heard: Result<String, String> = async {
            let file = self.api().get_file(&voice.file_id).await.map_err(|e| e.to_string())?;
            let path = file.file_path.as_deref().and_then(safe_file_path).ok_or("Telegram gave no plain file path")?;
            let bytes = self.api().download(path).await.map_err(|e| e.to_string())?;
            transcriber.transcribe(bytes).await.map_err(|e| format!("{e:#}"))
        }
        .await;
        match heard {
            Ok(text) if !text.trim().is_empty() => self.turn(chat, text.trim()).await,
            Ok(_) => self.say(chat, "The voice note said nothing I could hear. Try again, or type instead.").await,
            Err(why) => {
                tracing::warn!("telegram: a voice note was not transcribed: {why}");
                self.say(chat, "The voice note could not be transcribed. Try again, or type instead.").await
            }
        }
    }
}
