//! Gemini Live (BidiGenerateContent) wire format: pure functions, no I/O.
//! Ported from ReelGen (`reelgen/kitt/service.py` `_gemini_schema`,
//! `frontend/src/lib/live.ts`) plus session resumption and context compression.

use serde_json::{json, Map, Value};

use kagent_core::protocol::{Message, Role};
use kagent_core::tool::ToolSpec;

pub const GEMINI_WS: &str =
    "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// The history of a session (voice.md section 4): this many messages, each cut to so
/// many characters, so many together.
pub const HISTORY_MESSAGES: usize = 12;
pub const HISTORY_MESSAGE_CHARS: usize = 600;
pub const HISTORY_TOTAL_CHARS: usize = 6000;
/// The custom vocabulary (voice.md section 5): at most this many terms, each 2 to 40 characters.
pub const VOCABULARY_MAX: usize = 100;
pub const VOCABULARY_MIN_CHARS: usize = 2;
pub const VOCABULARY_MAX_CHARS: usize = 40;

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
    /// What the live model may call: `think` alone, or the tools of `voice.toolGroups`.
    pub tools: &'a [ToolSpec],
    pub resume_handle: Option<&'a str>,
    /// The first connection of a session: its history follows as `clientContent` once
    /// the setup is complete (voice.md section 4).
    pub history: bool,
    /// Terms the transcription should know (voice.md section 5).
    pub vocabulary: &'a [String],
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
    if p.history {
        setup["historyConfig"] = json!({"initialHistoryInClientContent": true});
    }
    let vocabulary = vocabulary(p.vocabulary);
    if !vocabulary.is_empty() {
        setup["inputAudioTranscription"] = json!({"customVocabulary": vocabulary});
    }
    json!({"setup": setup})
}

/// The vocabulary as the setup carries it: trimmed, each once (case ignored), 2 to 40
/// characters, the first `VOCABULARY_MAX` in the order given.
pub fn vocabulary(terms: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for term in terms {
        let term = term.trim();
        let chars = term.chars().count();
        if (VOCABULARY_MIN_CHARS..=VOCABULARY_MAX_CHARS).contains(&chars) && !out.iter().any(|t| t.eq_ignore_ascii_case(term)) {
            out.push(term.to_string());
            if out.len() == VOCABULARY_MAX {
                break;
            }
        }
    }
    out
}

/// The one function of the live model when the chat model thinks (tools.md section 24).
/// Not in the tool registry: text turns and MCP never see it.
pub fn think_spec() -> ToolSpec {
    ToolSpec {
        name: "think".into(),
        description: "Hand the user's words to the brain (the main agent with memory, skills and tools) and get back the text to say aloud.".into(),
        input_schema: json!({
            "type": "object",
            "required": ["text"],
            "properties": {"text": {"type": "string", "description": "the user's exact words, in their language"}}
        }),
    }
}

/// A text turn to the live model. `complete`: the model answers after it.
pub fn client_content(role: &str, text: &str, complete: bool) -> Value {
    json!({"clientContent": {"turns": [{"role": role, "parts": [{"text": text}]}], "turnComplete": complete}})
}

pub fn text_input(text: &str) -> Value {
    client_content("user", text, true)
}

/// The reply of a brain turn whose `think` call the server had cancelled, so nobody
/// waits for its result: a text turn the model reads aloud (voice.md section 2).
pub fn say_frame(reply: &str) -> Value {
    client_content("user", &format!("[SAY] {reply}"), true)
}

/// The last user and assistant messages as `clientContent` frames (voice.md section 4):
/// roles `user` and `model`, oldest first, each cut to `HISTORY_MESSAGE_CHARS`
/// characters and `HISTORY_TOTAL_CHARS` together (the newest keep their room), the
/// last frame with `turnComplete: true`. The server does not answer these.
pub fn history_frames(messages: &[Message]) -> Vec<Value> {
    // Newest first: the newest messages keep their room.
    let picked: Vec<(bool, &str)> = messages
        .iter()
        .rev()
        .filter(|m| matches!(m.role, Role::User | Role::Assistant) && !m.content.trim().is_empty())
        .take(HISTORY_MESSAGES)
        .map(|m| (m.role == Role::Assistant, m.content.trim()))
        .collect();
    let mut left = HISTORY_TOTAL_CHARS;
    let mut frames = Vec::new();
    for (assistant, content) in picked {
        if left == 0 {
            break;
        }
        let text: String = content.chars().take(HISTORY_MESSAGE_CHARS.min(left)).collect();
        left -= text.chars().count();
        frames.push(client_content(if assistant { "model" } else { "user" }, &text, false));
    }
    frames.reverse();
    if let Some(last) = frames.last_mut() {
        last["clientContent"]["turnComplete"] = json!(true);
    }
    frames
}

pub fn audio_input(b64: &str) -> Value {
    json!({"realtimeInput": {"audio": {"mimeType": "audio/pcm;rate=16000", "data": b64}}})
}

/// One video frame (a screenshot, vision.md section 3): a JPEG, base64.
pub fn video_input(b64: &str) -> Value {
    json!({"realtimeInput": {"video": {"mimeType": "image/jpeg", "data": b64}}})
}

/// The results of the function calls of one `toolCall` message.
pub fn tool_response(responses: &[(&str, &str, Value)]) -> Value {
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
        let m = setup_message(&SetupParams { model: "gemini-3.8-live", voice: "Kore", instruction: "hi", tools: &tools, resume_handle: Some("h1"), history: false, vocabulary: &[] });
        let s = &m["setup"];
        assert_eq!(s["model"], "models/gemini-3.8-live");
        assert_eq!(s["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"], "Kore");
        assert_eq!(s["sessionResumption"]["handle"], "h1");
        assert!(s["contextWindowCompression"]["slidingWindow"].is_object());
        assert_eq!(s["tools"][0]["functionDeclarations"][0]["parameters"]["type"], "OBJECT");
        let none = setup_message(&SetupParams { model: "models/x", voice: "Kore", instruction: "", tools: &[], resume_handle: None, history: false, vocabulary: &[] });
        assert_eq!(none["setup"]["model"], "models/x");
        assert!(none["setup"].get("tools").is_none());
        assert_eq!(none["setup"]["sessionResumption"], json!({}));
        // Without a vocabulary the transcription stays as it was.
        assert_eq!(none["setup"]["inputAudioTranscription"], json!({}));
    }

    /// voice.md section 10: the golden setup of each mode (the history config on a first
    /// connection only, the vocabulary, the voice tool set).
    #[test]
    fn setup_of_each_mode() {
        let (think, info) = (think_spec(), spec("system_info"));
        let first = |tools: &[ToolSpec]| setup_message(&SetupParams { model: "gemini-3.8-live", voice: "Kore", instruction: "i", tools, resume_handle: None, history: true, vocabulary: &[] });
        let names = |m: &Value| m["setup"]["tools"][0]["functionDeclarations"].as_array().map(|d| d.iter().map(|d| d["name"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>()).unwrap_or_default();
        assert_eq!(names(&first(&[think.clone()])), ["think"], "routed: think alone");
        assert_eq!(names(&first(&[think.clone(), info.clone()])), ["think", "system_info"], "auto: think and the tools of voice.toolGroups");
        assert_eq!(names(&first(&[info.clone()])), ["system_info"], "live: the tools alone");
        assert_eq!(first(&[])["setup"]["historyConfig"], json!({"initialHistoryInClientContent": true}), "a first connection asks for the history in client content");
        let rolled = setup_message(&SetupParams { model: "gemini-3.8-live", voice: "Kore", instruction: "i", tools: &[], resume_handle: Some("h2"), history: false, vocabulary: &[] });
        assert!(rolled["setup"].get("historyConfig").is_none(), "a reconnection with a handle sends no history");
        assert_eq!(think_spec().input_schema["required"], json!(["text"]));
    }

    #[test]
    fn vocabulary_is_trimmed_unique_and_capped() {
        let terms = vec!["Marko".to_string(), " KAgent ".to_string(), "Kagent".into(), "a".into(), "x".repeat(41), "slow-machine".into()];
        assert_eq!(vocabulary(&terms), ["Marko", "KAgent", "slow-machine"]);
        let many: Vec<String> = (0..130).map(|i| format!("term{i}")).collect();
        assert_eq!(vocabulary(&many).len(), VOCABULARY_MAX);
        let setup = setup_message(&SetupParams { model: "m", voice: "Kore", instruction: "", tools: &[], resume_handle: None, history: false, vocabulary: &terms });
        assert_eq!(setup["setup"]["inputAudioTranscription"], json!({"customVocabulary": ["Marko", "KAgent", "slow-machine"]}));
    }

    #[test]
    fn say_and_history_frames() {
        let field = |frame: &Value, path: &str| frame.pointer(&format!("/clientContent/turns/0/{path}")).and_then(Value::as_str).unwrap_or("").to_string();
        let text = |frame: &Value| field(frame, "parts/0/text");
        assert_eq!(say_frame("It is 45 C."), json!({"clientContent": {"turns": [{"role": "user", "parts": [{"text": "[SAY] It is 45 C."}]}], "turnComplete": true}}));
        assert_eq!(text_input("typed"), client_content("user", "typed", true));
        assert!(history_frames(&[]).is_empty(), "nothing to send for a new session");
        let mut messages = vec![
            Message::user("q1"),
            Message::assistant("a1", vec![]),
            Message::tool_result(&kagent_core::protocol::ToolCall { id: "c".into(), name: "t".into(), input: json!({}) }, "ignored", false),
            Message::assistant("", vec![]),
        ];
        for i in 0..13 {
            messages.push(Message::user(format!("q{i}")));
            messages.push(Message::assistant(format!("a{i}"), vec![]));
        }
        messages.push(Message::user("x".repeat(1000)));
        let frames = history_frames(&messages);
        let last = frames.len() - 1;
        assert_eq!(frames.len(), 12, "the last 12 messages that carry text: tool results and empty answers are left out");
        assert_eq!([field(&frames[0], "role"), text(&frames[0]), field(&frames[1], "role"), text(&frames[1])].join(" "), "model a7 user q8", "roles `user` and `model`, oldest first");
        assert_eq!(text(&frames[last]).chars().count(), 600, "a message is cut to 600 characters");
        assert_eq!(frames.iter().map(|f| text(f).chars().count()).sum::<usize>(), 628, "6000 characters together: the budget leaves room for the short ones");
        // Twelve long messages: ten whole ones fit, the two oldest are dropped.
        let long: Vec<Message> = (0..12).map(|i| if i % 2 == 0 { Message::user("y".repeat(1000)) } else { Message::assistant("z".repeat(1000), vec![]) }).collect();
        let frames = history_frames(&long);
        let last = frames.len() - 1;
        assert_eq!((frames.len(), frames.iter().map(|f| text(f).chars().count()).sum::<usize>(), field(&frames[0], "role")), (10, HISTORY_TOTAL_CHARS, "user".to_string()));
        assert!(frames[..last].iter().all(|f| f["clientContent"]["turnComplete"] == json!(false)) && frames[last]["clientContent"]["turnComplete"] == json!(true), "only the last frame completes the turn");
    }

    fn spec(name: &str) -> ToolSpec {
        ToolSpec { name: name.into(), description: "d".into(), input_schema: json!({"type":"object","properties":{}}) }
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
