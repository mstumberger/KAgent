//! What a provider says about usage (usage.md section 3): the limit type, its levels, and the seam the daemon plugs into.

use chrono::{DateTime, Utc};
use kagent_core::model::CallSource;
use kagent_core::protocol::Usage;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Under 70 %.
    Fine,
    /// 70 to 90 % inclusive.
    Close,
    /// Over 90 %.
    NearlyOut,
    /// 100 %, or a 429 that named the limit.
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unit {
    Requests,
    Tokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// A header or the body of an error: the provider said it.
    Provider,
    /// KAgent's own count against a limit the owner typed.
    Kagent,
}

/// One limit of a provider (usage.md section 3). `used` and `total` are `None` for a limit that is known to be out but
/// whose size is not.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limit {
    pub id: String,
    pub name: String,
    pub used: Option<u64>,
    pub total: Option<u64>,
    pub unit: Unit,
    pub resets_at: Option<String>,
    pub source: Source,
    pub level: Level,
}

/// The level of `used` of `total`: under 70 % fine, 70 to 90 % close, over 90 % nearly out, 100 % out. No total: out.
pub fn level_of(used: Option<u64>, total: Option<u64>) -> Level {
    let (Some(used), Some(total)) = (used, total) else { return Level::Out };
    let (u, t) = (used as u128 * 100, total as u128);
    if total == 0 || used >= total {
        Level::Out
    } else if u > t * 90 {
        Level::NearlyOut
    } else if u >= t * 70 {
        Level::Close
    } else {
        Level::Fine
    }
}

impl Limit {
    pub fn new(id: &str, name: &str, used: Option<u64>, total: Option<u64>, unit: Unit, resets_at: Option<DateTime<Utc>>, source: Source) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            used,
            total,
            unit,
            resets_at: resets_at.map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            source,
            level: level_of(used, total),
        }
    }

    /// `used / total` as a fraction of 1 (1 for a limit with no total: it is out).
    pub fn fraction(&self) -> f64 {
        match (self.used, self.total) {
            (Some(u), Some(t)) if t > 0 => (u as f64 / t as f64).min(1.0),
            _ => 1.0,
        }
    }

    pub fn resets(&self) -> Option<DateTime<Utc>> {
        self.resets_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc))
    }
}

/// The seam of the daemon's usage module: the client calls it, it stores. `counted` after a response that ended well.
pub trait UsageSink: Send + Sync {
    fn counted(&self, provider: &str, model: &str, source: CallSource, usage: &Usage);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_levels_at_their_edges() {
        let at = |used| level_of(Some(used), Some(100));
        assert_eq!(at(0), Level::Fine);
        assert_eq!(at(69), Level::Fine);
        assert_eq!(at(70), Level::Close);
        assert_eq!(at(90), Level::Close);
        assert_eq!(at(91), Level::NearlyOut);
        assert_eq!(at(99), Level::NearlyOut);
        assert_eq!(at(100), Level::Out);
        assert_eq!(at(120), Level::Out);
        assert_eq!(level_of(Some(901), Some(1000)), Level::NearlyOut, "90.1 %");
        assert_eq!(level_of(Some(900), Some(1000)), Level::Close, "exactly 90 %");
        assert_eq!(level_of(None, None), Level::Out);
        assert_eq!(level_of(Some(0), Some(0)), Level::Out, "a limit of nothing is out, not a division by zero");
    }

    #[test]
    fn a_limit_says_its_fraction_and_reset() {
        let now = DateTime::parse_from_rfc3339("2026-11-28T09:00:00Z").unwrap().with_timezone(&Utc);
        let l = Limit::new("requests", "Requests", Some(47), Some(50), Unit::Requests, Some(now), Source::Provider);
        assert_eq!(l.level, Level::NearlyOut);
        assert!((l.fraction() - 0.94).abs() < 1e-9);
        assert_eq!(l.resets(), Some(now));
        assert_eq!(serde_json::to_value(&l).unwrap()["level"], "nearlyOut");
        assert_eq!(Limit::new("rate", "Rate limit", None, None, Unit::Requests, None, Source::Provider).fraction(), 1.0);
    }
}
