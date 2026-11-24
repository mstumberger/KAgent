//! Ollama native `/api/chat` (NDJSON stream). Exists because Ollama's OpenAI endpoint
//! ignores `num_ctx`: it runs every model with the server default (often 4096) and
//! silently drops the start of longer prompts, system prompt included. Here the
//! context comes from the model config, so the agent and Ollama agree on the window.

use serde_json::{json, Value};
use tokio::sync::mpsc;

use kagent_core::model::{ModelDelta, ModelOptions, ModelResponse};
use kagent_core::protocol::{Message, Role, ToolCall, Usage};
use kagent_core::tool::ToolSpec;

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

pub fn chat_url(base: &str) -> String {
    let b = base.trim_end_matches('/').trim_end_matches("/v1").trim_end_matches("/api");
    format!("{b}/api/chat")
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
                let calls: Vec<Value> = m.tool_calls.iter().map(|c| json!({"function": {"name": c.name, "arguments": c.input}})).collect();
                let mut v = json!({"role": "assistant", "content": m.content});
                if !calls.is_empty() {
                    v["tool_calls"] = json!(calls);
                }
                msgs.push(v);
            }
            Role::Tool => msgs.push(json!({"role": "tool", "content": m.content, "tool_name": m.tool_name})),
        }
    }
    let tools: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.input_schema}}))
        .collect();
    let mut options = json!({"num_ctx": p.config.context_window.max(2048)});
    if let Some(n) = p.options.max_output_tokens {
        options["num_predict"] = json!(n);
    }
    if let Some(t) = p.options.temperature {
        options["temperature"] = json!(t);
    }
    let mut body = json!({"model": p.model_id, "messages": msgs, "stream": true, "options": options});
    if !tools.is_empty() {
        body["tools"] = json!(tools);
    }
    if let Some(level) = p.options.reasoning_level.as_deref() {
        // Thinking models (qwen3, deepseek-r1, gpt-oss) accept `think`.
        body["think"] = if level == "disabled" { json!(false) } else { json!(true) };
    }
    if !p.config.request_patch.is_null() {
        super::deep_merge(&mut body, &p.config.request_patch);
    }
    body
}

#[derive(Default)]
pub struct Reducer {
    text: String,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    error: Option<String>,
}

impl Reducer {
    pub fn new() -> Self {
        Self::default()
    }

    /// One NDJSON line per event (`data` = the line).
    pub fn feed(&mut self, ev: &SseEvent, deltas: &mpsc::UnboundedSender<ModelDelta>) {
        let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { return };
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            self.error = Some(e.to_string());
            return;
        }
        let msg = v.get("message");
        if let Some(t) = msg.and_then(|m| m.get("thinking")).and_then(Value::as_str).filter(|t| !t.is_empty()) {
            let _ = deltas.send(ModelDelta::Reasoning(t.to_string()));
        }
        if let Some(t) = msg.and_then(|m| m.get("content")).and_then(Value::as_str).filter(|t| !t.is_empty()) {
            self.text.push_str(t);
            let _ = deltas.send(ModelDelta::Text(t.to_string()));
        }
        // Ollama sends complete tool calls (arguments already an object).
        for c in msg.and_then(|m| m.get("tool_calls")).and_then(Value::as_array).into_iter().flatten() {
            let f = &c["function"];
            let input = match &f["arguments"] {
                Value::String(s) => super::parse_tool_arguments(s),
                Value::Null => json!({}),
                other => other.clone(),
            };
            self.tool_calls.push(ToolCall { id: c.get("id").and_then(Value::as_str).unwrap_or("").to_string(), name: f["name"].as_str().unwrap_or("").to_string(), input });
        }
        if v.get("done").and_then(Value::as_bool) == Some(true) {
            self.usage.input_tokens = v.get("prompt_eval_count").and_then(Value::as_u64).unwrap_or(0);
            self.usage.output_tokens = v.get("eval_count").and_then(Value::as_u64).unwrap_or(0);
        }
    }

    pub fn finish(self) -> anyhow::Result<ModelResponse> {
        if let Some(e) = self.error {
            anyhow::bail!("ollama: {e}");
        }
        Ok(ModelResponse { text: self.text, tool_calls: self.tool_calls.into_iter().filter(|c| !c.name.is_empty()).collect(), usage: self.usage, stop_reason: None, provider_state: None })
    }
}

/// Split a byte stream into NDJSON lines (partial lines carried over).
#[derive(Default)]
pub struct LineSplitter {
    buf: Vec<u8>,
}

impl LineSplitter {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            let s = String::from_utf8_lossy(&line).trim().to_string();
            if !s.is_empty() {
                out.push(SseEvent { event: None, data: s });
            }
        }
        out
    }
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let s = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).trim().to_string();
        if s.is_empty() { vec![] } else { vec![SseEvent { event: None, data: s }] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_body_and_stream() {
        assert_eq!(chat_url("http://127.0.0.1:11434/v1"), "http://127.0.0.1:11434/api/chat");
        assert_eq!(chat_url("http://127.0.0.1:11434"), "http://127.0.0.1:11434/api/chat");
        let call = ToolCall { id: "c1".into(), name: "system_info".into(), input: json!({}) };
        let msgs = vec![Message::user("hi"), Message::assistant("", vec![call.clone()]), Message::tool_result(&call, "{\"kernel\":\"7\"}", false)];
        let cfg = ModelConfig { context_window: 16384, ..Default::default() };
        let tools = vec![ToolSpec { name: "system_info".into(), description: "d".into(), input_schema: json!({"type":"object","properties":{}}) }];
        let b = build_body(&BodyParams { model_id: "qwen2.5:7b-instruct", system: "sys", messages: &msgs, tools: &tools, options: &ModelOptions::default(), config: &cfg });
        assert_eq!(b["options"]["num_ctx"], 16384);
        assert_eq!(b["messages"][2]["tool_calls"][0]["function"]["arguments"], json!({}));
        assert_eq!(b["messages"][3]["role"], "tool");
        assert_eq!(b["messages"][3]["tool_name"], "system_info");

        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut split = LineSplitter::default();
        let mut r = Reducer::new();
        let stream = b"{\"message\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"done\":false}\n{\"message\":{\"content\":\"lo\",\"tool_calls\":[{\"function\":{\"name\":\"kernel_info\",\"arguments\":{\"section\":\"version\"}}}]},\"done\":false}\n{\"message\":{\"content\":\"\"},\"done\":true,\"prompt_eval_count\":42,\"eval_count\":7}";
        for chunk in stream.chunks(17) {
            for ev in split.feed(chunk) {
                r.feed(&ev, &tx);
            }
        }
        for ev in split.finish() {
            r.feed(&ev, &tx);
        }
        let resp = r.finish().unwrap();
        assert_eq!(resp.text, "Hello");
        assert_eq!(resp.tool_calls[0].input, json!({"section": "version"}));
        assert_eq!((resp.usage.input_tokens, resp.usage.output_tokens), (42, 7));
        assert!(matches!(rx.try_recv(), Ok(ModelDelta::Text(_))));
        let mut bad = Reducer::new();
        bad.feed(&SseEvent { event: None, data: "{\"error\":\"model not found\"}".into() }, &tx);
        assert!(bad.finish().is_err());
    }
}
