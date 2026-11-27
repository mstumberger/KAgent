use super::*;
use serde_json::json;

/// A small catalog of known values: the template `t` with the models `a` and `b`, and
/// two rules for a model called `big`.
fn small_catalog() -> BuiltinCatalog {
    serde_json::from_value(json!({
        "templates": [{"id": "t", "name": "T", "api": {"type": "openai-chat-completions", "baseUrl": "https://t.example/v1", "headers": {}}, "models": ["a", "b"]}],
        "modelRules": [
            {"match": {"model": "big"}, "config": {"contextWindow": 1000000, "reasoningLevels": ["low", "high"], "requestPatch": {"x": 1}}},
            {"match": {"model": "big", "apiType": "openai-chat-completions", "baseUrl": "https://t\\.example/v1/?"}, "config": {"maxOutputTokens": 64000}}
        ]
    }))
    .unwrap()
}

/// An empty store over the builtin catalog.
fn store(dir: &tempfile::TempDir) -> ProviderStore {
    ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap()
}

/// An empty store over `small_catalog`.
fn small(dir: &tempfile::TempDir) -> ProviderStore {
    ProviderStore::load(dir.path().join("providers.json"), small_catalog(), &[]).unwrap()
}

fn row(id: &str) -> ModelEntry {
    ModelEntry { id: id.into(), name: String::new(), enabled: true, roles: vec!["chat".into()], smart: true, config: ModelConfigPatch::default() }
}

fn patch(body: &str) -> ProviderPatch {
    serde_json::from_str(body).unwrap()
}

fn ids(p: &ProviderEntry) -> Vec<&str> {
    p.models.iter().map(|m| m.id.as_str()).collect()
}

#[test]
fn keys_go_to_the_secret_sink_not_the_file() {
    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<(String, String)>>);
    impl SecretSink for Sink {
        fn put(&self, id: &str, key: &str) {
            self.0.lock().unwrap().push((id.into(), key.into()));
        }
        fn delete(&self, id: &str) {
            self.0.lock().unwrap().push((id.into(), String::new()));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.json");
    let store = ProviderStore::load(&path, ProviderStore::builtin_catalog().unwrap(), &[]).unwrap();
    let p = store.add_from_template("anthropic", None, None, Some("sk-ant-secret-123456".into()), Default::default(), None).unwrap();
    let sink = std::sync::Arc::new(Sink::default());
    store.set_secret_sink(sink.clone());
    store.save().unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(!disk.contains("sk-ant-secret"), "{disk}");
    assert!(disk.contains("\"apiKeyRef\": \"secret-service\""));
    assert_eq!(sink.0.lock().unwrap()[0], (p.id.clone(), "sk-ant-secret-123456".into()));
    // Memory still has the key for requests.
    assert_eq!(store.provider(&p.id).unwrap().api_key, "sk-ant-secret-123456");
}

#[test]
fn add_from_template_twice_and_mask() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir);
    let p1 = s.add_from_template("anthropic", None, None, Some("sk-ant-abcdefghijkl1234".into()), Default::default(), None).unwrap();
    let p2 = s.add_from_template("anthropic", None, None, None, Default::default(), None).unwrap();
    assert_eq!(p1.id, "anthropic");
    assert_eq!(p2.id, "anthropic-2");
    assert_eq!(s.providers().order, vec!["anthropic", "anthropic-2"]);

    let masked = s.masked();
    assert_eq!(masked[0]["apiKeyMasked"], "sk-...1234");
    assert_eq!(masked[0]["hasKey"], true);
    assert_eq!(masked[1]["apiKeyMasked"], "****");
    // Every row says whether it is smart, what it inherits and what is sent.
    assert_eq!(masked[0]["models"][0]["smart"], true);
    for key in ["contextWindow", "maxOutputTokens", "supportsToolCall", "supportsImage", "reasoningLevels", "reasoningPatches", "supportsLive", "liveProtocol", "requestPatch", "roles", "enabled"] {
        assert!(masked[0]["models"][0]["inherited"].get(key).is_some(), "inherited has {key}");
        assert!(masked[0]["models"][0]["effective"].get(key).is_some(), "effective has {key}");
    }
    assert!(masked[0].get("apiKey").is_none(), "the view never carries the key");

    s.delete("anthropic-2").unwrap();
    assert_eq!(s.providers().order, vec!["anthropic"]);
}

#[test]
fn file_is_0600_and_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.json");
    {
        let s = store(&dir);
        s.add_from_template("ollama", None, None, None, Default::default(), None).unwrap();
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let catalog = serde_json::from_str(include_str!("../../../../config/providers.builtin.json")).unwrap();
    let s2 = ProviderStore::load(&path, catalog, &[]).unwrap();
    assert_eq!(s2.provider("ollama").unwrap().api.base_url, "http://127.0.0.1:11434");
}

#[test]
fn resolves_model_ref() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir);
    s.add_from_template("ollama", None, None, None, Default::default(), None).unwrap();
    s.update("ollama", |p| p.models.push(row("qwen2.5:7b-instruct"))).unwrap();
    let r = s.resolve("ollama::qwen2.5:7b-instruct").unwrap();
    assert_eq!(r.provider.id, "ollama");
    assert!(r.config.supports_tool_call);
    assert!(s.resolve("nope::x").is_err());
    assert!(split_ref("bad").is_none());
}

#[test]
fn a_model_is_usable_only_when_it_has_the_chat_role_and_is_on_in_a_provider_that_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let s = small(&dir);
    s.add_from_template("t", None, None, None, Default::default(), None).unwrap();
    let r = "t::a";
    assert!(s.usable(r).is_some());
    s.update("t", |p| p.enabled = false).unwrap();
    assert!(s.usable(r).is_none(), "provider switched off");
    s.update("t", |p| p.enabled = true).unwrap();
    s.update("t", |p| p.models[0].enabled = false).unwrap();
    assert!(s.usable(r).is_none(), "model switched off");
    s.update("t", |p| p.models[0].enabled = true).unwrap();
    s.patch("t", &patch(r#"{"models":[{"id":"a","roles":["live"]}]}"#), &[]).unwrap();
    assert!(s.usable(r).is_none(), "a live model answers no text turn");
    s.patch("t", &patch(r#"{"models":[{"id":"a","roles":["chat","live"]}]}"#), &[]).unwrap();
    assert!(s.usable(r).is_some(), "both jobs");
    assert!(s.usable("t::zzz").is_none() && s.usable("nope::a").is_none() && s.usable("bad").is_none(), "unknown");
}

#[test]
fn unusable_names_the_reason_and_agrees_with_usable() {
    let dir = tempfile::tempdir().unwrap();
    let s = small(&dir);
    s.add_from_template("t", None, None, None, Default::default(), None).unwrap();
    let r = "t::a";
    let agrees = |s: &ProviderStore| {
        for m in [r, "t::zzz", "nope::a", "bad"] {
            assert_eq!(s.usable(m).is_some(), s.unusable(m).is_none(), "{m}");
        }
    };
    assert_eq!(s.unusable(r), None);
    agrees(&s);
    s.update("t", |p| p.enabled = false).unwrap();
    assert_eq!(s.unusable(r), Some("provider switched off"));
    agrees(&s);
    s.update("t", |p| p.enabled = true).unwrap();
    s.update("t", |p| p.models[0].enabled = false).unwrap();
    assert_eq!(s.unusable(r), Some("model switched off"));
    agrees(&s);
    s.update("t", |p| p.models[0].enabled = true).unwrap();
    s.patch("t", &patch(r#"{"models":[{"id":"a","roles":["live"]}]}"#), &[]).unwrap();
    assert_eq!(s.unusable(r), Some("not a chat model"), "a live model");
    agrees(&s);
    assert_eq!((s.unusable("t::zzz"), s.unusable("nope::a"), s.unusable("bad")), (Some("unknown model"), Some("unknown model"), Some("unknown model")));
}

#[test]
fn inherited_is_what_the_rules_give_and_effective_what_is_sent() {
    let dir = tempfile::tempdir().unwrap();
    let s = small(&dir);
    s.add_from_template("t", None, None, None, Default::default(), None).unwrap();
    s.update("t", |p| {
        p.models[0] = ModelEntry { id: "big".into(), config: ModelConfigPatch { context_window: Some(123), ..Default::default() }, ..row("big") };
    })
    .unwrap();
    let p = s.provider("t").unwrap();
    let (inherited, effective) = s.configs(&p, &p.models[0]);
    assert_eq!((inherited.context_window, inherited.max_output_tokens), (1_000_000, 64_000), "the rules, class by class");
    assert_eq!(inherited.reasoning_levels, vec!["low".to_string(), "high".to_string()]);
    assert_eq!(inherited.request_patch, json!({"x": 1}));
    assert_eq!((effective.context_window, effective.max_output_tokens), (123, 64_000), "the smart patch over the rules");

    // Manual: the six editable fields come from the patch, the request patch from the rule.
    let manual = ModelConfigPatch {
        context_window: Some(77_000),
        max_output_tokens: Some(111),
        supports_tool_call: Some(false),
        supports_image: Some(true),
        reasoning_levels: Some(vec!["a".into()]),
        ..Default::default()
    };
    let m = ModelEntry { smart: false, config: manual, ..row("big") };
    let (inherited, effective) = s.configs(&p, &m);
    assert_eq!(inherited.context_window, 1_000_000, "inherited does not depend on the row");
    assert_eq!((effective.context_window, effective.max_output_tokens, effective.supports_tool_call, effective.supports_image), (77_000, 111, false, true));
    assert_eq!(effective.reasoning_levels, vec!["a".to_string()]);
    assert_eq!(effective.request_patch, json!({"x": 1}), "kept from the rules");
    // What the API reports is the same pair.
    let view = &s.masked()[0]["models"][0];
    assert_eq!(view["inherited"]["contextWindow"], 1_000_000);
    assert_eq!(view["effective"]["contextWindow"], 123);
}

#[test]
fn load_corrects_the_rows_of_an_old_fetch_and_leaves_the_named_model_alone() {
    // Scenario S40 item 3, through the store: a load writes nothing.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.json");
    let old = json!({"schemaVersion": 1, "order": ["gemini"], "providers": {"gemini": {
        "id": "gemini", "name": "Gemini", "templateId": "gemini", "enabled": true, "api": {"type": "openai-chat-completions", "baseUrl": "https://x/v1", "headers": {}},
        "models": [
            {"id": "models/gemini-3.8-flash-tts", "name": "", "enabled": false, "roles": ["chat"], "config": {}},
            {"id": "models/lyria-3.5", "name": "", "enabled": false, "roles": ["chat"], "config": {}},
            {"id": "models/gemini-3.8-live", "name": "", "enabled": false, "roles": ["chat"], "config": {}},
            {"id": "models/imagen-4-generate", "name": "", "enabled": false, "roles": ["chat"], "config": {}},
            {"id": "gemini-3.8-pro", "name": "", "enabled": true, "roles": ["chat"], "config": {}}
        ]}}});
    std::fs::write(&path, old.to_string()).unwrap();
    let named = ["gemini::models/imagen-4-generate".to_string()];
    let s = ProviderStore::load(&path, small_catalog(), &named).unwrap();
    let p = s.provider("gemini").unwrap();
    let roles: Vec<&str> = p.models.iter().map(|m| m.roles.first().map(String::as_str).unwrap_or("-")).collect();
    assert_eq!(roles, ["-", "-", "live", "chat", "chat"], "text-to-speech and music have no job, live is live, the image model named in settings and the chat model stay");
    assert!(p.models.iter().enumerate().all(|(i, m)| m.enabled == (i == 4)), "nothing else moves");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), old.to_string(), "a load writes nothing");
    assert!(p.models.iter().all(|m| m.smart), "an old file is smart");
    // The next save writes the correction; a second load changes nothing.
    s.update("gemini", |_| {}).unwrap();
    let again = ProviderStore::load(&path, small_catalog(), &named).unwrap();
    assert_eq!(again.provider("gemini").unwrap().models, s.provider("gemini").unwrap().models);
    assert!(std::fs::read_to_string(&path).unwrap().contains(r#""schemaVersion": 2"#), "saved as version 2");
}

#[test]
fn a_role_the_owner_gives_after_the_correction_survives_a_restart() {
    // The correction runs once (schemaVersion 1 to 2), not at every start: the owner
    // gives a text-to-speech row the chat role, and the next start keeps it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.json");
    let old = json!({"schemaVersion": 1, "order": ["gemini"], "providers": {"gemini": {
        "id": "gemini", "name": "Gemini", "templateId": "gemini", "enabled": true, "api": {"type": "openai-chat-completions", "baseUrl": "https://x/v1", "headers": {}},
        "models": [{"id": "models/gemini-3.8-flash-tts", "name": "", "enabled": false, "roles": ["chat"], "config": {}}]}}});
    std::fs::write(&path, old.to_string()).unwrap();
    let s = ProviderStore::load(&path, small_catalog(), &[]).unwrap();
    assert!(s.provider("gemini").unwrap().models[0].roles.is_empty(), "corrected at the first start");
    let change: ProviderPatch = serde_json::from_value(json!({"models": [{"id": "models/gemini-3.8-flash-tts", "roles": ["chat"]}]})).unwrap();
    s.patch("gemini", &change, &[]).unwrap();
    let again = ProviderStore::load(&path, small_catalog(), &[]).unwrap();
    assert_eq!(again.provider("gemini").unwrap().models[0].roles, ["chat"], "the owner's role stays");
}

#[test]
fn the_custom_template_needs_a_name_and_a_base_url() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir);
    let err = |r: anyhow::Result<ProviderEntry>| r.unwrap_err().to_string();
    assert_eq!(err(s.add_from_template("nope", None, None, None, Default::default(), None)), "unknown template nope");
    assert_eq!(err(s.add_from_template("custom", None, Some("https://x/v1".into()), None, Default::default(), None)), "a custom provider needs a name");
    assert_eq!(err(s.add_from_template("custom", Some("  ".into()), Some("https://x/v1".into()), None, Default::default(), None)), "a custom provider needs a name");
    assert_eq!(err(s.add_from_template("custom", Some("Mine".into()), None, None, Default::default(), None)), "a custom provider needs a base URL");
    assert!(s.providers().order.is_empty(), "a refused add writes nothing");

    // The builtin catalog holds `custom` (OpenAI-compatible, no address, no models) and `nvidia`.
    let real = ProviderStore::load(dir.path().join("real.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap();
    let c = real.add_from_template("custom", Some("Mine".into()), Some("https://x/v1".into()), None, Default::default(), None).unwrap();
    assert_eq!((c.id.as_str(), c.name.as_str(), c.api.base_url.as_str()), ("custom", "Mine", "https://x/v1"));
    assert_eq!(c.api.api_type, ApiType::OpenaiChatCompletions);
    assert!(c.models.is_empty());
    let c2 = real.add_from_template("custom", Some("Other".into()), Some("https://y/v1".into()), None, Default::default(), Some(ApiType::AnthropicMessages)).unwrap();
    assert_eq!((c2.id.as_str(), c2.api.api_type), ("custom-2", ApiType::AnthropicMessages), "the format is chosen at creation");
    let n = real.add_from_template("nvidia", None, None, None, Default::default(), None).unwrap();
    assert_eq!(n.models.len(), 3);
    assert_eq!(real.configs(&n, &n.models[0]).1.context_window, 262_144, "the site rule of nvidia");
    assert!(real.configs(&n, &n.models[2]).1.supports_image);
}

#[test]
fn a_patch_adds_changes_removes_and_reorders_models() {
    let dir = tempfile::tempdir().unwrap();
    let s = small(&dir);
    s.add_from_template("t", Some("T".into()), None, Some("k".into()), Default::default(), None).unwrap();
    assert_eq!(s.provider("t").unwrap().models[0].enabled, true, "a template's few models are on, as before");

    let out = s
        .patch(
            "t",
            &patch(
                r#"{"name": "Renamed", "baseUrl": "https://new.example/v1", "apiKey": "", "headers": {"x": "1"}, "enabled": true,
                    "apiType": "openai-responses",
                    "addModels": [{"id": "c"}, {"id": "models/d-live"}],
                    "models": [{"id": "a", "enabled": false}, {"id": "c", "smart": false, "config": {"contextWindow": 9, "maxOutputTokens": 5, "reasoningLevels": [], "supportsToolCall": true, "supportsImage": false}}],
                    "removeModels": ["b"],
                    "modelOrder": ["c", "a", "models/d-live"]}"#,
            ),
            &[],
        )
        .unwrap();
    assert_eq!((out.name.as_str(), out.api.base_url.as_str(), out.api.api_type), ("Renamed", "https://new.example/v1", ApiType::OpenaiResponses));
    assert_eq!(out.api_key, "k", "an empty key keeps the stored one");
    assert_eq!(out.api.headers.get("x").unwrap(), "1");
    assert_eq!(ids(&out), ["c", "a", "models/d-live"]);
    let c = &out.models[0];
    assert!(!c.enabled && !c.smart, "a new row arrives disabled; the change made it manual");
    assert_eq!((c.config.context_window, c.config.max_output_tokens), (Some(9), Some(5)));
    assert!(!out.models[1].enabled, "the change of `a` was applied");
    assert_eq!(out.models[2].roles, ["live"], "the roles come from the id");
    assert_eq!(s.provider("t").unwrap().models, out.models, "written as answered");

    // `null` clears the key; a string replaces it.
    let out = s.patch("t", &patch(r#"{"apiKey": null}"#), &[]).unwrap();
    assert_eq!(out.api_key, "");
    let out = s.patch("t", &patch(r#"{"apiKey": "new-key-value"}"#), &[]).unwrap();
    assert_eq!(out.api_key, "new-key-value");
}

#[test]
fn a_refused_patch_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.json");
    let s = ProviderStore::load(&path, small_catalog(), &[]).unwrap();
    s.add_from_template("t", None, None, None, Default::default(), None).unwrap();
    s.update("t", |p| p.models.push(row("big"))).unwrap();
    let before = s.provider("t").unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    let named = [("fallbackModel", "t::a"), ("visionModel", "t::zzz")];

    let refused = |body: &str| s.patch("t", &patch(body), &named).unwrap_err();
    let invalid = |body: &str, part: &str| {
        let Refused::Invalid(msg) = refused(body) else { panic!("{body}") };
        assert!(msg.contains(part), "{body}: {msg}");
    };
    // addModels
    invalid(r#"{"addModels": [{"id": "a"}]}"#, "addModels: model 'a' already exists");
    invalid(r#"{"addModels": [{"id": "models/a"}]}"#, "addModels: model 'models/a' already exists (as 'a')");
    invalid(r#"{"addModels": [{"id": "with space"}]}"#, "addModels: an id is 1 to 200 characters");
    invalid(r#"{"addModels": [{"id": ""}]}"#, "addModels: an id is 1 to 200 characters");
    invalid(&format!(r#"{{"addModels": [{{"id": "{}"}}]}}"#, "x".repeat(201)), "addModels: an id is 1 to 200 characters");
    // models
    invalid(r#"{"models": [{"id": "zzz", "enabled": false}]}"#, "models: unknown model 'zzz'");
    invalid(r#"{"models": [{"id": "a", "roles": ["fast"]}]}"#, "models: 'a': role 'fast' is not one of chat, live");
    invalid(r#"{"models": [{"id": "a", "roles": ["chat", "live", "chat"]}]}"#, "models: 'a': role 'chat' is repeated");
    invalid(r#"{"models": [{"id": "a", "config": {"contextWindow": 0}}]}"#, "config.contextWindow of 'a': a whole number above 0");
    invalid(r#"{"models": [{"id": "a", "config": {"maxOutputTokens": 0}}]}"#, "config.maxOutputTokens of 'a': a whole number above 0");
    invalid(r#"{"models": [{"id": "a", "config": {"reasoningLevels": ["low", " "]}}]}"#, "config.reasoningLevels of 'a': no blank entry");
    invalid(r#"{"models": [{"id": "a", "config": {"reasoningLevels": ["low", "low"]}}]}"#, "config.reasoningLevels of 'a': 'low' is a repeat");
    // The keys of the mapping are the levels the model ends with (`big` has `low` and `high`).
    invalid(r#"{"models": [{"id": "big", "config": {"reasoningPatches": {"mid": {}}}}]}"#, "config.reasoningPatches of 'big': 'mid' is not one of the reasoning levels (low, high)");
    invalid(r#"{"models": [{"id": "big", "config": {"reasoningPatches": {"low": "none"}}}]}"#, "config.reasoningPatches of 'big': every value is an object");
    invalid(r#"{"models": [{"id": "big", "config": {"reasoningLevels": ["x"], "reasoningPatches": {"high": {}}}}]}"#, "config.reasoningPatches of 'big': 'high' is not one of the reasoning levels (x)");
    // Scenario S40 item 4: a manual model holds the six editable fields.
    invalid(r#"{"models": [{"id": "a", "smart": false, "config": {"contextWindow": 9, "reasoningLevels": [], "supportsToolCall": true, "supportsImage": false}}]}"#, "config.maxOutputTokens of 'a': required while smart is false");
    invalid(r#"{"models": [{"id": "a", "smart": false}]}"#, "config.contextWindow of 'a': required while smart is false");
    // removeModels
    assert_eq!(refused(r#"{"removeModels": ["a"]}"#), Refused::Conflict("removeModels: 't::a' is the fallbackModel".into()));
    invalid(r#"{"removeModels": ["nope"]}"#, "removeModels: unknown model 'nope'");
    // modelOrder (the rows are a, b and big)
    invalid(r#"{"modelOrder": ["a"]}"#, "modelOrder: name every model exactly once");
    invalid(r#"{"modelOrder": ["a", "a", "big"]}"#, "modelOrder: name every model exactly once");
    invalid(r#"{"modelOrder": ["a", "b", "nope"]}"#, "modelOrder: name every model exactly once");
    // The refusals come in the order of the spec.
    assert_eq!(refused(r#"{"addModels": [{"id": "a"}], "models": [{"id": "zzz"}], "removeModels": ["a"]}"#), Refused::Invalid("addModels: model 'a' already exists".into()));
    assert!(matches!(refused(r#"{"models": [{"id": "zzz"}], "removeModels": ["a"]}"#), Refused::Invalid(_)), "models before removeModels: 400, not 409");
    assert!(matches!(refused(r#"{"removeModels": ["a"], "modelOrder": ["nope"]}"#), Refused::Conflict(_)), "removeModels before modelOrder");
    assert_eq!(s.provider("t").unwrap(), before, "a refused patch changes nothing in memory");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), disk, "nor in the file");
    assert_eq!(s.patch("nope", &ProviderPatch::default(), &[]), Err(Refused::UnknownProvider));

    // A manual row keeps its rule when only its patch is replaced, and is left alone
    // by a change that names neither `smart` nor `config`.
    let six = r#""config": {"contextWindow": 9, "maxOutputTokens": 5, "reasoningLevels": [], "supportsToolCall": true, "supportsImage": false}"#;
    s.patch("t", &patch(&format!(r#"{{"models": [{{"id": "a", "smart": false, {six}}}]}}"#)), &named).unwrap();
    invalid(r#"{"models": [{"id": "a", "config": {"contextWindow": 9, "maxOutputTokens": 5}}]}"#, "config.reasoningLevels of 'a': required while smart is false");
    let manual = s.provider("t").unwrap().models[0].clone();
    s.patch("t", &patch(r#"{"models": [{"id": "a", "enabled": false}, {"id": "big", "roles": ["live"]}]}"#), &[]).unwrap();
    assert_eq!(s.provider("t").unwrap().models[0].config, manual.config, "a toggle keeps the patch of a manual row");
    assert!(!s.provider("t").unwrap().models[0].enabled);
    // A setting that names a model of another provider does not hold this one.
    s.patch("t", &patch(r#"{"removeModels": ["a"]}"#), &[("fallbackModel", "other::a")]).unwrap();
    assert_eq!(ids(&s.provider("t").unwrap()), ["b", "big"]);
    // A mapping for a level the row itself names is taken and used.
    s.patch("t", &patch(r#"{"models": [{"id": "big", "config": {"reasoningLevels": ["x"], "reasoningPatches": {"x": {"effort": 2}}}}]}"#), &named).unwrap();
    assert_eq!(s.resolve("t::big").unwrap().config.reasoning_patches["x"], json!({"effort": 2}));
}

#[test]
fn the_patch_types_refuse_what_the_spec_does_not_name() {
    assert!(serde_json::from_str::<ProviderPatch>(r#"{"bogus": 1}"#).is_err(), "an unknown key of the body");
    assert!(serde_json::from_str::<ProviderPatch>(r#"{"addModels": [{"id": "x", "enabled": true}]}"#).is_err(), "a new row has no other key");
    assert!(serde_json::from_str::<ProviderPatch>(r#"{"models": [{"id": "x", "configg": {}}]}"#).is_err());
    assert!(serde_json::from_str::<ProviderPatch>(r#"{"apiType": "soap"}"#).is_err(), "another API format");
    assert!(serde_json::from_str::<ProviderPatch>(r#"{"apiType": "ollama-native"}"#).is_ok());
    // `null` clears the key, an empty string keeps it.
    let clear = patch(r#"{"apiKey": null}"#);
    let keep = patch(r#"{"apiKey": ""}"#);
    assert_eq!((clear.api_key, keep.api_key), (Some(None), Some(Some(String::new()))));
    assert!(patch("{}").api_key.is_none());
    assert!(patch(r#"{"modelOrder": []}"#).model_order.is_some(), "an empty list is a list");
}

#[test]
fn a_patch_whose_save_fails_leaves_memory_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir);
    let p = s.add_from_template("ollama", None, None, None, Default::default(), None).unwrap();
    // The file's place becomes a directory: the save fails.
    let path = dir.path().join("providers.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let r = s.patch(&p.id, &patch(r#"{"name": "Renamed"}"#), &[]);
    assert!(matches!(r, Err(Refused::NotSaved(_))), "{r:?}");
    assert_eq!(s.provider(&p.id).unwrap().name, p.name, "not saved is not changed");
    assert!(s.update(&p.id, |e| e.name = "Again".into()).is_err());
    assert_eq!(s.provider(&p.id).unwrap().name, p.name);
}

#[test]
fn a_blank_name_or_an_address_that_is_not_http_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let s = store(&dir);
    let p = s.add_from_template("ollama", None, None, None, Default::default(), None).unwrap();
    for body in [r#"{"name": "  "}"#, r#"{"baseUrl": "localhost:11434"}"#, r#"{"baseUrl": "ftp://x/v1"}"#, r#"{"baseUrl": ""}"#] {
        assert!(matches!(s.patch(&p.id, &patch(body), &[]), Err(Refused::Invalid(_))), "{body}");
    }
    assert_eq!(s.provider(&p.id).unwrap(), p, "nothing changed");
    let ok = s.patch(&p.id, &patch(r#"{"name": " Local ", "baseUrl": " http://127.0.0.1:11434 "}"#), &[]).unwrap();
    assert_eq!((ok.name.as_str(), ok.api.base_url.as_str()), ("Local", "http://127.0.0.1:11434"), "trimmed");
    let err = s.add_from_template("custom", Some("Mine".into()), Some("not a url".into()), None, Default::default(), None).unwrap_err();
    assert!(err.to_string().contains("not an http or https address"), "{err}");
}
