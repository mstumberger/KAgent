//! Which model looks, and what "local" means for it (screen-watch.md sections 2 and 4).

use kagent_providers::config::ModelEntry;
use kagent_providers::ApiType;

use super::super::vision::{local_model, Api};
use crate::local_models::tests::{gpu, rig, MB};

fn add_model(r: &crate::local_models::tests::Rig, provider: &str, id: &str) {
    r.providers
        .update(provider, |p| p.models.push(ModelEntry { id: id.into(), name: String::new(), enabled: true, roles: vec!["chat".into()], smart: true, config: Default::default() }))
        .unwrap();
}

fn vision_setting(r: &crate::local_models::tests::Rig, model_ref: &str) {
    r.settings.write().unwrap().extra.insert("visionModel".into(), model_ref.into());
}

#[tokio::test]
async fn without_a_local_vision_model_the_refusal_names_the_page_and_the_recommendation() {
    let r = rig(gpu(12288, 10000), 64000).await;
    let e = local_model(&r.providers, &r.settings, &r.lm).await.unwrap_err();
    assert!(e.starts_with("Screen watch needs a local vision model: Settings -> Local models recommends ") && e.ends_with(" for this machine."), "{e}");
    // A chat model installed is not a vision model.
    r.ollama.install("qwen2.5:7b-instruct", 4000 * MB);
    assert!(local_model(&r.providers, &r.settings, &r.lm).await.is_err());
}

#[tokio::test]
async fn without_a_gpu_that_fits_anything_the_refusal_still_says_where_to_look() {
    let r = rig(gpu(1024, 1000), 2000).await;
    let e = local_model(&r.providers, &r.settings, &r.lm).await.unwrap_err();
    assert!(e.starts_with("Screen watch needs a local vision model: Settings -> Local models"), "{e}");
}

#[tokio::test]
async fn an_installed_vision_model_of_the_list_is_the_one_that_looks() {
    let r = rig(gpu(12288, 10000), 64000).await;
    r.ollama.install("gemma3:4b", 3300 * MB);
    let m = local_model(&r.providers, &r.settings, &r.lm).await.unwrap();
    assert_eq!((m.id.as_str(), m.api.clone(), m.base.as_str()), ("gemma3:4b", Api::Ollama, r.ollama.base.as_str()));
}

#[tokio::test]
async fn the_vision_setting_wins_when_its_provider_is_local() {
    let r = rig(gpu(12288, 10000), 64000).await;
    r.ollama.install("gemma3:4b", 3300 * MB);
    let p = r.providers.add_from_template("custom", Some("LM Studio".into()), Some("http://192.168.1.50:1234/v1".into()), Some("k".into()), Default::default(), Some(ApiType::OpenaiChatCompletions)).unwrap();
    add_model(&r, &p.id, "qwen2-vl");
    vision_setting(&r, &format!("{}::qwen2-vl", p.id));
    let m = local_model(&r.providers, &r.settings, &r.lm).await.unwrap();
    assert_eq!((m.id.as_str(), m.base.as_str(), m.api), ("qwen2-vl", "http://192.168.1.50:1234/v1", Api::OpenAi { key: "k".into() }));
}

#[tokio::test]
async fn a_cloud_vision_setting_is_never_used_for_a_watch() {
    let r = rig(gpu(12288, 10000), 64000).await;
    let p = r.providers.add_from_template("custom", Some("Cloud".into()), Some("https://api.example.com/v1".into()), Some("k".into()), Default::default(), Some(ApiType::OpenaiChatCompletions)).unwrap();
    add_model(&r, &p.id, "big-vision");
    vision_setting(&r, &format!("{}::big-vision", p.id));
    let e = local_model(&r.providers, &r.settings, &r.lm).await.unwrap_err();
    assert!(e.contains("needs a local vision model"), "the cloud model is not a way out: {e}");
    r.ollama.install("gemma3:4b", 3300 * MB);
    assert_eq!(local_model(&r.providers, &r.settings, &r.lm).await.unwrap().id, "gemma3:4b");
}

#[tokio::test]
async fn an_ollama_provider_on_another_server_is_not_local() {
    // The model is installed, but Ollama is at a public address: nothing is looked at there.
    let mut r = rig(gpu(12288, 10000), 64000).await;
    r.ollama.install("gemma3:4b", 3300 * MB);
    std::sync::Arc::get_mut(&mut r.lm).unwrap().base_override = Some("http://203.0.113.9:11434".into());
    assert!(local_model(&r.providers, &r.settings, &r.lm).await.is_err());
}
