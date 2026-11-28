use std::sync::RwLock;

use super::keys::PROTECTED_KEYS;
use super::*;

struct TestHost {
    settings: RwLock<Settings>,
    providers: ProviderStore,
    dir: tempfile::TempDir,
}

#[async_trait]
impl SettingsHost for TestHost {
    fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }
    fn providers(&self) -> &ProviderStore {
        &self.providers
    }
    async fn apply(&self, patch: Value) -> Result<(), String> {
        crate::settings_apply::patch_and_save(&self.settings, &self.providers, &self.dir.path().join("settings.json"), patch, |_| {}).map(|_| ()).map_err(|e| e.2)
    }
}

/// A host with Gemini (the live model and its voices) and NVIDIA (chat models) added.
fn host() -> Arc<TestHost> {
    let dir = tempfile::tempdir().unwrap();
    let providers = ProviderStore::load(dir.path().join("providers.json"), ProviderStore::builtin_catalog().unwrap(), &[]).unwrap();
    providers.add_from_template("gemini", None, None, Some("sk-very-secret-key-1234".into()), Default::default(), None).unwrap();
    providers.add_from_template("nvidia", None, None, Some("nvapi-very-secret-5678".into()), Default::default(), None).unwrap();
    let mut s = Settings::defaults();
    s.live_model = "gemini::gemini-3.8-live".into();
    Arc::new(TestHost { settings: RwLock::new(s), providers, dir })
}

fn tool(h: &Arc<TestHost>) -> SettingsTool {
    let cell: HostCell = Default::default();
    let host: Arc<dyn SettingsHost> = h.clone();
    let _ = cell.set(host);
    SettingsTool::new(cell)
}

async fn run(t: &SettingsTool, input: Value) -> ToolOutput {
    t.call(input, ToolContext::standalone(std::time::Duration::from_secs(5), 24_000)).await.unwrap()
}

fn first_chat_model(h: &TestHost) -> String {
    let p = h.providers.providers();
    let n = &p.providers["nvidia"];
    format!("nvidia::{}", n.models.iter().find(|m| m.enabled && m.roles.contains(&"chat".to_string())).unwrap().id)
}

#[tokio::test]
async fn get_masks_every_secret_and_shows_the_rest() {
    let h = host();
    h.settings.write().unwrap().patch(json!({"calendar": {"clientId": "abc.apps.example", "home": "Glavni trg 1"}, "foo": {"apiToken": "t-123", "emptyToken": ""}})).unwrap();
    let t = tool(&h);
    let all = run(&t, json!({"action": "get"})).await;
    assert!(!all.is_error);
    let v: Value = serde_json::from_str(&all.content).unwrap();
    assert_eq!(v["calendar"]["clientId"], "<set>");
    assert_eq!(v["foo"]["apiToken"], "<set>");
    assert_eq!(v["foo"]["emptyToken"], "<empty>");
    assert_eq!(v["calendar"]["home"], "Glavni trg 1", "what is not a secret is shown");
    assert_eq!(v["voice"]["brain"], "live", "a missing section shows its defaults");
    assert!(!all.content.contains("abc.apps.example") && !all.content.contains("t-123"));
    let one = run(&t, json!({"action": "get", "key": "calendar.clientId"})).await;
    assert!(one.content.contains("<set>") && !one.content.contains("abc.apps"), "{}", one.content);
    let key = run(&t, json!({"action": "get", "key": "liveVoice"})).await;
    assert_eq!(key.content.trim(), "\"Kore\"");
}

#[tokio::test]
async fn models_lists_refs_and_the_jobs_they_hold_and_no_key() {
    let h = host();
    let chat = first_chat_model(&h);
    h.settings.write().unwrap().chat_model = chat.clone();
    let out = run(&tool(&h), json!({"action": "models"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(!out.content.contains("very-secret") && !out.content.contains("apiKey"), "no key: {}", out.content);
    let v: Value = serde_json::from_str(&out.content).unwrap();
    let ids: Vec<&str> = v["providers"].as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["gemini", "nvidia"], "in the order of the providers");
    let nvidia = v["providers"][1]["models"].as_array().unwrap().iter().find(|m| m["ref"] == chat.as_str()).unwrap().clone();
    assert_eq!(nvidia["jobs"], json!(["chatModel"]));
    let live = v["providers"][0]["models"].as_array().unwrap().iter().find(|m| m["ref"] == "gemini::gemini-3.8-live").unwrap().clone();
    assert_eq!(live["jobs"], json!(["liveModel"]));
    assert!(v["voices"].as_array().unwrap().iter().any(|x| x == "Puck"), "the voices of the live model's template");
}

#[tokio::test]
async fn set_of_two_keys_applies_both_or_none() {
    let h = host();
    let t = tool(&h);
    let ok = run(&t, json!({"action": "set", "changes": {"persona.address": "Ana", "voice.brain": "auto"}})).await;
    assert!(!ok.is_error, "{}", ok.content);
    let s = h.settings();
    assert_eq!((s.persona.address.as_str(), s.voice_config().brain), ("Ana", crate::sections::VoiceBrain::Auto));
    let v: Value = serde_json::from_str(&ok.content).unwrap();
    assert_eq!(v["changed"][0]["key"], "persona.address");
    assert_eq!(v["changed"][0]["new"], "Ana");
    assert!(v["notes"][0].as_str().unwrap().contains("next voice session"));
    // One bad value: the good one is not applied either.
    let bad = run(&t, json!({"action": "set", "changes": {"persona.address": "Bojan", "voice.brain": "bogus"}})).await;
    assert!(bad.is_error && bad.content.contains("nothing was changed"), "{}", bad.content);
    assert_eq!(h.settings().persona.address, "Ana");
    let saved = std::fs::read_to_string(h.dir.path().join("settings.json")).unwrap();
    assert!(saved.contains("Ana") && !saved.contains("Bojan"), "the file holds what was stored");
    // 0 and 9 keys.
    assert!(run(&t, json!({"action": "set", "changes": {}})).await.is_error);
    let nine: Map<String, Value> = (0..9).map(|i| (format!("persona.k{i}"), json!(i))).collect();
    assert!(run(&t, json!({"action": "set", "changes": nine})).await.content.contains("1 to 8"));
}

#[tokio::test]
async fn every_protected_key_is_refused_with_the_reason() {
    let h = host();
    let t = tool(&h);
    let before = h.settings();
    for key in PROTECTED_KEYS {
        let out = run(&t, json!({"action": "set", "changes": {*key: "x"}})).await;
        assert!(out.is_error && out.content.contains("changed on the screen only"), "{key}: {}", out.content);
        // A key below a protected one is protected too.
        let below = run(&t, json!({"action": "set", "changes": {format!("{key}.x"): "x"}})).await;
        assert!(below.is_error && below.content.contains("changed on the screen only"), "{key}.x: {}", below.content);
    }
    run(&t, json!({"action": "set", "changes": {"permissionMode": "trusted", "persona.address": "Ana"}})).await;
    assert_eq!(h.settings(), before, "a patch with one protected key changes nothing");
    // A group that holds a protected setting cannot be set as a whole: a key names one
    // setting. (Found in review: `{"startup": {"autostart": false}}` passed.)
    for (group, value) in [
        ("startup", json!({"autostart": false})),
        ("tasks", json!({"onApproval": "allow"})),
        ("watchers", json!({"enabled": false})),
        ("search", json!({"searxUrl": "http://evil.example"})),
        ("voice", json!({"brain": "routed"})),
    ] {
        let out = run(&t, json!({"action": "set", "changes": {group: value}})).await;
        assert!(out.is_error && out.content.contains("is a group of settings"), "{group}: {}", out.content);
    }
    assert_eq!(h.settings(), before, "and nothing changed");
    // And a refused call asks no question: it says why at once.
    let a = t.assess(&json!({"action": "set", "changes": {"permissionMode": "trusted"}}));
    assert!(a.read_only && !a.needs_approval && !a.always_ask);
}

/// The words of the owner for each key that may be changed by voice: a new key must be put
/// in `PROTECTED_KEYS` or here, so a new safety setting is never open by accident.
const FREE_KEYS: &[&str] = &[
    "chatModel", "fastModel", "fallbackModel", "visionModel", "liveModel", "liveVoice", "maxIterations", "options", "persona", "startWidget", "startup.greeting", "voice",
    "desktop", "watchers.quietHours", "watchers.spoken", "tasks.deliver", "tasks.timeoutMin", "tasks.maxConcurrent", "skills", "compaction", "memory", "search.engine", "subagents",
    "calendar.calendars", "calendar.defaultCalendar", "calendar.defaultDurationMin", "calendar.home", "calendar.travelMarginMin", "calendar.report",
    "screenWatch.enabled", "screenWatch.defaultMinutes", "screenWatch.everySec",
    // The limits the owner types per provider (usage.md section 3.3): a number KAgent counts against, nothing more.
    "usage.knownLimits",
];

#[test]
fn every_key_is_protected_or_free() {
    let view = keys::full_view(&Settings::defaults());
    let under = |k: &str, p: &str| k == p || k.starts_with(&format!("{p}."));
    let mut unclassified = Vec::new();
    for key in keys::leaves(&view) {
        let protected = PROTECTED_KEYS.iter().any(|p| under(&key, p));
        let free = FREE_KEYS.iter().any(|p| under(&key, p));
        if protected == free {
            unclassified.push(format!("{key} ({})", if protected { "both" } else { "neither" }));
        }
    }
    assert!(unclassified.is_empty(), "put each key in PROTECTED_KEYS or FREE_KEYS, not both: {unclassified:?}");
    // The keys the tool is told about exist in the view (a typo in a list protects nothing).
    for p in PROTECTED_KEYS {
        assert!(keys::leaves(&view).iter().any(|k| under(k, p)), "{p} names no setting");
    }
    for p in FREE_KEYS {
        assert!(keys::leaves(&view).iter().any(|k| under(k, p)), "{p} names no setting");
    }
}

#[tokio::test]
async fn what_lets_a_frame_leave_the_machine_is_changed_on_the_screen_only() {
    let h = host();
    let t = tool(&h);
    for key in ["screenWatch.cloudConfirm", "screenWatch.telegram"] {
        let out = run(&t, json!({"action": "set", "changes": {key: true}})).await;
        assert!(out.is_error && out.content.contains("changed on the screen only"), "{key}: {}", out.content);
    }
    assert!(!h.settings().screen_watch_config().cloud_confirm);
    let free = run(&t, json!({"action": "set", "changes": {"screenWatch.everySec": 20}})).await;
    assert!(!free.is_error, "{}", free.content);
    assert_eq!(h.settings().screen_watch_config().every_sec, 20);
}

#[tokio::test]
async fn an_unknown_key_suggests_the_three_nearest() {
    let h = host();
    let out = run(&tool(&h), json!({"action": "set", "changes": {"voice.brian": "live"}})).await;
    assert!(out.is_error, "{}", out.content);
    let list = out.content.split("nearest are: ").nth(1).unwrap();
    assert_eq!(list.split(", ").count(), 3, "{list}");
    assert!(list.starts_with("voice.brain"), "{list}");
    let got = run(&tool(&h), json!({"action": "get", "key": "liveVoyce"})).await;
    assert!(got.is_error && got.content.contains("liveVoice"), "{}", got.content);
}

#[tokio::test]
async fn a_model_that_cannot_chat_is_refused_with_the_routes_message() {
    let h = host();
    let t = tool(&h);
    let out = run(&t, json!({"action": "set", "changes": {"chatModel": "gemini::gemini-3.8-live"}})).await;
    assert!(out.is_error && out.content.contains("is a live voice model and cannot answer text chat"), "{}", out.content);
    let id = h.providers.providers().providers["nvidia"].models[0].id.clone();
    h.providers.update("nvidia", |p| p.models[0].roles.clear()).unwrap();
    let out = run(&t, json!({"action": "set", "changes": {"chatModel": format!("nvidia::{id}")}})).await;
    assert!(out.is_error && out.content.contains("is not a chat model (its roles: no role)"), "{}", out.content);
    assert_eq!(h.settings().chat_model, "");
    let good = first_chat_model(&h);
    h.providers.update("nvidia", |p| p.models.iter_mut().for_each(|m| m.roles = vec!["chat".into()])).unwrap();
    assert!(!run(&t, json!({"action": "set", "changes": {"chatModel": good.clone()}})).await.is_error);
    assert_eq!(h.settings().chat_model, good);
}

#[tokio::test]
async fn the_voice_is_checked_against_the_template_and_spelt_as_it() {
    let h = host();
    let t = tool(&h);
    let out = run(&t, json!({"action": "set", "changes": {"liveVoice": "puck"}})).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(h.settings().live_voice, "Puck");
    assert!(out.content.contains("the voice changes after this sentence"));
    let bad = run(&t, json!({"action": "set", "changes": {"liveVoice": "Nobody"}})).await;
    assert!(bad.is_error && bad.content.contains("the voices are: Kore, Puck"), "{}", bad.content);
    assert_eq!(h.settings().live_voice, "Puck");
}

#[test]
fn the_summary_is_built_from_the_stored_values() {
    let h = host();
    let t = tool(&h);
    h.settings.write().unwrap().persona.address.clear();
    let a = t.assess(&json!({"action": "set", "changes": {"persona.address": "Marko"}}));
    assert_eq!(a.summary, "change setting persona.address from empty to Marko");
    assert!(a.always_ask && !a.read_only && a.needs_approval);
    h.settings.write().unwrap().chat_model = "nvidia::meta/llama-3.3-70b".into();
    let a = t.assess(&json!({"action": "set", "changes": {"chatModel": "gemini::models/gemini-3-flash", "liveVoice": "Puck"}}));
    assert_eq!(a.summary, "change setting chatModel from meta/llama-3.3-70b to gemini-3-flash; change setting liveVoice from Kore to Puck");
    assert!(t.assess(&json!({"action": "get"})).read_only && t.assess(&json!({"action": "models"})).read_only);
    // Never rememberable: the contract asks every time.
    assert!(t.contract().always_ask);
}
