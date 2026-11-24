//! Gemini Live (BidiGenerateContent) wire format: pure functions, no I/O.
//! Ported from ReelGen (`reelgen/kitt/service.py` `_gemini_schema`,
//! `frontend/src/lib/live.ts`) plus session resumption and context compression.

use serde_json::{json, Map, Value};

pub const GEMINI_WS: &str =
    "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// JSON Schema (our tool contracts) -> Gemini function-declaration schema: uppercase
/// types, only the keys Gemini accepts.
pub fn gemini_schema(schema: &Value) -> Value {
    let mut out = Map::new();
    if let Some(t) = schema.get("type").and_then(Value::as_str) {
        out.insert("type".into(), json!(t.to_uppercase()));
    }
    if let Some(d) = schema.get("description").and_then(Value::as_str) {
        out.insert("description".into(), json!(d));
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        let p: Map<String, Value> = props.iter().map(|(k, v)| (k.clone(), gemini_schema(v))).collect();
        out.insert("properties".into(), Value::Object(p));
    }
    if let Some(r) = schema.get("required").and_then(Value::as_array).filter(|r| !r.is_empty()) {
        out.insert("required".into(), Value::Array(r.clone()));
    }
    if let Some(items) = schema.get("items") {
        out.insert("items".into(), gemini_schema(items));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        // Gemini enums must be strings.
        out.insert("enum".into(), Value::Array(e.iter().map(|v| json!(v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect()));
    }
    // An OBJECT with no properties is rejected by the API; give it an empty map.
    if out.get("type") == Some(&json!("OBJECT")) && !out.contains_key("properties") {
        out.insert("properties".into(), json!({}));
    }
    Value::Object(out)
}

pub struct SetupParams<'a> {
    pub model: &'a str,
    pub voice: &'a str,
    pub instruction: &'a str,
    pub tools: &'a [kagent_core::tool::ToolSpec],
    pub resume_handle: Option<&'a str>,
}

pub fn setup_message(p: &SetupParams) -> Value {
    let decls: Vec<Value> = p
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "parameters": gemini_schema(&t.input_schema)}))
        .collect();
    let mut setup = json!({
        "model": format!("models/{}", p.model.trim_start_matches("models/")),
        "generationConfig": {
            "responseModalities": ["AUDIO"],
            "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": p.voice}}}
        },
        "systemInstruction": {"parts": [{"text": p.instruction}]},
        "inputAudioTranscription": {},
        "outputAudioTranscription": {},
        "realtimeInputConfig": {"automaticActivityDetection": {}},
        // Resumption keeps context across the ~10 min connection limit (goAway);
        // the sliding window keeps sessions alive past 15 min of audio.
        "sessionResumption": match p.resume_handle { Some(h) => json!({"handle": h}), None => json!({}) },
        "contextWindowCompression": {"slidingWindow": {}}
    });
    if !decls.is_empty() {
        setup["tools"] = json!([{"functionDeclarations": decls}]);
    }
    json!({"setup": setup})
}

pub fn audio_input(b64: &str) -> Value {
    json!({"realtimeInput": {"audio": {"mimeType": "audio/pcm;rate=16000", "data": b64}}})
}

pub fn text_input(text: &str) -> Value {
    json!({"clientContent": {"turns": [{"role": "user", "parts": [{"text": text}]}], "turnComplete": true}})
}

pub fn tool_response(responses: &[(String, String, Value)]) -> Value {
    let r: Vec<Value> = responses.iter().map(|(id, name, result)| json!({"id": id, "name": name, "response": {"result": result}})).collect();
    json!({"toolResponse": {"functionResponses": r}})
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    SetupComplete,
    Audio { data: String, rate: u32 },
    UserTranscript(String),
    AssistantTranscript(String),
    Interrupted,
    TurnComplete,
    ToolCalls(Vec<FunctionCall>),
    ToolCallCancellation(Vec<String>),
    ResumeHandle(String),
    GoAway,
    Error(String),
}

/// One server message can carry several things (audio + transcript + turnComplete).
pub fn translate(msg: &Value) -> Vec<ServerEvent> {
    let mut out = Vec::new();
    if msg.get("setupComplete").is_some() {
        out.push(ServerEvent::SetupComplete);
    }
    if let Some(sc) = msg.get("serverContent") {
        if sc.get("interrupted").and_then(Value::as_bool) == Some(true) {
            out.push(ServerEvent::Interrupted);
        }
        if let Some(t) = sc.pointer("/inputTranscription/text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            out.push(ServerEvent::UserTranscript(t.to_string()));
        }
        if let Some(t) = sc.pointer("/outputTranscription/text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            out.push(ServerEvent::AssistantTranscript(t.to_string()));
        }
        for part in sc.pointer("/modelTurn/parts").and_then(Value::as_array).into_iter().flatten() {
            if let Some(d) = part.pointer("/inlineData/data").and_then(Value::as_str) {
                let mime = part.pointer("/inlineData/mimeType").and_then(Value::as_str).unwrap_or("");
                let rate = mime.split("rate=").nth(1).and_then(|r| r.parse().ok()).unwrap_or(24_000);
                out.push(ServerEvent::Audio { data: d.to_string(), rate });
            }
        }
        if sc.get("turnComplete").and_then(Value::as_bool) == Some(true) {
            out.push(ServerEvent::TurnComplete);
        }
    }
    if let Some(calls) = msg.pointer("/toolCall/functionCalls").and_then(Value::as_array) {
        let fc = calls
            .iter()
            .map(|c| FunctionCall {
                id: c.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: c.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: c.get("args").cloned().unwrap_or(json!({})),
            })
            .collect();
        out.push(ServerEvent::ToolCalls(fc));
    }
    if let Some(ids) = msg.pointer("/toolCallCancellation/ids").and_then(Value::as_array) {
        out.push(ServerEvent::ToolCallCancellation(ids.iter().filter_map(Value::as_str).map(String::from).collect()));
    }
    if let Some(h) = msg.pointer("/sessionResumptionUpdate/newHandle").and_then(Value::as_str).filter(|h| !h.is_empty()) {
        if msg.pointer("/sessionResumptionUpdate/resumable").and_then(Value::as_bool) != Some(false) {
            out.push(ServerEvent::ResumeHandle(h.to_string()));
        }
    }
    if msg.get("goAway").is_some() {
        out.push(ServerEvent::GoAway);
    }
    if let Some(e) = msg.get("error") {
        out.push(ServerEvent::Error(e.to_string()));
    }
    out
}

/// base64 little-endian PCM16 -> samples (sign-correct: ReelGen's screeching bug).
pub fn decode_pcm16(b64: &str) -> Vec<i16> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default();
    bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::tool::ToolSpec;

    #[test]
    fn schema_conversion() {
        let s = json!({"type":"object","required":["pid"],"properties":{"pid":{"type":"integer","description":"x"},"sig":{"type":"string","enum":["TERM"]},"tags":{"type":"array","items":{"type":"string"}},"opts":{"type":"object","additionalProperties":{"type":"string"}}}});
        let g = gemini_schema(&s);
        assert_eq!(g["type"], "OBJECT");
        assert_eq!(g["properties"]["pid"]["type"], "INTEGER");
        assert_eq!(g["properties"]["tags"]["items"]["type"], "STRING");
        assert_eq!(g["properties"]["opts"], json!({"type":"OBJECT","properties":{}}));
        assert!(g["properties"]["opts"].get("additionalProperties").is_none());
        assert_eq!(gemini_schema(&json!({"type":"object","properties":{}})), json!({"type":"OBJECT","properties":{}}));
    }

    #[test]
    fn setup_golden() {
        let tools = vec![ToolSpec { name: "system_info".into(), description: "d".into(), input_schema: json!({"type":"object","properties":{}}) }];
        let m = setup_message(&SetupParams { model: "gemini-3.8-live", voice: "Kore", instruction: "hi", tools: &tools, resume_handle: Some("h1") });
        let s = &m["setup"];
        assert_eq!(s["model"], "models/gemini-3.8-live");
        assert_eq!(s["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"], "Kore");
        assert_eq!(s["sessionResumption"]["handle"], "h1");
        assert!(s["contextWindowCompression"]["slidingWindow"].is_object());
        assert_eq!(s["tools"][0]["functionDeclarations"][0]["parameters"]["type"], "OBJECT");
        let none = setup_message(&SetupParams { model: "models/x", voice: "Kore", instruction: "", tools: &[], resume_handle: None });
        assert_eq!(none["setup"]["model"], "models/x");
        assert!(none["setup"].get("tools").is_none());
        assert_eq!(none["setup"]["sessionResumption"], json!({}));
    }

    #[test]
    fn translates_server_messages() {
        let m = json!({"serverContent": {"modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": "AAA="}}]}, "outputTranscription": {"text": "Hi"}, "turnComplete": true}});
        assert_eq!(translate(&m), vec![ServerEvent::AssistantTranscript("Hi".into()), ServerEvent::Audio { data: "AAA=".into(), rate: 24000 }, ServerEvent::TurnComplete]);
        let t = json!({"toolCall": {"functionCalls": [{"id": "1", "name": "system_info", "args": {}}, {"id": "2", "name": "journal"}]}});
        match &translate(&t)[0] {
            ServerEvent::ToolCalls(c) => assert_eq!((c.len(), c[1].args.clone()), (2, json!({}))),
            e => panic!("{e:?}"),
        }
        assert_eq!(translate(&json!({"sessionResumptionUpdate": {"newHandle": "h", "resumable": true}})), vec![ServerEvent::ResumeHandle("h".into())]);
        assert!(translate(&json!({"sessionResumptionUpdate": {"newHandle": "h", "resumable": false}})).is_empty());
        assert_eq!(translate(&json!({"goAway": {"timeLeft": "5s"}})), vec![ServerEvent::GoAway]);
        assert_eq!(translate(&json!({"serverContent": {"interrupted": true}})), vec![ServerEvent::Interrupted]);
    }

    #[test]
    fn pcm_sign_extension() {
        use base64::Engine;
        let b = base64::engine::general_purpose::STANDARD.encode([0x00u8, 0x80, 0xff, 0x7f, 0xff, 0xff]);
        assert_eq!(decode_pcm16(&b), vec![-32768, 32767, -1]);
    }
}
