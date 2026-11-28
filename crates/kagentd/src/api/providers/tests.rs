use super::*;
use serde_json::json;

fn store() -> (tempfile::TempDir, ProviderStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap();
    (dir, s)
}

/// The provider `template` added once, and the id of its first model.
fn with_provider(s: &ProviderStore, template: &str) -> String {
    let v = ok(add(s, &json!({"templateId": template})));
    assert_eq!(v["id"], template);
    v["models"][0]["id"].as_str().unwrap().to_string()
}

/// `HttpError` has no `Debug`: these take the value, or the error, of a call.
fn ok<T>(r: Result<T, HttpError>) -> T {
    r.ok().expect("refused")
}

fn refused<T>(r: Result<T, HttpError>) -> HttpError {
    r.err().expect("not refused")
}

fn is(e: &HttpError, status: StatusCode, code: &str, part: &str) {
    assert_eq!((e.0, e.1), (status, code), "{}", e.2);
    assert!(e.2.contains(part), "{}", e.2);
}

#[test]
fn the_custom_template_needs_a_name_and_a_base_url_and_takes_the_api_type() {
    let (_d, s) = store();
    let e = |body: Value| refused(add(&s, &body));
    is(&e(json!({})), StatusCode::BAD_REQUEST, "invalid_input", "templateId required");
    is(&e(json!({"templateId": "nope"})), StatusCode::BAD_REQUEST, "invalid_input", "unknown template nope");
    is(&e(json!({"templateId": "custom", "baseUrl": "https://x/v1"})), StatusCode::BAD_REQUEST, "invalid_input", "a custom provider needs a name");
    is(&e(json!({"templateId": "custom", "name": "Mine"})), StatusCode::BAD_REQUEST, "invalid_input", "a custom provider needs a base URL");
    is(&e(json!({"templateId": "custom", "name": "Mine", "baseUrl": "https://x/v1", "apiType": "soap"})), StatusCode::BAD_REQUEST, "invalid_input", "apiType must be one of anthropic-messages");
    assert!(s.providers().order.is_empty(), "a refused add writes nothing");

    let v = ok(add(&s, &json!({"templateId": "custom", "name": "Mine", "baseUrl": "https://x/v1", "apiType": "anthropic-messages", "apiKey": "sk-x"})));
    assert_eq!((v["id"].as_str(), v["name"].as_str(), v["api"]["type"].as_str()), (Some("custom"), Some("Mine"), Some("anthropic-messages")), "the format is chosen at creation");
    assert_eq!(v["models"].as_array().unwrap().len(), 0, "no models of its own");
    assert_eq!((v["hasKey"].as_bool(), v["apiKeyMasked"].as_str()), (Some(true), Some("****")));
    // The answer is the view of `GET /api/providers`: never the key itself.
    assert!(v.get("apiKey").is_none());

    // NVIDIA is in the catalog: three models, on (the template lists fewer than five).
    let v = ok(add(&s, &json!({"templateId": "nvidia", "apiKey": "nvapi-1234567890abcdef"})));
    assert_eq!((v["id"].as_str(), v["name"].as_str()), (Some("nvidia"), Some("NVIDIA")));
    let models = v["models"].as_array().unwrap();
    assert_eq!(models.len(), 3);
    assert!(models.iter().all(|m| m["enabled"] == true && m["roles"] == json!(["chat"]) && m["smart"] == true));
    assert_eq!(models[0]["effective"]["contextWindow"], 262_144, "the site rule of nvidia, reported as effective");
    assert_eq!(models[2]["effective"]["supportsImage"], true);
    assert_eq!(v["apiKeyMasked"], "nva...cdef");
}

#[test]
fn every_refusal_of_the_patch_is_a_400_a_409_or_a_404() {
    let (_d, s) = store();
    let m = with_provider(&s, "nvidia");
    let mut settings = Settings::defaults();
    settings.fallback_model = format!("nvidia::{m}");
    let put = |body: Value| update(&s, &settings, "nvidia", &body);
    let e = |body: Value| refused(put(body));
    // What the message says, with the model's id in place of `{m}`.
    let named = |what: &str| what.replace("{m}", &m);

    // What the parser refuses.
    is(&e(json!({"bogus": 1})), StatusCode::BAD_REQUEST, "invalid_input", "unknown field `bogus`");
    is(&e(json!({"apiType": "soap"})), StatusCode::BAD_REQUEST, "invalid_input", "unknown variant `soap`, expected one of `anthropic-messages`");
    is(&e(json!({"models": [{"id": m, "config": {"contexttWindow": 1}}]})), StatusCode::BAD_REQUEST, "invalid_input", &named("config.contexttWindow of '{m}' is not a model setting"));
    is(&e(json!({"models": [{"id": m, "config": {"contextWindow": -5}}]})), StatusCode::BAD_REQUEST, "invalid_input", &named("config.contextWindow of '{m}': a whole number above 0"));
    is(&e(json!({"models": [{"id": m, "config": {"maxOutputTokens": 1.5}}]})), StatusCode::BAD_REQUEST, "invalid_input", &named("config.maxOutputTokens of '{m}': a whole number above 0"));
    is(&e(json!({"models": [{"id": m, "config": {"contextWindow": "9"}}]})), StatusCode::BAD_REQUEST, "invalid_input", "config.contextWindow");
    is(&e(json!({"addModels": [{"id": m, "enabled": true}]})), StatusCode::BAD_REQUEST, "invalid_input", "unknown field `enabled`");
    assert!(put(json!({"models": [{"id": m, "config": {"contextWindow": null}}]})).is_ok(), "null is a field left out");

    // What the store refuses, by its code.
    is(&e(json!({"models": [{"id": "nope"}]})), StatusCode::BAD_REQUEST, "invalid_input", "models: unknown model 'nope'");
    is(&e(json!({"models": [{"id": m, "roles": ["fast"]}]})), StatusCode::BAD_REQUEST, "invalid_input", "role 'fast' is not one of chat, live");
    is(&e(json!({"models": [{"id": m, "smart": false}]})), StatusCode::BAD_REQUEST, "invalid_input", &named("config.contextWindow of '{m}'"));
    is(&e(json!({"addModels": [{"id": "x y"}]})), StatusCode::BAD_REQUEST, "invalid_input", "addModels: an id is 1 to 200 characters");
    is(&e(json!({"modelOrder": []})), StatusCode::BAD_REQUEST, "invalid_input", "modelOrder: name every model exactly once");
    // Scenario S40 item 4: the fallback model is not removed, and the message says the role.
    is(&e(json!({"removeModels": [m]})), StatusCode::CONFLICT, "conflict", &named("removeModels: 'nvidia::{m}' is the fallbackModel"));
    is(&e(json!({"removeModels": ["nope"]})), StatusCode::BAD_REQUEST, "invalid_input", "removeModels: unknown model 'nope'");
    is(&refused(update(&s, &settings, "nope", &json!({}))), StatusCode::NOT_FOUND, "not_found", "unknown provider");

    // A valid patch answers the view of the provider.
    let v = ok(put(json!({"name": "Renamed", "models": [{"id": m, "enabled": false, "config": {"contextWindow": 11111}}]})));
    assert_eq!(v["name"], "Renamed");
    assert_eq!(v["models"][0]["enabled"], false);
    assert_eq!(v["models"][0]["effective"]["contextWindow"], 11111, "the patch is applied");
    assert!(v["models"][0]["inherited"]["contextWindow"].is_u64(), "and the view still reports what the rules give");
    assert!(v.get("apiKey").is_none());
    // What came before a refused patch is still there.
    is(&e(json!({"name": "Gone", "models": [{"id": "nope"}]})), StatusCode::BAD_REQUEST, "invalid_input", "unknown model 'nope'");
    assert_eq!(s.provider("nvidia").unwrap().name, "Renamed");
}

#[test]
fn chat_model_valid_only_when_the_model_can_be_used() {
    let (_d, s) = store();
    let m = with_provider(&s, "nvidia");
    let r = format!("nvidia::{m}");
    assert!(chat_model_valid(&s, &r));
    s.update("nvidia", |p| p.enabled = false).unwrap();
    assert!(!chat_model_valid(&s, &r), "provider switched off");
    s.update("nvidia", |p| p.enabled = true).unwrap();
    s.update("nvidia", |p| p.models[0].enabled = false).unwrap();
    assert!(!chat_model_valid(&s, &r), "model switched off");
    assert!(!chat_model_valid(&s, ""), "nothing set");
    assert!(!chat_model_valid(&s, "nvidia::nope"));
}

#[test]
fn a_model_without_the_chat_role_cannot_take_a_text_job() {
    let (_d, s) = store();
    let m = with_provider(&s, "nvidia");
    let r = format!("nvidia::{m}");
    let roles = |roles: Value| serde_json::from_value::<ProviderPatch>(json!({"models": [{"id": m, "roles": roles}]})).unwrap();
    s.patch("nvidia", &roles(json!([])), &[]).unwrap();

    for key in ["chatModel", "fastModel", "fallbackModel", "visionModel"] {
        is(&refused(check_model_roles(&s, &json!({key: r}))), StatusCode::BAD_REQUEST, "invalid_input", "is not a chat model (its roles: no role)");
    }
    s.patch("nvidia", &roles(json!(["live"])), &[]).unwrap();
    is(&refused(check_model_roles(&s, &json!({"fallbackModel": r}))), StatusCode::BAD_REQUEST, "invalid_input", "is not a chat model (its roles: live)");

    // With the chat role every text job is open to it.
    s.patch("nvidia", &roles(json!(["chat", "live"])), &[]).unwrap();
    for key in ["chatModel", "fastModel", "fallbackModel"] {
        ok(check_model_roles(&s, &json!({key: r})));
    }
    // v0.0.3 A4: a vision model must also accept images (its resolved config).
    let image = |on: bool| serde_json::from_value::<ProviderPatch>(json!({"models": [{"id": m, "config": {"supportsImage": on}}]})).unwrap();
    s.patch("nvidia", &image(false), &[]).unwrap();
    is(&refused(check_model_roles(&s, &json!({"visionModel": r}))), StatusCode::BAD_REQUEST, "invalid_input", "Accepts images");
    ok(check_model_roles(&s, &json!({"chatModel": r})));
    s.patch("nvidia", &image(true), &[]).unwrap();
    ok(check_model_roles(&s, &json!({"visionModel": r})));
    ok(check_model_roles(&s, &json!({"visionModel": ""})));
    // A Gemini live model: the live job, never a text one (the check of before).
    with_provider(&s, "gemini");
    let live_ref = "gemini::gemini-3.8-live";
    is(&refused(check_model_roles(&s, &json!({"chatModel": live_ref}))), StatusCode::BAD_REQUEST, "invalid_input", "is a live voice model and cannot answer text chat");
    ok(check_model_roles(&s, &json!({"liveModel": live_ref})));
    is(&refused(check_model_roles(&s, &json!({"liveModel": "gemini::gemini-3.8-flash"}))), StatusCode::BAD_REQUEST, "invalid_input", "is not a live voice model");
    // Empty values and a reference the providers do not know are not refused here.
    ok(check_model_roles(&s, &json!({"chatModel": "", "fastModel": "nope::x", "liveModel": ""})));
    ok(check_model_roles(&s, &json!({"maxIterations": 5})));
}

#[test]
fn the_model_lists_follow_the_roles() {
    let (_d, s) = store();
    with_provider(&s, "nvidia");
    let lists = model_lists(&s);
    for key in ["chat", "fast", "vision"] {
        assert_eq!(lists[key].as_array().unwrap().len(), 3, "{key}: every template model is a chat model");
    }
    assert!(lists["live"].as_array().unwrap().is_empty());
    assert_eq!(lists["chat"][0]["ref"], format!("nvidia::{}", lists["chat"][0]["id"].as_str().unwrap()));
    assert!(lists["chat"][0]["config"].get("contextWindow").is_some(), "the resolved config, as before");
    assert_eq!((lists["chat"][0]["providerId"].as_str(), lists["chat"][0]["providerName"].as_str()), (Some("nvidia"), Some("NVIDIA")), "the picker groups by provider (picker.md section 3)");

    with_provider(&s, "gemini");
    let lists = model_lists(&s);
    let live: Vec<&str> = lists["live"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(live, ["gemini-3.8-live", "gemini-3.8-live-extended-thinking"]);
    assert_eq!(lists["chat"].as_array().unwrap().len(), 5, "three of NVIDIA and the two Gemini chat models");

    // Switched off, without a job, or in a provider that is off: in no list.
    let second = lists["chat"][1]["id"].as_str().unwrap().to_string();
    let change = serde_json::from_value(json!({"models": [{"id": lists["chat"][0]["id"].as_str().unwrap(), "enabled": false}, {"id": second, "roles": []}]})).unwrap();
    s.patch("nvidia", &change, &[]).unwrap();
    assert_eq!(model_lists(&s)["chat"].as_array().unwrap().len(), 3, "one switched off, one without a job");
    s.update("nvidia", |p| p.enabled = false).unwrap();
    assert_eq!(model_lists(&s)["chat"].as_array().unwrap().len(), 2);
    assert!(model_lists(&s)["vision"].as_array().unwrap().iter().all(|m| m["ref"].as_str().unwrap().starts_with("gemini::")));
}

#[test]
fn the_named_models_are_the_settings_that_name_one() {
    let mut s = Settings::defaults();
    assert!(named_models(&s).is_empty());
    s.chat_model = "a::x".into();
    s.fallback_model = "b::y".into();
    s.live_model = "c::z".into();
    s.patch(json!({"visionModel": "d::w"})).unwrap();
    let named = named_models(&s);
    assert_eq!(named, [("chatModel", "a::x".to_string()), ("fallbackModel", "b::y".to_string()), ("visionModel", "d::w".to_string()), ("liveModel", "c::z".to_string())]);
    assert_eq!(as_named(&named), [("chatModel", "a::x"), ("fallbackModel", "b::y"), ("visionModel", "d::w"), ("liveModel", "c::z")]);
}

#[test]
fn deleting_a_provider_empties_every_setting_that_names_it() {
    let mut s = Settings::defaults();
    s.chat_model = "gemini::a".into();
    s.fast_model = "gemini::b".into();
    s.fallback_model = "ollama::c".into();
    s.live_model = "gemini::models/x-live".into();
    s.patch(json!({"visionModel": "gemini::d"})).unwrap();
    assert!(clear_provider_refs(&mut s, "gemini"));
    assert_eq!(named_models(&s), [("fallbackModel", "ollama::c".to_string())], "only the other provider's model is left");
    assert!(!clear_provider_refs(&mut s, "gemini"), "nothing left to clear");
    // A provider whose id is the start of another's is not the other one.
    s.fallback_model = "ollama2::c".into();
    assert!(!clear_provider_refs(&mut s, "ollama"));
}
