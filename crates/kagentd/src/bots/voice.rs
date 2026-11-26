//! Voice notes (bots.md section 3): a Gemini chat model transcribes one through
//! `generateContent`, as `inlineData` (`audio/ogg`).

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

use kagent_core::util::truncate_str;

/// What the model is asked.
pub const PROMPT: &str = "Transcribe this voice message in its language.";

/// What transcribes a voice note.
#[async_trait]
pub trait Transcriber: Send + Sync {
    /// The text of a voice note (`audio/ogg`).
    async fn transcribe(&self, audio: Vec<u8>) -> anyhow::Result<String>;
}

/// A Gemini chat model the owner enabled: where it is reached, and with which key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiModel {
    pub id: String,
    pub base_url: String,
    pub key: String,
    pub headers: Vec<(String, String)>,
}

/// Whether `model` at `base_url` is a Gemini chat model that takes audio: on Google's
/// address, and no live, speech, image or embedding variant.
pub fn is_gemini_chat(base_url: &str, model: &str) -> bool {
    let host = reqwest::Url::parse(base_url).ok().and_then(|u| u.host_str().map(str::to_string));
    let id = model.trim_start_matches("models/");
    let not_chat = ["-live", "native-audio", "-tts", "image", "embedding"].iter().any(|bad| id.contains(bad));
    host.as_deref() == Some("generativelanguage.googleapis.com") && id.starts_with("gemini-") && !not_chat
}

/// The native `generateContent` address: a Gemini provider's base URL is that of its
/// OpenAI-compatible API (`.../v1beta/openai`).
pub fn generate_content_url(base_url: &str, model: &str) -> String {
    let root = base_url.trim_end_matches('/');
    let root = root.strip_suffix("/openai").unwrap_or(root);
    format!("{root}/models/{}:generateContent", model.trim_start_matches("models/"))
}

/// Every Gemini chat model that is enabled and has a key, in the owner's order.
pub fn gemini_models(store: &kagent_providers::ProviderStore) -> Vec<GeminiModel> {
    store
        .enabled_models()
        .into_iter()
        .filter_map(|(provider, model, _)| {
            let entry = store.provider(&provider)?;
            is_gemini_chat(&entry.api.base_url, &model.id).then(|| GeminiModel {
                id: model.id.trim_start_matches("models/").to_string(),
                base_url: entry.api.base_url.clone(),
                key: entry.api_key.clone(),
                headers: entry.api.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            })
        })
        .filter(|model| !model.key.is_empty())
        .collect()
}

/// The one that transcribes: the chat model when it is one of them, else the first.
pub fn pick<'a>(models: &'a [GeminiModel], chat_model: &str) -> Option<&'a GeminiModel> {
    let wanted = kagent_providers::store::split_ref(chat_model).map(|(_, model)| model.trim_start_matches("models/"));
    models.iter().find(|model| Some(model.id.as_str()) == wanted).or_else(|| models.first())
}

/// Gemini through `generateContent`.
pub struct GeminiTranscriber {
    url: String,
    key: String,
    headers: Vec<(String, String)>,
}

impl GeminiTranscriber {
    pub fn new(model: &GeminiModel) -> Self {
        Self { url: generate_content_url(&model.base_url, &model.id), key: model.key.clone(), headers: model.headers.clone() }
    }
}

#[async_trait]
impl Transcriber for GeminiTranscriber {
    async fn transcribe(&self, audio: Vec<u8>) -> anyhow::Result<String> {
        let data = base64::engine::general_purpose::STANDARD.encode(&audio);
        let body = json!({ "contents": [{ "parts": [{ "text": PROMPT }, { "inlineData": { "mimeType": "audio/ogg", "data": data } }] }] });
        let http = reqwest::Client::builder().timeout(Duration::from_secs(60)).redirect(reqwest::redirect::Policy::none()).build()?;
        let mut request = http.post(&self.url).header("x-goog-api-key", &self.key).json(&body);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        // The key is in a header, not in the URL; a reqwest message is taken without its URL all the same.
        let response = request.send().await.map_err(|e| anyhow::anyhow!(e.without_url().to_string()))?;
        let status = response.status();
        let text = response.text().await.map_err(|e| anyhow::anyhow!(e.without_url().to_string()))?;
        if !status.is_success() {
            anyhow::bail!("Gemini answered {status}: {}", truncate_str(&text, 200));
        }
        transcript(&text)
    }
}

/// The spoken text of a `generateContent` answer; a thought part is not spoken.
pub fn transcript(answer: &str) -> anyhow::Result<String> {
    let value: Value = serde_json::from_str(answer).map_err(|e| anyhow::anyhow!("not generateContent's JSON: {e}"))?;
    let parts = value.pointer("/candidates/0/content/parts").and_then(Value::as_array).ok_or_else(|| anyhow::anyhow!("no candidate part in the answer"))?;
    let spoken: Vec<&str> = parts
        .iter()
        .filter(|part| part.get("thought").and_then(Value::as_bool) != Some(true))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    anyhow::ensure!(!spoken.is_empty(), "Gemini gave no transcript");
    Ok(spoken.join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use kagent_providers::ProviderStore;

    fn model(id: &str, base: &str) -> GeminiModel {
        GeminiModel { id: id.into(), base_url: base.into(), key: "k".into(), headers: Vec::new() }
    }

    #[test]
    fn gemini_chat_models_are_told_from_the_rest() {
        let google = "https://generativelanguage.googleapis.com/v1beta/openai";
        for good in ["gemini-2.5-flash", "models/gemini-3.8-flash", "gemini-flash-latest", "gemini-2.5-pro"] {
            assert!(is_gemini_chat(google, good), "{good}");
        }
        for (base, id) in [
            (google, "gemini-2.0-flash-live-001"),
            (google, "gemini-2.5-flash-native-audio-latest"),
            (google, "gemini-2.5-flash-preview-tts"),
            (google, "gemini-3-pro-image-preview"),
            (google, "gemini-embedding-001"),
            (google, "gpt-4o"),
            ("https://api.openai.com/v1", "gemini-2.5-flash"),
            ("https://proxy.example/v1beta/openai", "gemini-2.5-flash"),
            ("not a url", "gemini-2.5-flash"),
        ] {
            assert!(!is_gemini_chat(base, id), "{id} at {base}");
        }
    }

    #[test]
    fn the_native_address_and_the_choice() {
        assert_eq!(generate_content_url("https://generativelanguage.googleapis.com/v1beta/openai/", "models/gemini-2.5-flash"), "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent");
        assert_eq!(generate_content_url("https://generativelanguage.googleapis.com/v1beta", "gemini-2.5-flash"), "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent");
        let models = [model("gemini-2.5-pro", "u"), model("gemini-2.5-flash", "u")];
        assert_eq!(pick(&models, "gemini::gemini-2.5-flash").map(|m| m.id.as_str()), Some("gemini-2.5-flash"), "the chat model when it is one of them");
        assert_eq!(pick(&models, "ollama::qwen2.5:7b-instruct").map(|m| m.id.as_str()), Some("gemini-2.5-pro"), "else the first");
        assert_eq!(pick(&[], "gemini::gemini-2.5-flash"), None, "none enabled: voice notes need a Gemini model");
    }

    #[test]
    fn the_spoken_text_of_an_answer() {
        let answer = r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true},{"text":"Switch it off "},{"text":"and on again."}]}}]}"#;
        assert_eq!(transcript(answer).unwrap(), "Switch it off \nand on again.");
        assert!(transcript(r#"{"candidates":[{"content":{"parts":[{"text":"x","thought":true}]}}]}"#).unwrap_err().to_string().contains("no transcript"));
        assert!(transcript(r#"{"promptFeedback":{"blockReason":"SAFETY"}}"#).unwrap_err().to_string().contains("no candidate part"));
        assert!(transcript("not json").is_err());
    }

    #[tokio::test]
    async fn a_voice_note_goes_to_generate_content_as_inline_data() {
        use axum::extract::State;
        use axum::http::HeaderMap;
        use axum::response::{IntoResponse, Response};
        use axum::Router;
        use std::sync::Mutex;

        #[derive(Default)]
        struct Seen(Mutex<Option<(String, HeaderMap, Value)>>);
        async fn generate(State(seen): State<Arc<Seen>>, uri: axum::http::Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>) -> Response {
            *seen.0.lock().unwrap() = Some((uri.path().to_string(), headers, body));
            r#"{"candidates":[{"content":{"parts":[{"text":"Switch the router off and on."}]}}]}"#.into_response()
        }
        let seen = Arc::new(Seen::default());
        let app = Router::new().fallback(generate).with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1beta/openai", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });

        let transcriber = GeminiTranscriber::new(&GeminiModel { id: "gemini-2.5-flash".into(), base_url: base, key: "a-google-key".into(), headers: Vec::new() });
        assert_eq!(transcriber.transcribe(vec![0x4f, 0x67, 0x67, 0x53]).await.unwrap(), "Switch the router off and on.");
        let (path, headers, body) = seen.0.lock().unwrap().take().unwrap();
        assert!(path.ends_with("/v1beta/models/gemini-2.5-flash:generateContent"), "{path}");
        assert_eq!(headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()), Some("a-google-key"), "the key is in a header");
        assert_eq!(body["contents"][0]["parts"][0]["text"], PROMPT);
        assert_eq!(body["contents"][0]["parts"][1]["inlineData"], json!({"mimeType": "audio/ogg", "data": "T2dnUw=="}), "the note as inlineData (bots.md section 3)");
    }

    #[test]
    fn enabled_gemini_models_come_with_their_key_and_address() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap()).unwrap();
        assert!(gemini_models(&store).is_empty(), "nothing enabled with a key yet");
        store.add_from_template("gemini", None, None, Some("a-key".into()), Default::default()).unwrap();
        let models = gemini_models(&store);
        assert!(!models.is_empty(), "the Gemini template has chat models");
        assert!(models.iter().all(|m| m.base_url.contains("generativelanguage.googleapis.com") && m.key == "a-key" && m.id.starts_with("gemini-")), "{:?}",
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>());
        assert!(!models.iter().any(|m| m.id.contains("live") || m.id.contains("tts") || m.id.contains("image") || m.id.contains("embedding")), "{:?}",
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>());
        store.update("gemini", |p| p.api_key = String::new()).unwrap();
        assert!(gemini_models(&store).is_empty(), "a model without a key cannot be called");
    }
}
