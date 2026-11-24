//! openai-responses adapter (`POST {base}/responses`).

use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::sync::mpsc;

use kagent_core::model::{ModelDelta, ModelOptions, ModelResponse};
use kagent_core::protocol::{Message, Role, ToolCall, Usage};
use kagent_core::tool::ToolSpec;

use super::{deep_merge, parse_tool_arguments, reasoning_patch};
use crate::rules::ModelConfig;
use crate::sse::SseEvent;

pub struct BodyParams<'a> {
    pub model_id: &'a str,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub options: &'a ModelOptions,
    pub config: &'a ModelConfig,
}

pub fn build_body(p: &BodyParams) -> Value {
    let mut input: Vec<Value> = Vec::new();
    for m in p.messages {
        match m.role {
            Role::User => input.push(json!({"role": "user", "content": [{"type": "input_text", "text": m.content}]})),
            Role::Assistant => {
                if !m.content.is_empty() {
                    input.push(json!({"role": "assistant", "content": [{"type": "output_text", "text": m.content}]}));
                }
                for c in &m.tool_calls {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": c.id,
                        "name": c.name,
                        "arguments": c.input.to_string(),
                    }));
                }
            }
            Role::Tool => {
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "output": m.content,
                }));
            }
        }
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.input_schema}))
        .collect();
    let mut body = json!({
        "model": p.model_id,
        "stream": true,
    });
    if !p.system.is_empty() {
        body["instructions"] = json!(p.system);
    }
    if !input.is_empty() {
        body["input"] = json!(input);
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    if let Some(max) = p.options.max_output_tokens.or(Some(p.config.max_output_tokens)) {
        body["max_output_tokens"] = json!(max);
    }
    if let Some(t) = p.options.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(patch) = reasoning_patch(p.config, crate::config::ApiType::OpenaiResponses, p.options) {
        deep_merge(&mut body, &patch);
    }
    deep_merge(&mut body, &p.config.request_patch);
    body
}

#[derive(Default)]
struct PartialCall {
    call_id: String,
    name: String,
    args: String,
}

/// Accumulates one streaming /responses SSE stream.
#[derive(Default)]
pub struct Reducer {
    text: String,
    calls: BTreeMap<String, PartialCall>,
    current: String,
    usage: Usage,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, ev: &SseEvent, deltas: &mpsc::UnboundedSender<ModelDelta>) {
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { return };
        let kind = ev.event.clone().or_else(|| v.get("type").and_then(Value::as_str).map(str::to_string));
        match kind.as_deref() {
            Some("response.output_text.delta") => {
                let text = v.get("delta").and_then(Value::as_str).unwrap_or_default();
                self.text.push_str(text);
                let _ = deltas.send(ModelDelta::Text(text.to_string()));
            }
            Some("response.reasoning_summary_text.delta") => {
                let text = v.get("delta").and_then(Value::as_str).unwrap_or_default();
                let _ = deltas.send(ModelDelta::Reasoning(text.to_string()));
            }
            Some("response.output_item.added") => {
                let item = v.get("item").cloned().unwrap_or(Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    // Deltas reference the item by `id`; history identifies the call
                    // by `call_id`. Track both (some servers omit `id`).
                    let item_id = item
                        .get("id")
                        .or_else(|| item.get("call_id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let call_id = item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let entry = self.calls.entry(item_id.clone()).or_default();
                    entry.call_id = call_id;
                    entry.name = item.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
                    self.current = item_id;
                }
            }
            Some("response.function_call_arguments.delta") => {
                let id = v.get("item_id").and_then(Value::as_str).unwrap_or_default().to_string();
                let entry = self.calls.entry(id).or_default();
                entry.args.push_str(v.get("delta").and_then(Value::as_str).unwrap_or_default());
            }
            Some("response.completed") | Some("response.failed") => {
                if let Some(u) = v.pointer("/response/usage") {
                    self.usage.input_tokens = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
                    self.usage.output_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
                }
            }
            _ => {}
        }
    }

    pub fn finish(self) -> ModelResponse {
        let tool_calls: Vec<ToolCall> = self
            .calls
            .into_values()
            .filter(|c| !c.name.is_empty())
            .map(|c| ToolCall { id: c.call_id, name: c.name, input: parse_tool_arguments(&c.args) })
            .collect();
        ModelResponse {
            text: self.text,
            tool_calls,
            usage: self.usage,
            stop_reason: None,
            provider_state: None,
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
            model_id: "gpt-x",
            system: "be brief",
            messages,
            tools: &f.tools,
            options: &f.options,
            config: &f.config,
        }
    }

    #[test]
    fn body_maps_history() {
        let call = ToolCall { id: "c1".into(), name: "sys".into(), input: json!({"a": 1}) };
        let messages = vec![
            Message::user("hi"),
            Message::assistant("checking", vec![call.clone()]),
            Message::tool_result(&call, "done", false),
        ];
        let f = fx();
        let body = build_body(&params(&f, &messages));
        assert_eq!(body["instructions"], "be brief");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][1]["role"], "assistant");
        assert_eq!(body["input"][2]["type"], "function_call");
        assert_eq!(body["input"][2]["call_id"], "c1");
        assert_eq!(body["input"][2]["arguments"], "{\"a\":1}");
        assert_eq!(body["input"][3]["type"], "function_call_output");
        assert_eq!(body["tools"][0]["type"], "function");
    }

    #[test]
    fn reducer_streams_text_and_call() {
        let mut r = Reducer::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        for (event, data) in [
            (None, r#"{"type":"response.output_item.added","item":{"type":"function_call","call_id":"c1","name":"sys"}}"#.to_string()),
            (None, r#"{"type":"response.function_call_arguments.delta","item_id":"c1","delta":"{\"a\":"}"#.to_string()),
            (None, r#"{"type":"response.function_call_arguments.delta","item_id":"c1","delta":"1}"}"#.to_string()),
            (None, r#"{"type":"response.output_text.delta","delta":"Ans"}"#.to_string()),
            (None, r#"{"type":"response.output_text.delta","delta":"wer"}"#.to_string()),
            (None, r#"{"type":"response.completed","response":{"usage":{"input_tokens":3,"output_tokens":4}}}"#.to_string()),
        ] {
            r.feed(&SseEvent { event, data }, &tx);
        }
        let out = r.finish();
        assert_eq!(out.text, "Answer");
        assert_eq!(out.usage.input_tokens, 3);
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "c1");
        assert_eq!(out.tool_calls[0].input, json!({"a": 1}));
    }
}
