//! The keys of the settings as the `settings` tool sees them: the full view (defaults of
//! the typed sections filled in), dotted paths, the protected keys and the secrets.

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::sections::edit_distance;
use crate::settings::Settings;

/// Keys (and prefixes: `hooks` is `hooks.*`) that `set` refuses, whoever asks. They decide
/// what the agent may do or reach, or hold a secret: they are changed on the screen only.
/// `tests::every_key_is_protected_or_free` makes a new key choose a side.
pub const PROTECTED_KEYS: &[&str] = &[
    "permissionMode",
    "rules",
    "toolOverrides",
    "toolGroups",
    "mcpToolGroups",
    "privilege",
    "approvalTimeoutSec",
    "hooks",
    "telegram",
    "calendar.enabled",
    "calendar.clientId",
    // screen-watch.md section 4: it lets a frame leave the machine; telegram sends what the screen showed.
    "screenWatch.cloudConfirm",
    "screenWatch.telegram",
    "schemaVersion",
    "knownToolGroups",
    // Decided in S44 (the spec's list is the minimum): what reaches the network or the
    // owner's browser, the router, the unit that starts the daemon, the watchers.
    "browser",
    "mikrotik",
    "sentinel",
    "search.searxUrl",
    "tasks.onApproval",
    "startup.autostart",
    "watchers.enabled",
    "watchers.disabled",
];

fn under(key: &str, prefix: &str) -> bool {
    key == prefix || key.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('.'))
}

pub fn is_protected(key: &str) -> bool {
    PROTECTED_KEYS.iter().any(|p| under(key, p))
}

/// A value that must not be shown: a key, token, secret or password by its name.
pub fn is_secret(path: &str) -> bool {
    let last = path.rsplit('.').next().unwrap_or("").to_lowercase();
    ["token", "secret", "password", "apikey", "api_key"].iter().any(|w| last.contains(w)) || path == "calendar.clientId"
}

/// Keys that exist in the settings but may be absent from a file that never set them.
fn known_defaults() -> Value {
    json!({
        "visionModel": "",
        "startWidget": true,
        "desktop": {"editor": "", "projectRoots": []},
        "watchers": {"enabled": true, "disabled": [], "quietHours": "23:00-07:00", "spoken": false},
        "sentinel": {"honeytokens": false, "persistenceWatch": true},
        "mikrotik": {"host": "", "user": "kagent-ro", "fingerprint": "", "flowPort": 2055, "honeypotIp": "", "trapUser": "", "https": false},
    })
}

fn section<T: Serialize + serde::de::DeserializeOwned + Default>(s: &Settings, key: &str) -> Value {
    serde_json::to_value(s.section::<T>(key).unwrap_or_default()).unwrap_or(Value::Null)
}

/// The settings as the API shows them, plus what a missing section or key would read as.
pub fn full_view(s: &Settings) -> Value {
    let mut view = serde_json::to_value(s).unwrap_or(Value::Null);
    let Value::Object(m) = &mut view else { return view };
    let sections = [
        ("voice", section::<crate::sections::VoiceSection>(s, "voice")),
        ("browser", section::<crate::sections::BrowserSection>(s, "browser")),
        ("tasks", section::<crate::sections::TasksSection>(s, "tasks")),
        ("skills", section::<crate::sections::SkillsSection>(s, "skills")),
        ("compaction", section::<crate::sections::CompactionSection>(s, "compaction")),
        ("memory", section::<crate::memory::MemorySection>(s, "memory")),
        ("search", section::<crate::web::settings::SearchSection>(s, "search")),
        ("telegram", section::<crate::bots::settings::TelegramSection>(s, "telegram")),
        ("calendar", section::<crate::calendar::settings::CalendarSection>(s, "calendar")),
        ("screenWatch", section::<crate::screen_watch::ScreenWatchSection>(s, "screenWatch")),
        ("subagents", section::<crate::subagents::SubagentsSection>(s, "subagents")),
        ("hooks", section::<crate::hooks::config::HooksSection>(s, "hooks")),
    ];
    for (key, v) in sections {
        m.insert(key.into(), v);
    }
    // What the file holds wins over a default; a default fills the gap.
    if let Value::Object(defaults) = known_defaults() {
        for (k, d) in defaults {
            match m.get_mut(&k) {
                Some(have) if have.is_object() && d.is_object() => {
                    let mut merged = d.clone();
                    crate::settings::merge_patch(&mut merged, have);
                    *have = merged;
                }
                Some(_) => {}
                None => {
                    m.insert(k, d);
                }
            }
        }
    }
    view
}

/// Every dotted path of `view` that holds a value (an object that is empty counts).
pub fn leaves(view: &Value) -> Vec<String> {
    fn walk(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(m) if !m.is_empty() => m.iter().for_each(|(k, v)| walk(v, &if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out)),
            _ => out.push(path.to_string()),
        }
    }
    let mut out = Vec::new();
    walk(view, "", &mut out);
    out
}

/// The value at a dotted path (an object too), or `None`.
pub fn path_get<'a>(view: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(view, |v, part| v.get(part))
}

/// `{"voice.brain": "live"}` as the nested patch `{"voice": {"brain": "live"}}`.
pub fn patch_of(changes: &Map<String, Value>) -> Value {
    let mut patch = Value::Object(Map::new());
    for (key, value) in changes {
        let nested = key.rsplit('.').fold(value.clone(), |inner, part| json!({ part: inner }));
        crate::settings::merge_patch(&mut patch, &nested);
    }
    patch
}

/// The `n` known keys nearest to `key` (case ignored; a key that holds the typed one first).
pub fn nearest(key: &str, known: &[String], n: usize) -> Vec<String> {
    let lower = key.to_lowercase();
    let mut scored: Vec<(usize, &String)> = known
        .iter()
        .map(|k| {
            let kl = k.to_lowercase();
            let leaf = kl.rsplit('.').next().unwrap_or("");
            let d = edit_distance(&lower, &kl).min(edit_distance(&lower, leaf) + 1);
            (if kl.contains(&lower) { 0 } else { d }, k)
        })
        .collect();
    scored.sort();
    scored.into_iter().take(n).map(|(_, k)| k.clone()).collect()
}
