//! The buttons of the cards the bot sent (bots.md sections 4 and 5): Allow once, Deny
//! and Ack. Part of `Bot`.

use std::sync::Arc;

use kagent_core::permission::ApprovalDecision;

use super::Bot;
use crate::bots::cards::{self, Press};
use crate::bots::telegram_api::{CallbackQuery, Markup};

impl Bot {
    /// A button of the bot was pressed.
    pub(super) async fn press(self: &Arc<Self>, query: &CallbackQuery) {
        let Some(message) = &query.message else { return };
        let chat = message.chat.id;
        if self.paired().is_none_or(|p| p.chat_id != chat) {
            return tracing::info!(chat, "telegram: a button press from a chat that is not the paired one is dropped");
        }
        let answer = match query.data.as_deref().and_then(cards::press) {
            Some(Press::Approval { request, allow }) => self.approve(chat, message.message_id, &request, allow).await,
            Some(Press::Ack { alert }) => self.ack(chat, message.message_id, &alert).await,
            None => "This button is not one of mine.".to_string(),
        };
        if let Err(e) = self.api().answer_callback(&query.id, Some(&answer)).await {
            tracing::warn!("telegram: a button press could not be answered: {e}");
        }
    }

    /// Allow once, or Deny, on a card this bot forwarded: only those can be answered from here.
    async fn approve(&self, chat: i64, message: i64, request: &str, allow: bool) -> String {
        let Some(card) = self.inner.lock().unwrap().cards.remove(request) else {
            // Answered before, on the desktop or by an earlier press.
            return "Already resolved".to_string();
        };
        let decision = if allow { ApprovalDecision::AllowOnce } else { ApprovalDecision::Deny { reason: Some("denied from Telegram".into()) } };
        let (outcome, answer) = match self.agent.resolve(request, decision) {
            true if allow => ("\n\nAllowed once from Telegram.", "Allowed once"),
            true => ("\n\nDenied from Telegram.", "Denied"),
            // The card was answered elsewhere a moment ago, or it timed out.
            false => ("\n\nAlready resolved.", "Already resolved"),
        };
        if let Err(e) = self.api().edit_message_text(chat, message, &format!("{}{outcome}", card.text), Markup::None, None).await {
            tracing::warn!("telegram: the card could not show its outcome: {e}");
        }
        tracing::info!(message, "telegram: a card was {answer} from the phone");
        answer.to_string()
    }

    async fn ack(&self, chat: i64, message: i64, alert: &str) -> String {
        let key = cards::ack_data(alert);
        let Some(card) = self.inner.lock().unwrap().cards.remove(&key) else {
            return "That alert is gone.".to_string();
        };
        if !self.agent.ack_alert(alert) {
            return "That alert is gone.".to_string();
        }
        if let Err(e) = self.api().edit_message_text(chat, message, &format!("{}\n\nAcknowledged from Telegram.", card.text), Markup::None, None).await {
            tracing::warn!("telegram: the alert could not show it was acknowledged: {e}");
        }
        "Acknowledged".to_string()
    }
}
