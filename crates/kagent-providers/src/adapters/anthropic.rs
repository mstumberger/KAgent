//! anthropic-messages adapter (`POST {base}/v1/messages`). Thinking blocks are kept
//! in `provider_state` and replayed on the next request, as the API requires.

use serde_json::{json, Value};
use tokio::sync::mpsc;

use kagent_core::model::{ModelDelta, ModelOptions, ModelResponse};
use kagent_core::protocol::{Message, Role, ToolCall, Usage};
use kagent_core::tool::ToolSpec;

use super::{deep_merge, parse_tool_arguments, reasoning_patch};
use crate::rules::ModelConfig;
use crate::sse::SseEvent;

/// ZCode stores anthropic bases with or without a trailing `/v1`; the endpoint is
/// always `<base>/v1/messages`.
pub fn messages_url(base: &str) -> String {
    format!("{}/v1/messages", base.trim_end_matches('/').trim_end_matches("/v1"))
}

pub struct BodyParams<'a> {
    pub model_id: &'a str,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub options: &'a ModelOptions,
    pub config: &'a ModelConfig,
}

pub fn build_body(p: &BodyParams) -> Value {
    let mut msgs: Vec<Value> = Vec::new();
    for m in p.messages {
        match m.role {
            Role::User => msgs.push(json!({"role": "user", "content": [{"type": "text", "text": m.content}]})),
            Role::Assistant => {
                let mut blocks: Vec<Value> = Vec::new();
                // Replay provider thinking blocks (with signatures) verbatim.
                if let Some(Value::Array(thinking)) = &m.provider_state {
                    blocks.extend(thinking.iter().cloned());
                }
                if !m.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": m.content}));
                }
                for c in &m.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": c.input,
                    }));
                }
                if !blocks.is_empty() {
                    msgs.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            Role::Tool => {
                // Anthropic takes tool results as user messages with tool_result blocks.
                let mut block = json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                });
                if m.is_error {
                    block["is_error"] = json!(true);
                }
                if let Some(last) = msgs.last_mut() {
                    if last["role"] == "user" && last["content"].is_array() {
                        last["content"].as_array_mut().unwrap().push(block);
                        continue;
                    }
                }
                msgs.push(json!({"role": "user", "content": [block]}));
            }
        }
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
        .collect();
    let mut body = json!({
        "model": p.model_id,
        "max_tokens": p.options.max_output_tokens.unwrap_or(p.config.max_output_tokens).max(1024),
        "stream": true,
    });
    if !p.system.is_empty() {
        body["system"] = json!(p.system);
    }
    if !msgs.is_empty() {
        body["messages"] = json!(msgs);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    if let Some(t) = p.options.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(patch) = reasoning_patch(p.config, crate::config::ApiType::AnthropicMessages, p.options) {
        deep_merge(&mut body, &patch);
    }
    deep_merge(&mut body, &p.config.request_patch);
    body
}

#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    args: String,
}

#[derive(Default)]
struct ThinkingBlock {
    thinking: String,
    signature: String,
}

/// Accumulates one streaming /v1/messages response.
#[derive(Default)]
pub struct Reducer {
    text: String,
    calls: BTreeMapIndex,
    thinking: Vec<ThinkingBlock>,
    current_block_type: Option<String>,
    current_index: u64,
    usage: Usage,
    stop_reason: Option<String>,
}

// Index-ordered accumulation (blocks arrive in order with an index).
#[derive(Default)]
struct BTreeMapIndex {
    calls: std::collections::BTreeMap<u64, PartialCall>,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, ev: &SseEvent, deltas: &mpsc::UnboundedSender<ModelDelta>) {
        let Some(v) = serde_json::from_str::<Value>(&ev.data).ok() else { return };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_start" => {
                if let Some(u) = v.pointer("/message/usage") {
                    self.usage.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                }
            }
            "content_block_start" => {
                self.current_index = v.get("index").and_then(Value::as_u64).unwrap_or(0);
                self.current_block_type = v
                    .pointer("/content_block/type")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if self.current_block_type.as_deref() == Some("tool_use") {
                    let entry = self.calls.calls.entry(self.current_index).or_default();
                    entry.id = v.pointer("/content_block/id").and_then(Value::as_str).unwrap_or_default().into();
                    entry.name = v.pointer("/content_block/name").and_then(Value::as_str).unwrap_or_default().into();
                } else if self.current_block_type.as_deref() == Some("thinking") {
                    self.thinking.push(ThinkingBlock::default());
                }
            }
            "content_block_delta" => {
                let Some(delta) = v.get("delta") else { return };
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or_default();
                        self.text.push_str(text);
                        let _ = deltas.send(ModelDelta::Text(text.to_string()));
                    }
                    "input_json_delta" => {
                        let partial = delta.get("partial_json").and_then(Value::as_str).unwrap_or_default();
                        if let Some(entry) = self.calls.calls.get_mut(&self.current_index) {
                            entry.args.push_str(partial);
                        }
                    }
                    "thinking_delta" => {
                        let text = delta.get("thinking").and_then(Value::as_str).unwrap_or_default();
                        if let Some(last) = self.thinking.last_mut() {
                            last.thinking.push_str(text);
                        }
                        let _ = deltas.send(ModelDelta::Reasoning(text.to_string()));
                    }
                    "signature_delta" => {
                        let sig = delta.get("thinking_signature").and_then(Value::as_str).unwrap_or_default();
                        if let Some(last) = self.thinking.last_mut() {
                            last.signature.push_str(sig);
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = v.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(u) = v.get("usage") {
                    self.usage.output_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(self.usage.output_tokens);
                }
            }
            _ => {}
        }
    }

    pub fn finish(self) -> ModelResponse {
        let tool_calls: Vec<ToolCall> = self
            .calls
            .calls
            .into_values()
            .map(|c| ToolCall { id: c.id, name: c.name, input: parse_tool_arguments(&c.args) })
            .collect();
        let provider_state = if self.thinking.is_empty() {
            None
        } else {
            Some(json!(self
                .thinking
                .iter()
                .map(|t| json!({"type": "thinking", "thinking": t.thinking, "signature": t.signature}))
                .collect::<Vec<_>>()))
        };
        ModelResponse {
            text: self.text,
            tool_calls,
            usage: self.usage,
            stop_reason: self.stop_reason,
            provider_state,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn one_tool() -> ToolSpec {
        ToolSpec { name: "sys".into(), description: "d".into(), input_schema: json!({"type": "object"}) }
    }

    struct Fx {
        tools: Vec<ToolSpec>,
        options: ModelOptions,
        config: ModelConfig,
    }

    fn fx() -> Fx {
        Fx { tools: vec![one_tool()], options: ModelOptions::default(), config: ModelConfig::default() }
    }

    fn params<'a>(f: &'a Fx, messages: &'a [Message]) -> BodyParams<'a> {
        BodyParams {
            model_id: "claude-x",
            system: "be brief",
            messages,
            tools: &f.tools,
            options: &f.options,
            config: &f.config,
        }
    }

    #[test]
    fn url_normalization() {
        assert_eq!(messages_url("https://api.anthropic.com/v1"), "https://api.anthropic.com/v1/messages");
        assert_eq!(messages_url("https://api.anthropic.com"), "https://api.anthropic.com/v1/messages");
        assert_eq!(messages_url("https://api.z.ai/api/anthropic"), "https://api.z.ai/api/anthropic/v1/messages");
    }

    #[test]
    fn body_maps_history() {
        let call = ToolCall { id: "t1".into(), name: "sys".into(), input: json!({"a": 1}) };
        let messages = vec![
            Message::user("hi"),
            Message::assistant("let me check", vec![call.clone()]),
            Message::tool_result(&call, "done", false),
        ];
        let f = fx();
        let body = build_body(&params(&f, &messages));
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["max_tokens"], 8192);
        assert_eq!(body["messages"][0]["content"][0]["text"], "hi");
        assert_eq!(body["messages"][1]["content"][0]["type"], "text");
        assert_eq!(body["messages"][1]["content"][1]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][1]["input"]["a"], 1);
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(body["tools"][0]["name"], "sys");
    }

    #[test]
    fn reducer_streams_text_thinking_and_call() {
        let mut r = Reducer::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        for data in [
            r#"{"type":"message_start","message":{"usage":{"input_tokens":7}}}"#,
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#,
            r#"{"type":"content_block_stop","index":0}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"sys"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"a\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"1}"}}"#,
            r#"{"type":"content_block_stop","index":1}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#,
        ] {
            r.feed(&SseEvent { event: None, data: data.into() }, &tx);
        }
        let out = r.finish();
        assert_eq!(out.text, "Hi");
        assert_eq!(out.usage.input_tokens, 7);
        assert_eq!(out.usage.output_tokens, 9);
        assert_eq!(out.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].input, json!({"a": 1}));
    }

    #[test]
    fn thinking_round_trips_into_provider_state() {
        let mut r = Reducer::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        for data in [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","thinking_signature":"sig1"}}"#,
        ] {
            r.feed(&SseEvent { event: None, data: data.into() }, &tx);
        }
        let out = r.finish();
        let state = out.provider_state.expect("thinking kept");
        assert_eq!(state[0]["signature"], "sig1");
        // And the body replays it.
        let messages = vec![Message { provider_state: Some(state), ..Message::assistant("answer", vec![]) }];
        let f = fx();
        let body = build_body(&params(&f, &messages));
        assert_eq!(body["messages"][0]["content"][0]["type"], "thinking");
    }
}
