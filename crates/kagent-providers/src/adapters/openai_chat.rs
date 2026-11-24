//! openai-chat-completions adapter (`POST {base}/chat/completions`).

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
    let mut msgs = Vec::new();
    if !p.system.is_empty() {
        msgs.push(json!({"role": "system", "content": p.system}));
    }
    for m in p.messages {
        match m.role {
            Role::User => msgs.push(json!({"role": "user", "content": m.content})),
            Role::Assistant => {
                let content = if m.content.is_empty() { Value::Null } else { json!(m.content) };
                let calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {"name": c.name, "arguments": c.input.to_string()}
                        })
                    })
                    .collect();
                let mut msg = json!({"role": "assistant", "content": content});
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                msgs.push(msg);
            }
            Role::Tool => {
                let name = m.tool_name.clone().unwrap_or_default();
                let mut msg = json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                });
                if !name.is_empty() {
                    msg["name"] = json!(name);
                }
                msgs.push(msg);
            }
        }
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
        .collect();
    let mut body = json!({
        "model": p.model_id,
        "messages": msgs,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    if let Some(max) = p.options.max_output_tokens.or(Some(p.config.max_output_tokens)) {
        body["max_tokens"] = json!(max);
    }
    if let Some(t) = p.options.temperature {
        body["temperature"] = json!(t);
    }
    if let Some(patch) = reasoning_patch(p.config, crate::config::ApiType::OpenaiChatCompletions, p.options) {
        deep_merge(&mut body, &patch);
    }
    deep_merge(&mut body, &p.config.request_patch);
    body
}

/// Accumulates one streaming response from chat/completions SSE events.
#[derive(Default)]
pub struct Reducer {
    text: String,
    tool_calls: BTreeMap<u64, ToolCall>,
    usage: Usage,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, ev: &SseEvent, deltas: &mpsc::UnboundedSender<ModelDelta>) {
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { return };
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            self.usage.input_tokens = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
            self.usage.output_tokens = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
        }
        let Some(choice) = v.get("choices").and_then(|c| c.get(0)) else { return };
        let Some(delta) = choice.get("delta") else { return };
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            self.text.push_str(text);
            let _ = deltas.send(ModelDelta::Text(text.to_string()));
        }
        let reasoning = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str);
        if let Some(text) = reasoning {
            let _ = deltas.send(ModelDelta::Reasoning(text.to_string()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for c in calls {
                let index = c.get("index").and_then(Value::as_u64).unwrap_or(0);
                let entry = self.tool_calls.entry(index).or_insert_with(|| ToolCall {
                    id: String::new(),
                    name: String::new(),
                    input: Value::Null,
                });
                if let Some(id) = c.get("id").and_then(Value::as_str) {
                    if !id.is_empty() {
                        entry.id = id.to_string();
                    }
                }
                if let Some(f) = c.get("function") {
                    if let Some(name) = f.get("name").and_then(Value::as_str) {
                        if !name.is_empty() {
                            entry.name = name.to_string();
                        }
                    }
                    // Arguments stream as string fragments; accumulate raw.
                    if let Some(args) = f.get("arguments").and_then(Value::as_str) {
                        if entry.input.is_null() {
                            entry.input = json!(args);
                        } else {
                            entry.input = json!(entry.input.as_str().unwrap_or_default().to_string() + args);
                        }
                    }
                }
            }
        }
    }

    pub fn finish(self) -> ModelResponse {
        let tool_calls = self
            .tool_calls
            .into_values()
            .filter(|c| !c.name.is_empty())
            .map(|mut c| {
                let raw = c.input.as_str().unwrap_or_default().to_string();
                c.input = parse_tool_arguments(&raw);
                c
            })
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
    use kagent_core::protocol::ToolCall;

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
            model_id: "test-model",
            system: "be brief",
            messages,
            tools: &f.tools,
            options: &f.options,
            config: &f.config,
        }
    }

    #[test]
    fn body_shape() {
        let mut assistant = Message::assistant("thinking out loud", vec![ToolCall {
            id: "call_1".into(),
            name: "system_info".into(),
            input: json!({"x": 1}),
        }]);
        assistant.provider_state = None;
        let messages = vec![
            Message::user("hi"),
            assistant,
            Message::tool_result(&ToolCall { id: "call_1".into(), name: "system_info".into(), input: json!({}) }, "result", false),
        ];
        let f = fx();
        let body = build_body(&params(&f, &messages));
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hi");
        assert_eq!(body["messages"][2]["tool_calls"][0]["function"]["name"], "system_info");
        assert_eq!(body["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"x\":1}");
        assert_eq!(body["messages"][3]["role"], "tool");
        assert_eq!(body["messages"][3]["tool_call_id"], "call_1");
        assert_eq!(body["tools"][0]["function"]["name"], "sys");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn reducer_accumulates_split_arguments() {
        let mut r = Reducer::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let events: Vec<SseEvent> = [
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"system_info","arguments":"{\"a\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1,\"b\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"2}"}}]}}]}"#,
            r#"{"choices":[{"delta":{}}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        ]
        .iter()
        .map(|d| SseEvent { event: None, data: d.to_string() })
        .collect();
        for ev in &events {
            r.feed(ev, &tx);
        }
        let out = r.finish();
        assert_eq!(out.text, "Hello");
        assert_eq!(out.usage.input_tokens, 10);
        assert_eq!(out.usage.output_tokens, 5);
        assert_eq!(out.tool_calls.len(), 1);
        assert_eq!(out.tool_calls[0].id, "call_1");
        assert_eq!(out.tool_calls[0].input, json!({"a": 1, "b": 2}));
    }

    #[test]
    fn reducer_invalid_json_becomes_raw() {
        let mut r = Reducer::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        for d in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"t","arguments":"not-json"}}]}}]}"#,
        ] {
            r.feed(&SseEvent { event: None, data: d.into() }, &tx);
        }
        let out = r.finish();
        assert_eq!(out.tool_calls[0].input, json!({"_raw": "not-json"}));
    }
}
