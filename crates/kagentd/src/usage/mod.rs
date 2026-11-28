//! Usage and limits (usage.md): the counters of every answer, in `store.rs`; the hub that the providers client
//! reports to, and the view behind `GET /api/usage`, here; the routes in `http.rs`.

pub mod http;
pub mod store;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local, Utc};
use kagent_core::model::CallSource;
use kagent_core::protocol::{AgentEvent, Usage};
use kagent_providers::usage::UsageSink;
use kagent_providers::ProviderStore;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use self::store::{Counters, Window};

/// While answers keep coming, `usageChanged` goes out at most this often per provider (usage.md section 4).
const EMIT_EVERY_SECS: i64 = 30;

pub struct UsageHub {
    pub counters: Counters,
    providers: Arc<ProviderStore>,
    events: broadcast::Sender<AgentEvent>,
    last_emit: Mutex<HashMap<String, DateTime<Utc>>>,
}

impl UsageHub {
    pub fn new(counters: Counters, providers: Arc<ProviderStore>, events: broadcast::Sender<AgentEvent>) -> Arc<Self> {
        counters.prune(Local::now());
        Arc::new(Self { counters, providers, events, last_emit: Mutex::new(HashMap::new()) })
    }

    /// The counters in `<data>/usage.sqlite`; in memory (and a warning) when the file cannot be opened.
    pub fn open(data: &std::path::Path, providers: Arc<ProviderStore>, events: broadcast::Sender<AgentEvent>) -> Arc<Self> {
        let counters = Counters::open(&data.join("usage.sqlite")).unwrap_or_else(|e| {
            tracing::warn!("usage counters are kept in memory only: {e:#}");
            Counters::memory().expect("an in-memory database")
        });
        Self::new(counters, providers, events)
    }

    /// An answer that ended well: counted, and `usageChanged` when it is time.
    pub fn count_at(&self, at: DateTime<Local>, provider: &str, model: &str, source: CallSource, usage: &Usage) {
        if let Err(e) = self.counters.record(at, provider, model, source.as_str(), usage.input_tokens, usage.output_tokens) {
            tracing::warn!("usage not counted: {e:#}");
            return;
        }
        self.notify(provider, false, at.with_timezone(&Utc));
    }

    /// `usageChanged {provider}` when `urgent` (a level changed), else at most every 30 s per provider.
    pub fn notify(&self, provider: &str, urgent: bool, now: DateTime<Utc>) {
        let mut last = self.last_emit.lock().unwrap();
        if !urgent && last.get(provider).is_some_and(|t| (now - *t).num_seconds() < EMIT_EVERY_SECS) {
            return;
        }
        last.insert(provider.to_string(), now);
        let _ = self.events.send(AgentEvent::UsageChanged { provider: provider.to_string() });
    }

    /// `GET /api/usage` at `at`.
    pub fn view_at(&self, at: DateTime<Local>) -> Value {
        let file = self.providers.providers();
        let rows: Vec<Value> = file
            .order
            .iter()
            .filter_map(|id| file.providers.get(id))
            .map(|p| {
                json!({
                    "id": p.id,
                    "name": p.name,
                    "local": kagent_providers::is_local_url(&p.api.base_url),
                    "limits": [],
                    "tightest": null,
                    "today": self.counters.totals(&p.id, Window::Day, at),
                    "month": self.counters.totals(&p.id, Window::Month, at),
                    "models": self.counters.per_model(&p.id, at),
                    "knownLimits": [],
                    "checkedAt": null,
                })
            })
            .collect();
        json!({ "providers": rows })
    }

    pub fn view(&self) -> Value {
        self.view_at(Local::now())
    }
}

impl UsageSink for UsageHub {
    fn counted(&self, provider: &str, model: &str, source: CallSource, usage: &Usage) {
        self.count_at(Local::now(), provider, model, source, usage);
    }
}
