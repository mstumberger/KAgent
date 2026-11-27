use super::*;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{AgentConfig, AgentDeps};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::AgentEvent;
use kagent_core::tool::ToolRegistry;

struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        anyhow::bail!("no model in this test")
    }
}

struct Rig {
    agent: Arc<Agent>,
    providers: ProviderStore,
    events: broadcast::Sender<AgentEvent>,
    id: String,
    /// The refs of the three models: with efforts, without, and switched off.
    ids: [String; 3],
    _dir: tempfile::TempDir,
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let meta = store.create("t").await.unwrap();
    let (events, _) = broadcast::channel(16);
    let cfg = AgentConfig { model_ref: String::new(), system_prompt: String::new(), max_iterations: 4, options: Default::default(), privilege: Default::default(), context_tokens: 0, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(NoModel),
        tools: Arc::new(ToolRegistry::new()),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
        store,
        events: events.clone(),
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    let providers = ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap();
    let entry = providers.add_from_template("nvidia", None, None, None, Default::default(), None).unwrap();
    let ids: Vec<String> = entry.models.iter().map(|m| format!("nvidia::{}", m.id)).collect();
    providers
        .update("nvidia", |p| {
            p.models[0].config.reasoning_levels = Some(vec!["low".into(), "medium".into(), "high".into()]);
            p.models[1].config.reasoning_levels = Some(vec![]);
            p.models[2].enabled = false;
        })
        .unwrap();
    Rig { agent, providers, events, id: meta.id, ids: [ids[0].clone(), ids[1].clone(), ids[2].clone()], _dir: dir }
}

type Refused = (StatusCode, &'static str, String);

fn refusal<T>(r: Result<T, HttpError>) -> Refused {
    match r {
        Ok(_) => panic!("not refused"),
        Err(HttpError(status, code, message)) => (status, code, message),
    }
}

async fn put(r: &Rig, body: Value) -> Result<SessionMeta, HttpError> {
    set(&r.agent, &r.providers, "", &r.id, &body).await
}

async fn stored(r: &Rig) -> (Option<String>, Option<String>) {
    let m = r.agent.store().meta(&r.id).await.unwrap();
    (m.model, m.reasoning_level)
}

#[tokio::test]
async fn every_refusal_has_its_status_code_and_message_and_changes_nothing() {
    let r = rig().await;
    let [big, plain, off] = r.ids.clone();
    let bad = |m: &str| (StatusCode::BAD_REQUEST, "invalid_input", m.to_string());
    let shape = "expected an object with model and reasoningLevel (strings, or null to clear)";
    for body in [json!([1]), json!("x"), json!({"model": 5}), json!({"reasoningLevel": true}), json!({"title": "x"}), json!({"models": "a"})] {
        assert_eq!(refusal(put(&r, body.clone()).await), bad(shape), "{body}");
    }
    assert_eq!(refusal(put(&r, json!({"model": "nope::x"})).await), bad("model 'nope::x' cannot be used: unknown model"));
    assert_eq!(refusal(put(&r, json!({"model": "garbage"})).await), bad("model 'garbage' cannot be used: unknown model"));
    assert_eq!(refusal(put(&r, json!({"model": off})).await), bad(&format!("model '{off}' cannot be used: model switched off")));
    r.providers.update("nvidia", |p| p.models[1].roles = vec!["live".into()]).unwrap();
    assert_eq!(refusal(put(&r, json!({"model": plain})).await), bad(&format!("model '{plain}' cannot be used: not a chat model")), "a live model");
    r.providers.update("nvidia", |p| p.models[1].roles = vec!["chat".into()]).unwrap();
    r.providers.update("nvidia", |p| p.enabled = false).unwrap();
    assert_eq!(refusal(put(&r, json!({"model": big})).await), bad(&format!("model '{big}' cannot be used: provider switched off")));
    r.providers.update("nvidia", |p| p.enabled = true).unwrap();

    assert_eq!(refusal(put(&r, json!({"model": big, "reasoningLevel": "max"})).await), bad(&format!("model '{big}' has no effort 'max' (it offers: low, medium, high)")));
    assert_eq!(refusal(put(&r, json!({"model": plain, "reasoningLevel": "low"})).await), bad(&format!("model '{plain}' has no effort 'low' (it offers none)")));
    assert_eq!(refusal(put(&r, json!({"reasoningLevel": "low"})).await), bad("effort 'low' needs a model: pick a model first"), "no model of the session and no default");
    assert_eq!(stored(&r).await, (None, None), "every refusal left the session as it was");

    // A level is asked of the model that answers: the session's, else the default.
    put(&r, json!({"model": plain})).await.ok().unwrap();
    assert_eq!(refusal(put(&r, json!({"reasoningLevel": "low"})).await), bad(&format!("model '{plain}' has no effort 'low' (it offers none)")));
    put(&r, json!({"model": null})).await.ok().unwrap();
    let with_default = set(&r.agent, &r.providers, &big, &r.id, &json!({"reasoningLevel": "low"})).await.ok().unwrap();
    assert_eq!(with_default.reasoning_level.as_deref(), Some("low"), "the default model answers");
    let e = refusal(set(&r.agent, &r.providers, &plain, &r.id, &json!({"reasoningLevel": "high"})).await);
    assert_eq!(e.2, format!("model '{plain}' has no effort 'high' (it offers none)"));

    let e = refusal(set(&r.agent, &r.providers, "", "no-such", &json!({"model": big})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::NOT_FOUND, "not_found", "session no-such not found"));
    let child = r.agent.store().create_with("job", SessionOrigin::Subagent, Some(r.id.clone())).await.unwrap();
    let e = refusal(set(&r.agent, &r.providers, "", &child.id, &json!({"model": big})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::CONFLICT, "conflict", "the session of a subagent takes the model of its job: it cannot be changed"));
    assert_eq!(r.agent.store().meta(&child.id).await.unwrap().model, None);
}

#[tokio::test]
async fn a_key_left_out_keeps_its_value_and_null_or_empty_clears_it() {
    let r = rig().await;
    let [big, plain, _] = r.ids.clone();
    let m = put(&r, json!({"model": big, "reasoningLevel": "high"})).await.ok().unwrap();
    assert_eq!((m.model.as_deref(), m.reasoning_level.as_deref()), (Some(big.as_str()), Some("high")));
    assert_eq!(put(&r, json!({"reasoningLevel": "low"})).await.ok().unwrap().model.as_deref(), Some(big.as_str()), "the model is kept");
    assert_eq!(put(&r, json!({})).await.ok().unwrap().reasoning_level.as_deref(), Some("low"), "an empty body keeps both");
    let m = put(&r, json!({"reasoningLevel": null})).await.ok().unwrap();
    assert_eq!((m.model.as_deref(), m.reasoning_level), (Some(big.as_str()), None), "null clears the level, the model stays");
    put(&r, json!({"reasoningLevel": "high"})).await.ok().unwrap();
    let m = put(&r, json!({"reasoningLevel": ""})).await.ok().unwrap();
    assert_eq!(m.reasoning_level, None, "an empty string clears too");
    // The level goes with the model it was for, unless the new model offers it.
    put(&r, json!({"reasoningLevel": "medium"})).await.ok().unwrap();
    let m = put(&r, json!({"model": plain})).await.ok().unwrap();
    assert_eq!((m.model.as_deref(), m.reasoning_level), (Some(plain.as_str()), None), "a model without efforts clears the level");
    put(&r, json!({"model": big, "reasoningLevel": "medium"})).await.ok().unwrap();
    let m = put(&r, json!({"model": big})).await.ok().unwrap();
    assert_eq!(m.reasoning_level.as_deref(), Some("medium"), "the same model keeps it");
    let m = put(&r, json!({"model": "", "reasoningLevel": null})).await.ok().unwrap();
    assert_eq!((m.model, m.reasoning_level), (None, None), "Default clears both");
    assert_eq!(stored(&r).await, (None, None));
}

#[tokio::test]
async fn the_event_carries_the_stored_meta_and_the_same_meta_twice_sends_nothing() {
    let r = rig().await;
    let big = r.ids[0].clone();
    let mut rx = r.events.subscribe();
    let m = put(&r, json!({"model": big, "reasoningLevel": "low"})).await.ok().unwrap();
    let AgentEvent::SessionMetaChanged { session_id, meta } = rx.try_recv().unwrap() else { panic!("sessionMetaChanged") };
    assert_eq!(session_id, r.id);
    assert_eq!(meta, r.agent.store().meta(&r.id).await.unwrap(), "the whole meta, as the file has it");
    assert_eq!(meta, m);
    let json = serde_json::to_value(AgentEvent::SessionMetaChanged { session_id: "s".into(), meta }).unwrap();
    assert_eq!((json["type"].as_str(), json["sessionId"].as_str(), json["meta"]["model"].as_str(), json["meta"]["reasoningLevel"].as_str()), (Some("sessionMetaChanged"), Some("s"), Some(big.as_str()), Some("low")));
    put(&r, json!({"model": big, "reasoningLevel": "low"})).await.ok().unwrap();
    put(&r, json!({})).await.ok().unwrap();
    assert!(rx.try_recv().is_err(), "the same meta twice: no second event");
    put(&r, json!({"reasoningLevel": null})).await.ok().unwrap();
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::SessionMetaChanged { .. })), "a change goes out");
}

#[tokio::test]
async fn a_session_is_created_with_a_choice_and_a_refused_one_makes_none() {
    let r = rig().await;
    let [big, plain, _] = r.ids.clone();
    let sessions = r.agent.store();
    let m = create(sessions, &r.providers, "", &json!({"title": "Hello", "model": big, "reasoningLevel": "high"})).await.ok().unwrap();
    assert_eq!((m.title.as_str(), m.model.as_deref(), m.reasoning_level.as_deref()), ("Hello", Some(big.as_str()), Some("high")));
    assert_eq!(sessions.meta(&m.id).await.unwrap(), m, "saved with both");
    let plain_one = create(sessions, &r.providers, "", &json!({})).await.ok().unwrap();
    assert_eq!((plain_one.title.as_str(), plain_one.model, plain_one.reasoning_level), ("", None, None));
    let before = sessions.list().await.unwrap().len();
    for (body, part) in [
        (json!({"model": "nope::x"}), "cannot be used: unknown model"),
        (json!({"model": plain, "reasoningLevel": "high"}), "(it offers none)"),
        (json!({"reasoningLevel": "high"}), "pick a model first"),
        (json!({"model": big, "extra": 1}), "expected an object"),
        (json!({"title": 5}), "expected an object"),
    ] {
        let e = refusal(create(sessions, &r.providers, "", &body).await);
        assert!(e.0 == StatusCode::BAD_REQUEST && e.2.contains(part), "{body}: {e:?}");
    }
    assert_eq!(sessions.list().await.unwrap().len(), before, "a refused create leaves no session and no file");
    let default = create(sessions, &r.providers, &big, &json!({"reasoningLevel": "low"})).await.ok().unwrap();
    assert_eq!((default.model, default.reasoning_level.as_deref()), (None, Some("low")), "a level for the default model");
}

#[test]
fn choice_of_reads_the_three_shapes_of_a_value() {
    assert_eq!(choice_of(&json!({})), Ok(Choice::default()));
    assert_eq!(choice_of(&json!({"model": "a::b", "reasoningLevel": null})), Ok(Choice { model: Some(Some("a::b".into())), level: Some(None) }));
    assert_eq!(choice_of(&json!({"model": ""})), Ok(Choice { model: Some(None), level: None }));
    assert!(choice_of(&json!({"title": "x"})).is_err(), "the title is for POST only");
}
