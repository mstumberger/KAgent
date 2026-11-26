//! Gated live test: run with `KAGENT_IT=1` against local Ollama
//! (qwen2.5:7b-instruct at http://127.0.0.1:11434/v1). Skipped otherwise.

use std::sync::Arc;

use kagent_core::model::{ModelClient, ModelRequest, ModelOptions};
use kagent_core::protocol::Message;
use kagent_core::tool::ToolSpec;
use kagent_providers::client::ProvidersClient;
use kagent_providers::config::{BuiltinCatalog, ModelConfigPatch, ModelEntry};
use kagent_providers::store::ProviderStore;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn ollama_tool_round_trip() {
    if std::env::var("KAGENT_IT").ok().as_deref() != Some("1") {
        eprintln!("skipping live Ollama test (set KAGENT_IT=1 to run)");
        return;
    }
    let home = std::env::temp_dir().join(format!("kagent-it-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let catalog: BuiltinCatalog =
        serde_json::from_str(include_str!("../../../config/providers.builtin.json")).unwrap();
    let store = Arc::new(ProviderStore::load(home.join("providers.json"), catalog, &[]).unwrap());
    if store.provider("ollama").is_none() {
        store.add_from_template("ollama", None, None, None, Default::default(), None).unwrap();
    }
    store
        .update("ollama", |p| {
            if !p.models.iter().any(|m| m.id == "qwen2.5:7b-instruct") {
                p.models.insert(
                    0,
                    ModelEntry {
                        id: "qwen2.5:7b-instruct".into(),
                        name: String::new(),
                        enabled: true,
                        roles: vec!["chat".into()],
                        smart: true,
                        config: ModelConfigPatch::default(),
                    },
                );
            }
        })
        .unwrap();

    let client = ProvidersClient::new(store);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = client
        .complete(
            ModelRequest {
                model_ref: "ollama::qwen2.5:7b-instruct".into(),
                system: "You must call the tool exactly once.".into(),
                messages: vec![Message::user("Call the system_info tool now.")],
                tools: vec![ToolSpec {
                    name: "system_info".into(),
                    description: "Get system information".into(),
                    input_schema: json!({"type": "object", "properties": {}}),
                }],
                options: ModelOptions::default(),
            },
            tx,
            CancellationToken::new(),
        )
        .await
        .expect("ollama round trip");
    let mut text = String::new();
    while let Ok(d) = rx.try_recv() {
        if let kagent_core::model::ModelDelta::Text(t) = d {
            text.push_str(&t);
        }
    }
    println!("text: {text:?} tool_calls: {:?}", out.tool_calls);
    assert!(
        !out.tool_calls.is_empty() || !text.is_empty(),
        "model produced neither tool call nor text"
    );
}
