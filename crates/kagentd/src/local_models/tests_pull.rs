//! Downloads, cancel and remove against the fake Ollama (local-models.md section 7).

use std::time::Duration;

use axum::http::StatusCode;
use kagent_core::protocol::AgentEvent;
use serde_json::{json, Value};

use super::fake::{happy, Step};
use super::http::http_error;
use super::pull::{Refusal, SPARE_MB};
use super::tests::{gpu, rig, Rig, MB};
use super::Disk;

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..250 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {what}");
}

fn pulls(r: &Rig) -> Vec<(String, String)> {
    r.lm.pull_rows().iter().map(|p| (p.id.clone(), serde_json::to_value(p.state).unwrap().as_str().unwrap().to_string())).collect()
}

async fn rig12() -> Rig {
    let mut r = rig(gpu(12288, 9550), 64000).await;
    std::sync::Arc::get_mut(&mut r.lm).unwrap().disk = Disk::Fixed(Some(500_000));
    r
}

fn model_row(r: &Rig, id: &str) -> Value {
    let p = r.providers.provider("ollama").expect("the Ollama provider");
    serde_json::to_value(p.models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("no row {id}"))).unwrap()
}

#[tokio::test]
async fn an_id_outside_the_list_is_refused_and_so_is_a_speech_model() {
    let r = rig12().await;
    let e = r.lm.pull("llama3:70b").await.unwrap_err();
    assert!(matches!(&e, Refusal::Invalid(m) if m.contains("not on the list")), "{e:?}");
    assert_eq!(http_error(e).0, StatusCode::BAD_REQUEST);
    let e = r.lm.pull("whisper-small").await.unwrap_err();
    assert!(matches!(&e, Refusal::Invalid(m) if m.contains("speech")), "{e:?}");
    assert!(r.ollama.starts().is_empty(), "Ollama was not asked");
    assert!(r.lm.pull_rows().is_empty());
}

#[tokio::test]
async fn a_pull_adds_the_model_with_the_roles_of_its_jobs_and_makes_it_nothing() {
    let r = rig12().await;
    r.ollama.script("gemma3:4b", happy(&[("aaa", 3000 * MB), ("bbb", 300 * MB)], 4));
    r.lm.pull("gemma3:4b").await.unwrap();
    until("the model to be installed and added", || r.ollama.has("gemma3:4b") && r.lm.pull_rows().is_empty() && r.providers.provider("ollama").is_some_and(|p| p.models.iter().any(|m| m.id == "gemma3:4b" && m.enabled))).await;
    assert_eq!(r.ollama.starts(), vec!["gemma3:4b"]);
    assert_eq!(r.ollama.seen_at("/api/pull")[0].2, json!({"model": "gemma3:4b", "stream": true}).to_string());

    // The first pull added the Ollama provider (at the address that was used).
    let p = r.providers.provider("ollama").unwrap();
    assert_eq!((p.template_id.as_str(), p.api.base_url.as_str()), ("ollama", r.ollama.base.as_str()));
    let row = model_row(&r, "gemma3:4b");
    assert_eq!((row["enabled"].clone(), row["roles"].clone()), (json!(true), json!(["chat"])), "vision and chat are the chat role");
    assert_eq!((row["config"]["supportsImage"].clone(), row["config"]["supportsToolCall"].clone()), (json!(true), json!(false)));
    // Nothing is switched to it.
    let s = r.settings.read().unwrap();
    assert!(s.chat_model.is_empty() && s.fast_model.is_empty() && s.fallback_model.is_empty() && !s.extra.contains_key("visionModel"));
    drop(s);

    // The page now sees it as installed, with its reference, and the role check lets it be the vision model.
    let v = r.lm.view().await;
    let g = v["recommended"].as_array().unwrap().iter().find(|m| m["id"] == "gemma3:4b").unwrap();
    assert_eq!((g["installed"].clone(), g["ref"].clone(), g["usedFor"].clone()), (json!(true), json!("ollama::gemma3:4b"), json!([])));
    crate::api::providers::check_model_roles(&r.providers, &json!({"visionModel": "ollama::gemma3:4b"})).ok().expect("usable as vision");
    assert!(crate::api::providers::check_model_roles(&r.providers, &json!({"chatModel": "ollama::gemma3:4b"})).is_ok(), "the role check is the same as for any chat model");
}

#[tokio::test]
async fn an_embedding_arrives_on_with_no_role_under_ollamas_latest_tag() {
    let r = rig12().await;
    let mut steps = happy(&[("e1", 274 * MB)], 2);
    steps.retain(|s| !matches!(s, Step::Install(_)));
    steps.insert(steps.len() - 1, Step::InstallAs("nomic-embed-text:latest".into(), 274 * MB));
    r.ollama.script("nomic-embed-text", steps);
    r.lm.pull("nomic-embed-text").await.unwrap();
    until("the row", || r.providers.provider("ollama").is_some_and(|p| p.models.iter().any(|m| m.id == "nomic-embed-text:latest"))).await;
    until("the row to be switched on", || model_row(&r, "nomic-embed-text:latest")["enabled"] == true).await;
    let row = model_row(&r, "nomic-embed-text:latest");
    assert_eq!(row["roles"], json!([]), "an embedding has no role");
    assert!(row["config"].get("supportsImage").is_none());
    assert!(crate::api::providers::check_model_roles(&r.providers, &json!({"chatModel": "ollama::nomic-embed-text:latest"})).is_err());
    // A second pull of it is refused: it is installed.
    let e = r.lm.pull("nomic-embed-text").await.unwrap_err();
    assert!(matches!(&e, Refusal::Conflict(m) if m.contains("already installed")), "{e:?}");
}

#[tokio::test]
async fn progress_is_the_sum_of_the_layers_and_events_come_at_most_twice_a_second() {
    let mut r = rig12().await;
    // 12 progress lines 250 ms apart: three seconds of stream.
    let mut steps = vec![Step::Line(json!({"status": "pulling manifest"}))];
    for c in 1..=12u64 {
        steps.push(Step::Wait(Duration::from_millis(250)));
        steps.push(Step::Line(json!({"status": "pulling aaa", "digest": "sha256:aaa", "total": 3000 * MB, "completed": 250 * MB * c})));
        steps.push(Step::Line(json!({"status": "pulling bbb", "digest": "sha256:bbb", "total": 300 * MB, "completed": 25 * MB * c})));
    }
    steps.push(Step::Install(3300 * MB));
    steps.push(Step::Line(json!({"status": "success"})));
    r.ollama.script("gemma3:4b", steps);
    let started = std::time::Instant::now();
    r.lm.pull("gemma3:4b").await.unwrap();
    // Halfway: the sum of both layers.
    until("some progress", || r.lm.pull_rows().first().is_some_and(|p| p.done_mb >= 1650)).await;
    let mid = r.lm.pull_rows()[0].clone();
    assert_eq!((mid.state, mid.total_mb), (super::PullState::Pulling, 3300));
    assert!(mid.done_mb >= 1650 && mid.done_mb < 3300, "{mid:?}");
    until("the end", || r.ollama.has("gemma3:4b") && r.lm.pull_rows().is_empty()).await;
    let secs = started.elapsed().as_secs_f64();
    let mut progress = 0;
    let mut last_done = 0;
    while let Ok(ev) = r.events.try_recv() {
        if let AgentEvent::LocalModelsChanged { pulls } = ev {
            if let Some(p) = pulls.as_array().and_then(|a| a.first()).filter(|p| p["state"] == "pulling" && p["doneMb"].as_u64().unwrap_or(0) > 0) {
                progress += 1;
                let d = p["doneMb"].as_u64().unwrap();
                assert!(d >= last_done, "progress never goes back");
                last_done = d;
            }
        }
    }
    assert!(progress >= 2, "progress is reported: {progress}");
    assert!(progress as f64 <= secs * 2.0 + 2.0, "{progress} progress events in {secs:.1} s is more than twice a second");
}

#[tokio::test]
async fn not_enough_disk_is_refused_with_the_numbers_before_anything_is_asked_of_ollama() {
    let mut r = rig12().await;
    let need = 3300 + SPARE_MB;
    std::sync::Arc::get_mut(&mut r.lm).unwrap().disk = Disk::Fixed(Some(need - 1));
    let e = r.lm.pull("gemma3:4b").await.unwrap_err();
    let Refusal::Conflict(m) = &e else { panic!("{e:?}") };
    assert!(m.contains(&format!("{} MB are free", need - 1)) && m.contains(&format!("{need} MB are needed")), "{m}");
    assert_eq!(http_error(e).0, StatusCode::CONFLICT);
    assert!(r.ollama.starts().is_empty() && r.lm.pull_rows().is_empty());
    // Exactly enough is enough; an unknown free space is not a refusal.
    r.ollama.script("gemma3:4b", happy(&[("aaa", 100 * MB)], 1));
    std::sync::Arc::get_mut(&mut r.lm).unwrap().disk = Disk::Fixed(Some(need));
    r.lm.pull("gemma3:4b").await.unwrap();
    until("done", || r.ollama.has("gemma3:4b")).await;
    until("quiet", || r.lm.pull_rows().is_empty()).await;
    r.ollama.script("gemma3:12b", happy(&[("ccc", 100 * MB)], 1));
    std::sync::Arc::get_mut(&mut r.lm).unwrap().disk = Disk::Fixed(None);
    r.lm.pull("gemma3:12b").await.unwrap();
    until("done", || r.ollama.has("gemma3:12b")).await;
}

#[tokio::test]
async fn one_pull_runs_at_a_time_and_the_others_wait_their_turn() {
    let r = rig12().await;
    for id in ["gemma3:4b", "llama3.1:8b"] {
        let mut s = vec![Step::Line(json!({"status": "pulling manifest"})), Step::Gate];
        s.extend(happy(&[("aaa", 100 * MB)], 1));
        r.ollama.script(id, s);
    }
    r.lm.pull("gemma3:4b").await.unwrap();
    r.lm.pull("llama3.1:8b").await.unwrap();
    let e = r.lm.pull("llama3.1:8b").await.unwrap_err();
    assert!(matches!(&e, Refusal::Conflict(m) if m.contains("waiting in the queue")), "{e:?}");
    until("the first to start", || r.ollama.starts().len() == 1).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(r.ollama.starts(), vec!["gemma3:4b"], "the second has not started");
    assert_eq!(pulls(&r), vec![("gemma3:4b".to_string(), "pulling".to_string()), ("llama3.1:8b".to_string(), "queued".to_string())]);
    r.ollama.release();
    until("the second to start", || r.ollama.starts().len() == 2).await;
    assert!(r.ollama.has("gemma3:4b"), "the first was finished before the second began");
    r.ollama.release();
    until("both done", || r.ollama.has("llama3.1:8b") && r.lm.pull_rows().is_empty()).await;
    assert_eq!(r.ollama.starts(), vec!["gemma3:4b", "llama3.1:8b"]);
}

#[tokio::test]
async fn cancel_stops_the_running_pull_and_a_queued_one_leaves_the_queue() {
    let r = rig12().await;
    r.ollama.script("gemma3:4b", vec![Step::Line(json!({"status": "pulling manifest"})), Step::Hang]);
    r.ollama.script("llama3.1:8b", happy(&[("aaa", 100 * MB)], 1));
    r.lm.pull("gemma3:4b").await.unwrap();
    r.lm.pull("llama3.1:8b").await.unwrap();
    until("the first to run", || r.ollama.starts().len() == 1).await;
    // The queued one leaves; nothing was asked of Ollama for it.
    r.lm.cancel("llama3.1:8b").unwrap();
    assert_eq!(pulls(&r), vec![("gemma3:4b".to_string(), "pulling".to_string())]);
    // The running one: the connection is closed (Ollama keeps the partial layers) and the row goes.
    r.lm.cancel("gemma3:4b").unwrap();
    until("the row to go", || r.lm.pull_rows().is_empty()).await;
    until("the connection to close", || r.ollama.dropped() == vec!["gemma3:4b"]).await;
    assert_eq!(r.ollama.starts(), vec!["gemma3:4b"]);
    assert!(!r.ollama.has("gemma3:4b"));
    assert!(matches!(r.lm.cancel("gemma3:4b").unwrap_err(), Refusal::Missing(_)));
    // The queue is free again: a new pull starts.
    r.lm.pull("llama3.1:8b").await.unwrap();
    until("the next pull", || r.ollama.has("llama3.1:8b")).await;
}

#[tokio::test]
async fn a_failed_pull_keeps_its_error_and_retry_replaces_the_row() {
    let r = rig12().await;
    r.ollama.script("gemma3:4b", vec![Step::Line(json!({"status": "pulling manifest"})), Step::Line(json!({"error": "pull model manifest: Get registry: connection reset"}))]);
    r.lm.pull("gemma3:4b").await.unwrap();
    until("the failure", || r.lm.pull_rows().first().is_some_and(|p| p.state == super::PullState::Failed)).await;
    let v = r.lm.view().await;
    assert_eq!(v["pulls"][0]["error"], "pull model manifest: Get registry: connection reset");
    assert_eq!(v["pulls"][0]["state"], "failed");
    // A stream that just ends is a failure too, with a way forward in its text.
    r.ollama.script("llama3.1:8b", vec![Step::Line(json!({"status": "pulling manifest"}))]);
    r.lm.pull("llama3.1:8b").await.unwrap();
    until("the second failure", || r.lm.pull_rows().iter().filter(|p| p.state == super::PullState::Failed).count() == 2).await;
    let ended = r.lm.pull_rows().into_iter().find(|p| p.id == "llama3.1:8b").unwrap();
    assert!(ended.error.unwrap().contains("closed the connection"));
    // Retry: the same id again replaces the failed row and now succeeds.
    r.ollama.script("gemma3:4b", happy(&[("aaa", 100 * MB)], 1));
    r.lm.pull("gemma3:4b").await.unwrap();
    until("the retry", || r.ollama.has("gemma3:4b") && !r.lm.pull_rows().iter().any(|p| p.id == "gemma3:4b")).await;
    // A failed row is dismissed by the cancel route.
    r.lm.cancel("llama3.1:8b").unwrap();
    assert!(r.lm.pull_rows().is_empty());
}

#[tokio::test]
async fn ollama_that_does_not_know_the_model_says_so_on_the_row() {
    let r = rig12().await;
    r.lm.pull("bge-m3").await.unwrap();
    until("the failure", || r.lm.pull_rows().first().is_some_and(|p| p.error.as_deref().is_some_and(|e| e.contains("does not exist")))).await;
}

#[tokio::test]
async fn without_ollama_nothing_is_pulled_and_the_reason_says_what_to_do() {
    let mut r = rig12().await;
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    std::sync::Arc::get_mut(&mut r.lm).unwrap().base_override = Some(closed);
    let e = r.lm.pull("gemma3:4b").await.unwrap_err();
    assert!(matches!(&e, Refusal::Conflict(m) if m.contains("not reachable") && m.contains("start it")), "{e:?}");
    assert!(r.lm.pull_rows().is_empty() && r.providers.provider("ollama").is_none(), "no provider was added either");
}

#[tokio::test]
async fn a_model_installed_outside_kagent_only_needs_its_row() {
    let r = rig12().await;
    r.ollama.install("gemma3:4b", 3300 * MB);
    r.lm.pull("gemma3:4b").await.unwrap();
    assert!(r.ollama.starts().is_empty(), "nothing is downloaded");
    assert_eq!(model_row(&r, "gemma3:4b")["roles"], json!(["chat"]));
}

#[tokio::test]
async fn remove_is_refused_while_a_setting_names_the_model() {
    let r = rig12().await;
    r.ollama.script("gemma3:4b", happy(&[("aaa", 100 * MB)], 1));
    r.lm.pull("gemma3:4b").await.unwrap();
    until("installed", || r.providers.provider("ollama").is_some_and(|p| p.models.iter().any(|m| m.id == "gemma3:4b"))).await;
    until("quiet", || r.lm.pull_rows().is_empty()).await;
    r.settings.write().unwrap().extra.insert("visionModel".into(), json!("ollama::gemma3:4b"));
    let e = r.lm.remove("gemma3:4b").await.unwrap_err();
    let Refusal::Conflict(m) = &e else { panic!("{e:?}") };
    assert!(m.contains("visionModel") && m.contains("choose another model"), "{m}");
    assert_eq!(http_error(e).0, StatusCode::CONFLICT);
    assert!(r.ollama.has("gemma3:4b") && r.ollama.seen_at("/api/delete").is_empty(), "Ollama was not asked to delete");
    assert!(r.lm.view().await["recommended"].as_array().unwrap().iter().any(|m| m["id"] == "gemma3:4b" && m["usedFor"] == json!(["visionModel"])));
    // Free again: it goes from Ollama and from the providers.
    r.settings.write().unwrap().extra.remove("visionModel");
    r.lm.remove("gemma3:4b").await.unwrap();
    assert!(!r.ollama.has("gemma3:4b"));
    assert!(r.providers.provider("ollama").unwrap().models.iter().all(|m| m.id != "gemma3:4b"));
    assert!(matches!(r.lm.remove("gemma3:4b").await.unwrap_err(), Refusal::Missing(_)));
    assert!(matches!(r.lm.remove("mistral:7b").await.unwrap_err(), Refusal::Invalid(_)));
    assert!(matches!(r.lm.remove("whisper-small").await.unwrap_err(), Refusal::Invalid(_)));
}
