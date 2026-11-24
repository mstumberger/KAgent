//! The model port. `kagent-providers` implements it for every API type; the agent loop
//! only sees this trait, so tests can drive the loop with a scripted model.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::protocol::{Message, ToolCall, Usage};
use crate::tool::ToolSpec;

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
