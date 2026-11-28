//! The model port. `kagent-providers` implements it for every API type; the agent loop
//! only sees this trait, so tests can drive the loop with a scripted model.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::protocol::{Message, ToolCall, Usage};
use crate::tool::ToolSpec;

/// Who a model call is for (usage.md section 2): the usage counters keep it per call. The loop sets it for a
/// turn with `CALL_SOURCE.scope`; a call outside a turn is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallSource {
    Chat,
    Voice,
    Subagent,
    Task,
    #[default]
    Other,
}

impl CallSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Voice => "voice",
            Self::Subagent => "subagent",
            Self::Task => "task",
            Self::Other => "other",
        }
    }

    /// The source of the turn a message started (text, Telegram, the watch and notes are the owner's chat).
    pub fn of_via(via: crate::protocol::Via) -> Self {
        use crate::protocol::Via;
        match via {
            Via::Text | Via::Telegram | Via::Watch | Via::Note => Self::Chat,
            Via::Voice => Self::Voice,
            Via::Task => Self::Task,
            Via::Agent => Self::Subagent,
        }
    }
}

tokio::task_local! {
    /// The source of the model calls of the running task (usage.md section 2).
    pub static CALL_SOURCE: CallSource;
}

/// The source of the calls made now: the turn's, else `Other`.
pub fn current_source() -> CallSource {
    CALL_SOURCE.try_with(|s| *s).unwrap_or_default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// `"<providerId>::<modelId>"`
    pub model_ref: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub options: ModelOptions,
}

#[derive(Debug, Clone)]
pub enum ModelDelta {
    Text(String),
    Reasoning(String),
}

#[derive(Debug, Clone, Default)]
pub struct ModelResponse {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub stop_reason: Option<String>,
    /// Opaque state to store on the assistant message (see `Message::provider_state`).
    pub provider_state: Option<Value>,
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn complete(
        &self,
        request: ModelRequest,
        deltas: mpsc::UnboundedSender<ModelDelta>,
        cancel: CancellationToken,
    ) -> anyhow::Result<ModelResponse>;
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::protocol::Via;

    #[tokio::test]
    async fn the_source_is_the_turns_and_other_outside() {
        assert_eq!(current_source(), CallSource::Other);
        assert_eq!(CALL_SOURCE.scope(CallSource::of_via(Via::Task), async { current_source() }).await, CallSource::Task);
        assert_eq!(CallSource::of_via(Via::Agent).as_str(), "subagent");
        assert_eq!(CallSource::of_via(Via::Telegram), CallSource::Chat);
        assert_eq!(CallSource::of_via(Via::Voice), CallSource::Voice);
    }
}
