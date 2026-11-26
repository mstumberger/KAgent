//! One path for notifications (foundations.md section 4). The desktop channel runs
//! `notify-send`; the Telegram channel goes through the sink the bots module registers
//! (S29), and nothing is sent over it until then.

use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Desktop,
    /// The paired Telegram chat (bots.md section 5), through `Notifier::set_telegram_sink`.
    Telegram,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub urgency: Urgency,
    pub channels: Vec<Channel>,
}

/// What the Telegram channel needs to show a notification; the bots module registers it.
#[async_trait]
pub trait TelegramSink: Send + Sync {
    /// Show it in the paired chat. A failure is logged; the caller is not failed.
    async fn notify(&self, notification: Notification);
}

pub struct Notifier {
    /// False when `KAGENT_NO_NOTIFY` is set (tests): the desktop channel is skipped.
    desktop: bool,
    /// The sink of the Telegram channel: `None` until the bots module registers it.
    telegram: std::sync::Mutex<Option<Arc<dyn TelegramSink>>>,
}

impl Default for Notifier {
    fn default() -> Self {
        Self { desktop: std::env::var_os("KAGENT_NO_NOTIFY").is_none(), telegram: std::sync::Mutex::new(None) }
    }
}

impl Notifier {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Register the sink of the Telegram channel (S29).
    pub fn set_telegram_sink(&self, sink: Arc<dyn TelegramSink>) {
        *self.telegram.lock().unwrap() = Some(sink);
    }

    /// Show the notification on each of its channels. A channel that fails is logged;
    /// the caller is not failed.
    pub async fn send(&self, notification: Notification) -> anyhow::Result<()> {
        if notification.channels.contains(&Channel::Telegram) {
            let sink = self.telegram.lock().unwrap().clone();
            if let Some(sink) = sink {
                sink.notify(notification.clone()).await;
            } else {
                tracing::debug!("Telegram channel: no sink is registered, nothing is sent");
            }
        }
        for argv in self.plan(&notification) {
            let status = tokio::process::Command::new(&argv[0])
                .args(&argv[1..])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;
            match status {
                Ok(s) if s.success() => {}
                Ok(s) => tracing::warn!("{} exited with {s}", argv[0]),
                Err(e) => tracing::warn!("{} failed: {e:#}", argv[0]),
            }
        }
        Ok(())
    }

    /// The commands to run: one per notification for the desktop channel, when it is
    /// requested and not disabled.
    fn plan(&self, notification: &Notification) -> Vec<Vec<String>> {
        if !self.desktop || !notification.channels.contains(&Channel::Desktop) {
            return Vec::new();
        }
        vec![desktop_argv(notification)]
    }
}

/// `notify-send` for a notification: the title is the summary, the body the text.
fn desktop_argv(n: &Notification) -> Vec<String> {
    ["notify-send", "-a", "KAgent", "--urgency", n.urgency.as_str()]
        .iter()
        .map(|s| s.to_string())
        .chain([n.title.clone(), n.body.clone()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(channels: &[Channel]) -> Notification {
        Notification {
            title: "Reminder: Stand up".into(),
            body: "It is 10:00".into(),
            urgency: Urgency::Normal,
            channels: channels.to_vec(),
        }
    }

    #[test]
    fn desktop_sink_builds_the_expected_argv() {
        let notifier = Notifier { desktop: true, ..Notifier::default() };
        let both = note(&[Channel::Desktop, Channel::Telegram]);
        assert_eq!(
            desktop_argv(&both),
            ["notify-send", "-a", "KAgent", "--urgency", "normal", "Reminder: Stand up", "It is 10:00"]
        );
        assert_eq!(notifier.plan(&both).len(), 1, "one desktop command");
        assert!(notifier.plan(&note(&[Channel::Telegram])).is_empty(), "Telegram is not a desktop command: it goes through its sink");
        let critical = Notification { urgency: Urgency::Critical, ..note(&[Channel::Desktop]) };
        assert!(desktop_argv(&critical).contains(&"critical".to_string()));
    }

    #[test]
    fn kagent_no_notify_skips_the_desktop_channel() {
        // `Notifier { desktop: false }` is what a set KAGENT_NO_NOTIFY gives at start.
        let notifier = Notifier { desktop: false, ..Notifier::default() };
        assert!(notifier.plan(&note(&[Channel::Desktop])).is_empty());
        assert_eq!(Notifier::default().desktop, std::env::var_os("KAGENT_NO_NOTIFY").is_none());
    }

    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<Notification>>);

    #[async_trait]
    impl TelegramSink for Sink {
        async fn notify(&self, notification: Notification) {
            self.0.lock().unwrap().push(notification);
        }
    }

    #[tokio::test]
    async fn the_telegram_channel_reaches_the_registered_sink_and_nothing_else_does() {
        let sink = Arc::new(Sink::default());
        // Until the bots module registers its sink, the channel is accepted and does nothing.
        Notifier { desktop: false, ..Notifier::default() }.send(note(&[Channel::Telegram])).await.unwrap();
        let notifier = Notifier { desktop: false, ..Notifier::default() };
        notifier.set_telegram_sink(sink.clone());
        notifier.send(note(&[Channel::Telegram])).await.unwrap();
        notifier.send(note(&[Channel::Desktop])).await.unwrap();
        let got = sink.0.lock().unwrap();
        assert_eq!(got.len(), 1, "the Telegram channel goes to the sink; a desktop notification does not");
        assert_eq!(got[0].channels, vec![Channel::Telegram]);
    }
}
