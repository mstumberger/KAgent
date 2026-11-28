//! The module's behaviour against a fake Ollama (local-models.md section 7).

use std::sync::{Arc, RwLock};

use kagent_core::platform::gpu::Gpu;
use kagent_providers::ProviderStore;
use serde_json::{json, Value};

use super::fake::FakeOllama;
use super::{Hardware, LocalModels};
use crate::settings::Settings;

pub const MB: u64 = 1024 * 1024;

pub struct Rig {
    pub lm: Arc<LocalModels>,
    pub ollama: FakeOllama,
    pub providers: Arc<ProviderStore>,
    pub settings: Arc<RwLock<Settings>>,
    pub events: tokio::sync::broadcast::Receiver<kagent_core::protocol::AgentEvent>,
    pub _dir: tempfile::TempDir,
}

pub fn gpu(total: u64, free: u64) -> Option<Gpu> {
    Some(Gpu { name: "NVIDIA GeForce RTX 3060".into(), vram_total_mb: total, vram_free_mb: free, source: "nvidia-smi".into() })
}

pub async fn rig(gpu: Option<Gpu>, ram: u64) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let providers = Arc::new(ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap());
    let settings = Arc::new(RwLock::new(Settings::defaults()));
    let (tx, events) = tokio::sync::broadcast::channel(256);
    let ollama = FakeOllama::start().await;
    let mut lm = LocalModels::new(providers.clone(), settings.clone(), tx);
    {
        let m = Arc::get_mut(&mut lm).unwrap();
        m.base_override = Some(ollama.base.clone());
        m.hardware = Hardware::Fixed { gpu, ram_total_mb: ram };
    }
    Rig { lm, ollama, providers, settings, events, _dir: dir }
}

fn row<'a>(v: &'a Value, id: &str) -> &'a Value {
    v["recommended"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap()
}

#[tokio::test]
async fn the_machine_answer_has_ollama_the_card_the_installed_and_the_list_with_fit() {
    let r = rig(gpu(12288, 9550), 64000).await;
    r.ollama.install("qwen2.5:7b-instruct", 4683 * MB);
    r.ollama.install("nomic-embed-text:latest", 274 * MB);
    r.ollama.install("mistral:7b", 4100 * MB);
    let v = r.lm.view().await;
    assert_eq!(v["ollama"], json!({"reachable": true, "version": "0.9.1", "baseUrl": r.ollama.base}));
    assert_eq!((v["gpu"]["name"].as_str(), v["gpu"]["vramTotalMb"].as_u64(), v["gpu"]["vramFreeMb"].as_u64(), v["ramTotalMb"].as_u64()), (Some("NVIDIA GeForce RTX 3060"), Some(12288), Some(9550), Some(64000)));
    assert_eq!(v["pulls"], json!([]));

    let installed = v["installed"].as_array().unwrap();
    assert_eq!(installed.len(), 3);
    assert_eq!((installed[0]["id"].clone(), installed[0]["sizeMb"].clone(), installed[0]["jobs"].clone(), installed[0]["listed"].clone()), (json!("qwen2.5:7b-instruct"), json!(4683), json!(["chat"]), json!(true)));
    assert_eq!(installed[2]["listed"], false);
    assert_eq!(installed[2]["jobs"], json!([]), "a model outside the list has no job here");

    let q7 = row(&v, "qwen2.5:7b-instruct");
    assert_eq!((q7["fit"].as_str(), q7["installed"].as_bool(), q7["fitNote"].is_null()), (Some("fits"), Some(true), true));
    let nomic = row(&v, "nomic-embed-text");
    assert_eq!((nomic["installed"].as_bool(), nomic["installedId"].as_str()), (Some(true), Some("nomic-embed-text:latest")), "the :latest tag is the listed model");
    let g12 = row(&v, "gemma3:12b");
    assert_eq!(g12["fit"], "tight");
    assert_eq!(g12["fitNote"], "2738 MB are in use by other programs at the moment");
    assert_eq!(row(&v, "llama3.1:8b")["recommendedFor"], json!(["chat"]));
    assert_eq!(row(&v, "gemma3:4b")["recommendedFor"], json!(["vision"]));
    assert_eq!(row(&v, "whisper-small")["downloadable"], false);
    assert_eq!(row(&v, "whisper-small")["recommendedFor"], json!([]));
}

#[tokio::test]
async fn no_graphics_card_is_slow_and_gpu_is_null() {
    let r = rig(None, 16000).await;
    let v = r.lm.view().await;
    assert!(v["gpu"].is_null());
    assert_eq!(row(&v, "qwen2.5:7b-instruct")["fit"], "slow");
    assert_eq!(row(&v, "qwen2.5:14b-instruct")["fit"], "tooBig");
    assert!(row(&v, "qwen2.5:7b-instruct")["fitNote"].is_null());
}

#[tokio::test]
async fn ollama_not_reachable_is_a_state_not_an_error() {
    let mut r = rig(gpu(8192, 8000), 32000).await;
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    Arc::get_mut(&mut r.lm).unwrap().base_override = Some(closed.clone());
    let v = r.lm.view().await;
    assert_eq!((v["ollama"]["reachable"].clone(), v["ollama"]["version"].clone(), v["ollama"]["baseUrl"].clone()), (json!(false), Value::Null, json!(closed)));
    assert_eq!(v["installed"], json!([]));
    assert_eq!(row(&v, "gemma3:4b")["installed"], false);
}

#[tokio::test]
async fn the_base_is_the_ollama_providers_without_its_v1() {
    let dir = tempfile::tempdir().unwrap();
    let providers = Arc::new(ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap());
    let (tx, _rx) = tokio::sync::broadcast::channel(8);
    let lm = LocalModels::new(providers.clone(), Arc::new(RwLock::new(Settings::defaults())), tx);
    assert_eq!(lm.base(), "http://127.0.0.1:11434", "no provider: the default");
    providers.add_from_template("ollama", None, Some("http://10.0.0.5:11434/v1".into()), None, Default::default(), None).unwrap();
    assert_eq!(lm.base(), "http://10.0.0.5:11434");
}

#[tokio::test]
async fn the_tool_answers_in_words_and_never_pulls() {
    let r = rig(gpu(12288, 9550), 64000).await;
    r.ollama.install("gemma3:4b", 3300 * MB);
    let cell: super::tool::LocalModelsCell = Default::default();
    let tool = super::tool::LocalModelsTool::new(cell.clone());
    use kagent_core::tool::Tool as _;
    assert_eq!(tool.contract().name, "local_models");
    assert!(tool.contract().read_only, "read only");
    assert_eq!(kagent_core::tools::group_of("local_models"), "agent");
    let text = super::tool::describe(&r.lm.view().await);
    assert!(text.contains("Ollama 0.9.1 is running"), "{text}");
    assert!(text.contains("NVIDIA GeForce RTX 3060, 12288 MB of memory, 9550 MB free now"), "{text}");
    assert!(text.contains("Gemma 3 4B (gemma3:4b), 3300 MB, fit: fits, RECOMMENDED, installed"), "{text}");
    assert!(text.contains("no tool calls (a helper, not the Brain)"), "{text}");
    assert!(text.contains("not usable yet") && text.contains("you cannot download it"), "{text}");
    assert!(r.ollama.starts().is_empty(), "the tool never pulls");
    let _ = cell.set(r.lm.clone());
}
