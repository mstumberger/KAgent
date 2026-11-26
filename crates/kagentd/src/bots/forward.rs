//! What goes to the chat by itself (bots.md sections 4 and 5): the Telegram channel of
//! the `Notifier` (reminders and results of automations), raised alerts, and approval
//! cards that waited their delay. Nothing is sent while the bot is off, has no token or
//! no paired chat (`Bots::served`), whoever asks.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kagent_core::permission::ApprovalRequest;
use kagent_core::protocol::AgentEvent;
use tokio::sync::broadcast;

use super::cards;
use super::telegram_api::{Button, Keyboard};
use super::Bots;
use crate::notify::{Notification, TelegramSink};
use crate::watch::{Alert, AlertPush};

/// The Telegram sink of the daemon: one per `Bots`, registered in `Bots::start`.
pub struct ToChat {
    pub bots: Arc<Bots>,
}

#[async_trait]
impl TelegramSink for ToChat {
    async fn notify(&self, notification: Notification) {
        let Some(pairing) = self.bots.served() else {
            return tracing::debug!("telegram: the bot is off, a notification is not sent");
        };
        self.bots.bot.say(pairing.chat_id, &format!("{}\n{}", notification.title, notification.body)).await;
    }
}

impl AlertPush for ToChat {
    /// Called where the alert is raised (under no lock of the manager): the message is sent in a task.
    fn raised(&self, alert: &Alert) {
        if let Some(chat) = self.bots.alert_goes(alert) {
            let (bots, alert) = (self.bots.clone(), alert.clone());
            tokio::spawn(async move {
                let keyboard = Keyboard::row([Button { text: "Ack".into(), callback_data: cards::ack_data(&alert.id) }]);
                if let Err(e) = bots.bot.card(chat, &cards::ack_data(&alert.id), &cards::alert_text(&alert), keyboard).await {
                    tracing::warn!("telegram: an alert was not pushed: {e}");
                }
            });
        }
    }
}

impl ToChat {
    /// The events of the daemon: a card that waits long enough goes to the chat, and a
    /// resolved card is forgotten.
    pub async fn watch_cards(self: Arc<Self>) {
        let mut events = self.bots.events.subscribe();
        loop {
            match events.recv().await {
                Ok(AgentEvent::ApprovalRequested(card)) => {
                    let me = self.clone();
                    tokio::spawn(async move { me.approval(card).await });
                }
                Ok(AgentEvent::ApprovalResolved { request_id, .. }) => self.bots.bot.card_gone(&request_id),
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    /// A card pending for `approvalForwardSec` goes to the chat while it still waits.
    async fn approval(&self, card: ApprovalRequest) {
        let delay = self.bots.section().approval_forward_sec;
        if !self.goes(&card) {
            return;
        }
        self.bots.clock.sleep(Duration::from_secs(u64::from(delay))).await;
        // Answered on the desktop meanwhile, or the owner switched things off: nothing is sent.
        if !self.goes(&card) || !self.bots.agent.pending().iter().any(|pending| pending.request_id == card.request_id) {
            return;
        }
        let Some(pairing) = self.bots.served() else { return };
        let keyboard = Keyboard::row([
            Button { text: "Allow once".into(), callback_data: cards::approval_data(&card.request_id, true) },
            Button { text: "Deny".into(), callback_data: cards::approval_data(&card.request_id, false) },
        ]);
        tracing::info!(tool = %card.tool, "telegram: an approval card is forwarded");
        if let Err(e) = self.bots.bot.card(pairing.chat_id, &card.request_id, &cards::approval_text(&card), keyboard).await {
            tracing::warn!("telegram: an approval card was not forwarded: {e}");
        }
    }

    /// Whether the card may go to the chat now: forwarding is on, the bot serves a chat,
    /// and the rule of bots.md section 4 allows this card. "Always" is never offered, so
    /// there is no third button to leave out.
    fn goes(&self, card: &ApprovalRequest) -> bool {
        let section = self.bots.section();
        section.forward_approvals && self.bots.served().is_some() && matches!(cards::forward_rule(card, section.allow_privileged), Ok(()))
    }
}

/// What a card's buttons carry, for a test that presses one.
#[cfg(test)]
pub(crate) fn button_data(keyboard: &Keyboard, label: &str) -> Option<String> {
    keyboard.0.iter().flatten().find(|button| button.text == label).map(|button| button.callback_data.clone())
}
