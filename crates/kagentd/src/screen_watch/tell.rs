//! Telling the owner (screen-watch.md section 3): the session gets the message (assistant, `via: "watch"`);
//! with a live voice session open it is spoken as a `[SAY]` turn, else it is a desktop notification, and
//! Telegram gets it too when `screenWatch.telegram` is on. The same for a hit, the end of the time and a failure.

use async_trait::async_trait;
use kagent_core::protocol::{Message, Via};

use super::Watch;
use crate::notify::{Channel, Notification, Urgency};

/// The channels, as the daemon has them; the tests have fakes.
#[async_trait]
pub trait Deliver: Send + Sync {
    async fn session_message(&self, session_id: &str, message: Message) -> Result<(), String>;
    /// Speak into a live session. False: none is open.
    fn say(&self, session_id: &str, text: &str) -> bool;
    async fn notify(&self, notification: Notification);
    fn telegram_on(&self) -> bool;
}

/// What is spoken: the message without its `(goal: ...)` tail, which is for the screen.
pub fn spoken(text: &str) -> String {
    let cut = text.rfind(" (goal: ").map(|i| &text[..i]).unwrap_or(text);
    kagent_core::util::speakable(cut, 240)
}

pub async fn tell(d: &dyn Deliver, watch: &Watch, text: &str) {
    if let Err(e) = d.session_message(&watch.session_id, Message::assistant(text, Vec::new()).with_via(Via::Watch)).await {
        tracing::warn!("screen watch: the message could not be stored in the session: {e}");
    }
    let mut channels = Vec::new();
    if !d.say(&watch.session_id, &spoken(text)) {
        channels.push(Channel::Desktop);
    }
    if d.telegram_on() {
        channels.push(Channel::Telegram);
    }
    if !channels.is_empty() {
        d.notify(Notification { title: "Screen watch".into(), body: text.to_string(), urgency: Urgency::Normal, channels }).await;
    }
}

/// The note that a watch did not survive a restart: in its session, and nowhere else.
pub async fn restarted(d: &dyn Deliver, watch: &Watch, text: &str) {
    if let Err(e) = d.session_message(&watch.session_id, Message::assistant(text, Vec::new()).with_via(Via::Watch)).await {
        tracing::warn!("screen watch: the restart note could not be stored in the session: {e}");
    }
}

#[cfg(test)]
mod tests;
