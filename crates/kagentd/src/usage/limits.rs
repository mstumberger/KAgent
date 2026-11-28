//! The limits each provider last said (usage.md section 4), in memory: they are what the last answers said, a restart
//! starts empty. A limit goes when its reset has passed, or (no reset given) ten minutes after it was seen.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use kagent_providers::usage::{Level, Limit};

/// A limit that names no reset is dropped this long after it was seen, unless a newer answer repeats it.
const NO_RESET_TTL_MIN: i64 = 10;

struct Entry {
    limit: Limit,
    seen: DateTime<Utc>,
}

impl Entry {
    fn alive(&self, now: DateTime<Utc>) -> bool {
        match self.limit.resets() {
            Some(reset) => reset > now,
            None => now - self.seen < Duration::minutes(NO_RESET_TTL_MIN),
        }
    }
}

#[derive(Default)]
struct State {
    entries: BTreeMap<String, Entry>,
    checked: Option<DateTime<Utc>>,
}

impl State {
    fn levels(&self, now: DateTime<Utc>) -> BTreeMap<String, Level> {
        self.entries.iter().filter(|(_, e)| e.alive(now)).map(|(id, e)| (id.clone(), e.limit.level)).collect()
    }
}

#[derive(Default)]
pub struct LimitStore {
    inner: Mutex<HashMap<String, State>>,
}

impl LimitStore {
    /// An answer said these limits at `seen`. A limit with the same id is replaced; the others stay. `true` when the
    /// set of levels changed (a limit appeared, went away or changed level).
    pub fn observe(&self, provider: &str, limits: Vec<Limit>, seen: DateTime<Utc>) -> bool {
        let mut all = self.inner.lock().unwrap();
        let state = all.entry(provider.to_string()).or_default();
        let before = state.levels(seen);
        state.entries.retain(|_, e| e.alive(seen));
        if !limits.is_empty() {
            state.checked = Some(seen);
        }
        for limit in limits {
            state.entries.insert(limit.id.clone(), Entry { limit, seen });
        }
        before != state.levels(seen)
    }

    /// The limits of `provider` that are still true at `now`.
    pub fn limits(&self, provider: &str, now: DateTime<Utc>) -> Vec<Limit> {
        let all = self.inner.lock().unwrap();
        all.get(provider).map(|s| s.entries.values().filter(|e| e.alive(now)).map(|e| e.limit.clone()).collect()).unwrap_or_default()
    }

    /// When the last answer that carried a limit was seen.
    pub fn checked(&self, provider: &str) -> Option<DateTime<Utc>> {
        self.inner.lock().unwrap().get(provider).and_then(|s| s.checked)
    }
}

/// The tightest of `limits`: the highest `used / total` (a limit with no total counts as full), the out ones first.
pub fn tightest(limits: &[Limit]) -> Option<&Limit> {
    limits.iter().max_by(|a, b| (a.level == Level::Out, a.fraction()).partial_cmp(&(b.level == Level::Out, b.fraction())).unwrap_or(std::cmp::Ordering::Equal))
}
