//! What the bot needs from the agent (bots.md sections 3 and 4): sessions and turns,
//! approval cards, the health of the daemon and the open alerts. The daemon's
//! `DaemonAgent` reaches `Agent` and `AlertManager`; the tests use a fake (`fakes.rs`).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kagent_core::agent::Agent;
use kagent_core::permission::{ApprovalDecision, ApprovalRequest};
use kagent_core::protocol::{Role, TurnStatus, Via};
use kagent_core::session::SessionOrigin;
use serde_json::Value;

use super::voice::{GeminiTranscriber, Transcriber};
use crate::settings::Settings;
use crate::watch::AlertManager;

/// How the turn of a session ended: the last answer, also when it was cut short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub status: TurnStatus,
    pub error: Option<String>,
    pub text: String,
}

/// What `/status` says: the doctor score (when the check ran), the open alerts and the
/// running turns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub doctor: Option<u64>,
    pub open_alerts: usize,
    pub running: usize,
}

/// The agent as the bot sees it. The daemon's answers come from `Agent` and the alerts;
/// the tests script a fake.
#[async_trait]
pub trait BotAgent: Send + Sync {
    /// A new session of the chat (origin `telegram`).
    async fn new_session(&self, title: &str) -> anyhow::Result<String>;
    async fn session_exists(&self, session: &str) -> bool;
    /// Start a turn, or queue the text as steering while one runs.
    async fn send(&self, session: &str, text: &str) -> anyhow::Result<()>;
    /// Resolve once the session is idle, with how its last turn ended.
    async fn wait_idle(&self, session: &str) -> TurnEnd;
    /// Cancel every running turn; how many.
    async fn stop_all(&self) -> usize;
    /// Answer a waiting card; false when nobody waits for it.
    fn resolve(&self, request: &str, decision: ApprovalDecision) -> bool;
    fn pending(&self) -> Vec<ApprovalRequest>;
    async fn status(&self) -> Status;
    /// Acknowledge an alert; false when there is none.
    fn ack_alert(&self, id: &str) -> bool;
    /// What transcribes a voice note; `None` without a Gemini chat model.
    fn transcriber(&self) -> Option<Arc<dyn Transcriber>>;
}

/// The agent of the daemon.
pub struct DaemonAgent {
    pub agent: Arc<Agent>,
    pub alerts: Arc<AlertManager>,
    pub providers: Arc<kagent_providers::ProviderStore>,
    pub settings: Arc<std::sync::RwLock<Settings>>,
}

#[async_trait]
impl BotAgent for DaemonAgent {
    async fn new_session(&self, title: &str) -> anyhow::Result<String> {
        Ok(self.agent.store().create_with(title, SessionOrigin::Telegram, None).await?.id)
    }

    async fn session_exists(&self, session: &str) -> bool {
        self.agent.store().meta(session).await.is_ok()
    }

    async fn send(&self, session: &str, text: &str) -> anyhow::Result<()> {
        self.agent.send(session, text.to_string(), Via::Telegram).await.map(|_| ())
    }

    async fn wait_idle(&self, session: &str) -> TurnEnd {
        let result = self.agent.wait_idle(session).await;
        // The last assistant message is the answer, not the narration of the turn (the
        // rule of `tasks::execute` and `krunner::ask` since S17).
        let messages = self.agent.messages(session).await.unwrap_or_default();
        let last = messages.iter().rev().find(|m| m.role == Role::Assistant && !m.content.trim().is_empty()).map(|m| m.content.trim().to_string()).unwrap_or_default();
        let streamed = result.text.trim().to_string();
        let text = if streamed.is_empty() || last.is_empty() || !streamed.ends_with(&last) { streamed } else { last };
        TurnEnd { status: result.status, error: result.error, text }
    }

    async fn stop_all(&self) -> usize {
        self.agent.stop_all().await
    }

    fn resolve(&self, request: &str, decision: ApprovalDecision) -> bool {
        self.agent.broker().resolve(request, decision)
    }

    fn pending(&self) -> Vec<ApprovalRequest> {
        self.agent.broker().pending()
    }

    async fn status(&self) -> Status {
        let ctx = kagent_core::tool::ToolContext::standalone(Duration::from_secs(30), 24_000);
        let doctor = kagent_core::tools::timeline::checkup(&ctx).await.get("score").and_then(Value::as_u64);
        let mut running = 0;
        if let Ok(sessions) = self.agent.store().list().await {
            for s in &sessions {
                if self.agent.is_running(&s.id).await {
                    running += 1;
                }
            }
        }
        Status { doctor, open_alerts: self.alerts.open_count(), running }
    }

    fn ack_alert(&self, id: &str) -> bool {
        self.alerts.ack(id, Some("acknowledged from Telegram".into())).is_some()
    }

    fn transcriber(&self) -> Option<Arc<dyn Transcriber>> {
        let chat_model = self.settings.read().unwrap().chat_model.clone();
        let models = super::voice::gemini_models(&self.providers);
        super::voice::pick(&models, &chat_model).map(|model| Arc::new(GeminiTranscriber::new(model)) as Arc<dyn Transcriber>)
    }
}
