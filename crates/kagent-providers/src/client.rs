//! ProvidersClient: implements `kagent_core::model::ModelClient` over the registry.
//! Resolves the model ref, builds the request per API type, streams SSE with retry
//! on 429/5xx before the first byte and cancellation at any point.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};

use crate::adapters::{anthropic, ollama, openai_chat, openai_responses};
use crate::config::ApiType;
use crate::sse::SseParser;
use crate::store::{ResolvedModel, ProviderStore};

enum Adapter {
    Chat(openai_chat::Reducer),
    Anthropic(anthropic::Reducer),
    Responses(openai_responses::Reducer),
    Ollama(ollama::Reducer),
}

impl Adapter {
    fn feed(&mut self, ev: &crate::sse::SseEvent, deltas: &mpsc::UnboundedSender<ModelDelta>) {
        match self {
            Self::Chat(r) => r.feed(ev, deltas),
            Self::Anthropic(r) => r.feed(ev, deltas),
            Self::Responses(r) => r.feed(ev, deltas),
            Self::Ollama(r) => r.feed(ev, deltas),
        }
    }
    fn finish(self) -> anyhow::Result<ModelResponse> {
        Ok(match self {
            Self::Chat(r) => r.finish(),
            Self::Anthropic(r) => r.finish(),
            Self::Responses(r) => r.finish(),
            Self::Ollama(r) => r.finish()?,
        })
    }
}

pub struct ProvidersClient {
    store: Arc<ProviderStore>,
    http: reqwest::Client,
}

impl ProvidersClient {
    pub fn new(store: Arc<ProviderStore>) -> Self {
        Self {
            store,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .connect_timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
        }
    }

    fn endpoint(resolved: &ResolvedModel) -> String {
        let base = resolved.provider.api.base_url.trim_end_matches('/');
        match resolved.provider.api.api_type {
            ApiType::AnthropicMessages => anthropic::messages_url(base),
            ApiType::OpenaiChatCompletions => format!("{base}/chat/completions"),
            ApiType::OpenaiResponses => format!("{base}/responses"),
            ApiType::OllamaNative => ollama::chat_url(base),
        }
    }

    fn build(resolved: &ResolvedModel, req: &ModelRequest) -> (serde_json::Value, Adapter) {
        match resolved.provider.api.api_type {
            ApiType::OpenaiChatCompletions => {
                let body = openai_chat::build_body(&openai_chat::BodyParams {
                    model_id: &resolved.model.id,
                    system: &req.system,
                    messages: &req.messages,
                    tools: &req.tools,
                    options: &req.options,
                    config: &resolved.config,
                });
                (body, Adapter::Chat(openai_chat::Reducer::new()))
            }
            ApiType::AnthropicMessages => {
                let body = anthropic::build_body(&anthropic::BodyParams {
                    model_id: &resolved.model.id,
                    system: &req.system,
                    messages: &req.messages,
                    tools: &req.tools,
                    options: &req.options,
                    config: &resolved.config,
                });
                (body, Adapter::Anthropic(anthropic::Reducer::new()))
            }
            ApiType::OpenaiResponses => {
                let body = openai_responses::build_body(&openai_responses::BodyParams {
                    model_id: &resolved.model.id,
                    system: &req.system,
                    messages: &req.messages,
                    tools: &req.tools,
                    options: &req.options,
                    config: &resolved.config,
                });
                (body, Adapter::Responses(openai_responses::Reducer::new()))
            }
            ApiType::OllamaNative => {
                let body = ollama::build_body(&ollama::BodyParams {
                    model_id: &resolved.model.id,
                    system: &req.system,
                    messages: &req.messages,
                    tools: &req.tools,
                    options: &req.options,
                    config: &resolved.config,
                });
                (body, Adapter::Ollama(ollama::Reducer::new()))
            }
        }
    }

    fn request(&self, resolved: &ResolvedModel, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let url = Self::endpoint(resolved);
        let mut r = self.http.post(url);
        r = match resolved.provider.api.api_type {
            ApiType::AnthropicMessages => r
                .header("x-api-key", &resolved.provider.api_key)
                .header("anthropic-version", "2023-06-01"),
            _ => {
                if resolved.provider.api_key.is_empty() {
                    r
                } else {
                    r.bearer_auth(&resolved.provider.api_key)
                }
            }
        };
        for (k, v) in &resolved.provider.api.headers {
            r = r.header(k, v);
        }
        r.json(body)
    }

    async fn complete_inner(
        &self,
        request: ModelRequest,
        deltas: mpsc::UnboundedSender<ModelDelta>,
        cancel: CancellationToken,
    ) -> anyhow::Result<ModelResponse> {
        let resolved = self.store.resolve(&request.model_ref)?;
        if !resolved.provider.enabled {
            anyhow::bail!("provider '{}' is disabled", resolved.provider.id);
        }
        let (body, mut reducer) = Self::build(&resolved, &request);

        // Retries only before the first byte (429/5xx), twice with backoff.
        let mut response = None;
        for attempt in 0..3 {
            if cancel.is_cancelled() {
                anyhow::bail!("cancelled");
            }
            match self.request(&resolved, &body).send().await {
                Ok(r) if r.status().is_success() => {
                    response = Some(r);
                    break;
                }
                Ok(r) if (r.status().as_u16() == 429 || r.status().is_server_error()) && attempt < 2 => {
                    let status = r.status();
                    let detail = r.text().await.unwrap_or_default();
                    tracing::warn!(attempt, %status, "model request failed, retrying: {}", util::short(&detail));
                    tokio::time::sleep(Duration::from_millis(500 * (attempt as u64 + 1))).await;
                }
                Ok(r) => {
                    let status = r.status();
                    let detail = r.text().await.unwrap_or_default();
                    anyhow::bail!("provider error {status}: {}", util::short(&detail));
                }
                Err(e) => anyhow::bail!("request failed: {e:#}"),
            }
        }
        let response = response.expect("success path sets response");

        let mut stream = response.bytes_stream();
        let mut parser = SseParser::new();
        let mut lines = ollama::LineSplitter::default();
        let ndjson = resolved.provider.api.api_type == ApiType::OllamaNative;
        loop {
            let chunk = tokio::select! {
                c = stream.next() => c,
                _ = cancel.cancelled() => anyhow::bail!("cancelled"),
            };
            match chunk {
                Some(Ok(bytes)) => {
                    let events = if ndjson { lines.feed(&bytes) } else { parser.feed(&bytes) };
                    for ev in events {
                        reducer.feed(&ev, &deltas);
                    }
                }
                Some(Err(e)) => anyhow::bail!("stream error: {e:#}"),
                None => break,
            }
        }
        let rest = if ndjson { lines.finish() } else { parser.finish() };
        for ev in rest {
            reducer.feed(&ev, &deltas);
        }
        reducer.finish()
    }
}

#[async_trait]
impl ModelClient for ProvidersClient {
    async fn complete(
        &self,
        request: ModelRequest,
        deltas: mpsc::UnboundedSender<ModelDelta>,
        cancel: CancellationToken,
    ) -> anyhow::Result<ModelResponse> {
        self.complete_inner(request, deltas, cancel).await
    }
}

mod util {
    pub fn short(s: &str) -> String {
        let mut out: String = s.chars().take(300).collect();
        if out.len() < s.len() {
            out.push_str("...");
        }
        out.replace('\n', " ")
    }
}

// The reducer/parse layer is tested per adapter with fixtures; the model_config
// resolution is tested in rules.rs. Live Ollama round trip: tests/ollama.rs (KAGENT_IT=1).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BuiltinCatalog, ModelConfigPatch};
    use crate::store::ProviderStore;
    use kagent_core::protocol::Message;

    fn store_with_ollama(dir: &std::path::Path) -> Arc<ProviderStore> {
        let catalog: BuiltinCatalog =
            serde_json::from_str(include_str!("../../../config/providers.builtin.json")).unwrap();
        let s = ProviderStore::load(dir.join("providers.json"), catalog, &[]).unwrap();
        s.add_from_template("ollama", None, Some("http://127.0.0.1:1/v1".into()), None, Default::default(), None)
            .unwrap();
        s.update("ollama", |p| {
            p.models.push(crate::config::ModelEntry {
                id: "m".into(),
                name: String::new(),
                enabled: true,
                roles: vec!["chat".into()],
                smart: true,
                config: ModelConfigPatch::default(),
            })
        })
        .unwrap();
        Arc::new(s)
    }

    #[tokio::test]
    async fn cancel_before_first_byte_returns_fast() {
        let dir = tempfile::tempdir().unwrap();
        let client = ProvidersClient::new(store_with_ollama(dir.path()));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, _rx) = mpsc::unbounded_channel();
        // Start timing after client construction: only the request path must be fast.
        let start = std::time::Instant::now();
        let r = client
            .complete(
                ModelRequest {
                    model_ref: "ollama::m".into(),
                    system: String::new(),
                    messages: vec![Message::user("hi")],
                    tools: vec![],
                    options: Default::default(),
                },
                tx,
                cancel,
            )
            .await;
        assert!(r.is_err());
        // Bound is generous: the box may be under load; what matters is that
        // cancellation returns without touching the network (no connect timeout).
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn unreachable_provider_errors_clearly() {
        let dir = tempfile::tempdir().unwrap();
        let client = ProvidersClient::new(store_with_ollama(dir.path()));
        let (tx, _rx) = mpsc::unbounded_channel();
        let r = client
            .complete(
                ModelRequest {
                    model_ref: "ollama::m".into(),
                    system: String::new(),
                    messages: vec![Message::user("hi")],
                    tools: vec![],
                    options: Default::default(),
                },
                tx,
                CancellationToken::new(),
            )
            .await;
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(format!("{err:#}").contains("failed"));
    }
}
