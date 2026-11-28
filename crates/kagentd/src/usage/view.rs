//! The answer of `GET /api/usage` and what the tool and the provider list read from it: the limits of a provider (what
//! the providers said, and the owner's known limits counted by KAgent) and its counters.

use chrono::{DateTime, Datelike, Local, Months, TimeZone, Timelike, Utc};
use kagent_providers::usage::{Limit, Source, Unit};
use serde_json::{json, Value};

use super::limits::tightest;
use super::settings::{KnownLimit, KnownUnit, KnownWindow};
use super::store::Window;
use super::UsageHub;

/// The end of the window `at` falls in, in local time.
fn window_end(window: KnownWindow, at: DateTime<Local>) -> Option<DateTime<Utc>> {
    let naive = match window {
        KnownWindow::Minute => (at.naive_local().with_second(0)?.with_nanosecond(0)?) + chrono::Duration::minutes(1),
        KnownWindow::Day => at.date_naive().succ_opt()?.and_hms_opt(0, 0, 0)?,
        KnownWindow::Month => at.date_naive().with_day(1)?.checked_add_months(Months::new(1))?.and_hms_opt(0, 0, 0)?,
    };
    Local.from_local_datetime(&naive).earliest().map(|t| t.with_timezone(&Utc))
}

impl UsageHub {
    /// A known limit as a limit: KAgent's own count of this window against the number the owner typed.
    fn known_limit(&self, provider: &str, k: &KnownLimit, at: DateTime<Local>) -> Limit {
        let (window, word, name) = match k.window {
            KnownWindow::Minute => (Window::Minute, "minute", "per minute"),
            KnownWindow::Day => (Window::Day, "day", "per day"),
            KnownWindow::Month => (Window::Month, "month", "per month"),
        };
        let totals = self.counters.totals(provider, window, at);
        let (used, unit, what) = match k.unit {
            KnownUnit::Requests => (totals.requests, Unit::Requests, "Requests"),
            KnownUnit::Tokens => (totals.input_tokens + totals.output_tokens, Unit::Tokens, "Tokens"),
        };
        Limit::new(&format!("known-{word}-{}", if unit == Unit::Requests { "requests" } else { "tokens" }), &format!("{what} {name}"), Some(used), Some(k.total), unit, window_end(k.window, at), Source::Kagent)
    }

    /// Every limit of `provider` at `at`: the providers' own that still hold, then the known ones. A local provider has none.
    pub fn provider_limits_at(&self, provider: &str, local: bool, at: DateTime<Local>) -> Vec<Limit> {
        if local {
            return Vec::new();
        }
        let mut out = self.limits.limits(provider, at.with_timezone(&Utc));
        let known = self.settings.read().unwrap().usage_config().known_limits.get(provider).cloned().unwrap_or_default();
        out.extend(known.iter().map(|k| self.known_limit(provider, k, at)));
        out
    }

    /// The tightest limit of `provider`, or none (a local provider, or one that said nothing).
    pub fn tightest_at(&self, provider: &str, local: bool, at: DateTime<Local>) -> Option<Limit> {
        tightest(&self.provider_limits_at(provider, local, at)).cloned()
    }

    /// `tightestLimit` on each provider row of `GET /api/providers` (usage.md section 5).
    pub fn add_tightest(&self, rows: &mut [Value], at: DateTime<Local>) {
        for row in rows {
            let (id, local) = (row["id"].as_str().unwrap_or("").to_string(), row["local"].as_bool().unwrap_or(false));
            row["tightestLimit"] = serde_json::to_value(self.tightest_at(&id, local, at)).unwrap_or(Value::Null);
        }
    }

    /// `GET /api/usage` at `at`.
    pub fn view_at(&self, at: DateTime<Local>) -> Value {
        let file = self.providers.providers();
        let known = self.settings.read().unwrap().usage_config().known_limits;
        let rows: Vec<Value> = file
            .order
            .iter()
            .filter_map(|id| file.providers.get(id))
            .map(|p| {
                let local = kagent_providers::is_local_url(&p.api.base_url);
                let limits = self.provider_limits_at(&p.id, local, at);
                json!({
                    "id": p.id,
                    "name": p.name,
                    "local": local,
                    "tightest": tightest(&limits),
                    "limits": limits,
                    "today": self.counters.totals(&p.id, Window::Day, at),
                    "month": self.counters.totals(&p.id, Window::Month, at),
                    "models": self.counters.per_model(&p.id, at),
                    "knownLimits": known.get(&p.id).cloned().unwrap_or_default(),
                    "checkedAt": self.limits.checked(&p.id).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
                })
            })
            .collect();
        json!({ "providers": rows })
    }

    pub fn view(&self) -> Value {
        self.view_at(Local::now())
    }
}
