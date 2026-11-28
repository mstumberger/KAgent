//! Rate-limit headers (usage.md section 3.1). Two families, both read on every response. A missing or malformed header
//! gives no limit, never a wrong one.
//!
//! The samples of the tests are written from the providers' documentation (Anthropic's rate limit headers, OpenAI's
//! "rate limits" guide, Groq's), NOT recorded from real answers: check them against real responses.

use chrono::{DateTime, Duration, TimeZone, Utc};

use super::{Headers, Limit, Source, Unit};

/// A duration as OpenAI and Groq write it: `1s`, `6m0s`, `2m59.56s`, `20ms`, `1h2m3.5s`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let (mut rest, mut total_ms) = (t, 0f64);
    while !rest.is_empty() {
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        if end == 0 {
            return None;
        }
        let n: f64 = rest[..end].parse().ok()?;
        rest = &rest[end..];
        let (factor, len) = if rest.starts_with("ms") {
            (1.0, 2)
        } else if rest.starts_with('h') {
            (3_600_000.0, 1)
        } else if rest.starts_with('m') {
            (60_000.0, 1)
        } else if rest.starts_with('s') {
            (1000.0, 1)
        } else {
            return None;
        };
        total_ms += n * factor;
        rest = &rest[len..];
    }
    Some(Duration::milliseconds(total_ms.round() as i64))
}

/// A reset as a time: a duration, a number (under 1e9 seconds from now, under 1e12 an epoch in seconds, else an epoch in
/// milliseconds), or an RFC 3339 time. Anything else is malformed.
pub fn parse_reset(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let t = text.trim();
    if let Ok(n) = t.parse::<f64>() {
        if !n.is_finite() || n < 0.0 {
            return None;
        }
        return if n < 1e9 {
            Some(now + Duration::milliseconds((n * 1000.0).round() as i64))
        } else if n < 1e12 {
            Utc.timestamp_opt(n as i64, 0).single()
        } else {
            Utc.timestamp_millis_opt(n as i64).single()
        };
    }
    if let Some(d) = parse_duration(t) {
        return Some(now + d);
    }
    DateTime::parse_from_rfc3339(t).ok().map(|d| d.with_timezone(&Utc))
}

/// `retry-after`: seconds, or an HTTP date.
pub fn retry_after(headers: &Headers, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let v = headers.get("retry-after")?.trim();
    if let Ok(secs) = v.parse::<u64>() {
        return Some(now + Duration::seconds(secs as i64));
    }
    DateTime::parse_from_rfc2822(v).ok().map(|d| d.with_timezone(&Utc))
}

/// One family member: `limit` and `remaining` must be whole numbers with `0 < limit` and `remaining <= limit`; a reset that is there
/// and cannot be read drops the limit, one that is not there leaves the reset empty.
fn member(h: &Headers, keys: [String; 3], id: &str, name: &str, unit: Unit, now: DateTime<Utc>) -> Option<Limit> {
    let total: u64 = h.get(&keys[0])?.trim().parse().ok()?;
    let left: u64 = h.get(&keys[1])?.trim().parse().ok()?;
    if total == 0 || left > total {
        return None;
    }
    let reset = match h.get(&keys[2]) {
        None => None,
        Some(v) => Some(parse_reset(v, now)?),
    };
    Some(Limit::new(id, name, Some(total - left), Some(total), unit, reset, Source::Provider))
}

/// Every limit the headers of one response give.
pub fn limits_from_headers(h: &Headers, now: DateTime<Utc>) -> Vec<Limit> {
    let mut out = Vec::new();
    for (key, id, name, unit) in [
        ("requests", "requests", "Requests", Unit::Requests),
        ("tokens", "tokens", "Tokens", Unit::Tokens),
        ("input-tokens", "input-tokens", "Input tokens", Unit::Tokens),
        ("output-tokens", "output-tokens", "Output tokens", Unit::Tokens),
    ] {
        let keys = ["limit", "remaining", "reset"].map(|part| format!("anthropic-ratelimit-{key}-{part}"));
        out.extend(member(h, keys, id, name, unit, now));
    }
    for (key, name, unit) in [("requests", "Requests", Unit::Requests), ("tokens", "Tokens", Unit::Tokens)] {
        // An Anthropic header of the same id wins: the two families are not mixed on one provider.
        if out.iter().any(|l| l.id == key) {
            continue;
        }
        let keys = [format!("x-ratelimit-limit-{key}"), format!("x-ratelimit-remaining-{key}"), format!("x-ratelimit-reset-{key}")];
        out.extend(member(h, keys, key, name, unit, now));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Level;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-28T09:00:00Z").unwrap().with_timezone(&Utc)
    }
    fn headers(pairs: &[(&str, &str)]) -> Headers {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    // FIXTURE: from documentation, not recorded. Anthropic, "Rate limits" (response headers).
    const ANTHROPIC: &[(&str, &str)] = &[
        ("anthropic-ratelimit-requests-limit", "50"),
        ("anthropic-ratelimit-requests-remaining", "49"),
        ("anthropic-ratelimit-requests-reset", "2026-11-28T09:00:30Z"),
        ("anthropic-ratelimit-tokens-limit", "40000"),
        ("anthropic-ratelimit-tokens-remaining", "10000"),
        ("anthropic-ratelimit-tokens-reset", "2026-11-28T09:00:45Z"),
        ("anthropic-ratelimit-input-tokens-limit", "30000"),
        ("anthropic-ratelimit-input-tokens-remaining", "30000"),
        ("anthropic-ratelimit-input-tokens-reset", "2026-11-28T09:00:00Z"),
        ("anthropic-ratelimit-output-tokens-limit", "8000"),
        ("anthropic-ratelimit-output-tokens-remaining", "400"),
        ("anthropic-ratelimit-output-tokens-reset", "2026-11-28T09:00:10Z"),
    ];

    #[test]
    fn anthropic_headers_give_four_limits() {
        let got = limits_from_headers(&headers(ANTHROPIC), now());
        let by = |id: &str| got.iter().find(|l| l.id == id).unwrap_or_else(|| panic!("no {id}"));
        assert_eq!(got.len(), 4);
        assert_eq!((by("requests").used, by("requests").total, by("requests").level), (Some(1), Some(50), Level::Fine));
        assert_eq!((by("tokens").used, by("tokens").total, by("tokens").level), (Some(30000), Some(40000), Level::Close));
        assert_eq!(by("input-tokens").used, Some(0));
        assert_eq!((by("output-tokens").used, by("output-tokens").level), (Some(7600), Level::NearlyOut));
        assert_eq!(by("requests").resets_at.as_deref(), Some("2026-11-28T09:00:30Z"));
        assert_eq!(by("requests").source, Source::Provider);
        assert_eq!(by("tokens").unit, Unit::Tokens);
    }

    // FIXTURE: from documentation, not recorded. OpenAI, "Rate limits" guide; Groq writes the same names.
    const OPENAI: &[(&str, &str)] = &[
        ("x-ratelimit-limit-requests", "60"),
        ("x-ratelimit-remaining-requests", "59"),
        ("x-ratelimit-reset-requests", "1s"),
        ("x-ratelimit-limit-tokens", "150000"),
        ("x-ratelimit-remaining-tokens", "149984"),
        ("x-ratelimit-reset-tokens", "6m0s"),
    ];

    #[test]
    fn openai_style_headers_give_requests_and_tokens() {
        let got = limits_from_headers(&headers(OPENAI), now());
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].id.as_str(), got[0].used, got[0].total), ("requests", Some(1), Some(60)));
        assert_eq!(got[0].resets_at.as_deref(), Some("2026-11-28T09:00:01Z"));
        assert_eq!((got[1].id.as_str(), got[1].used, got[1].total), ("tokens", Some(16), Some(150000)));
        assert_eq!(got[1].resets_at.as_deref(), Some("2026-11-28T09:06:00Z"));
    }

    #[test]
    fn every_reset_format() {
        let n = now();
        let at = |s: &str| parse_reset(s, n).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        assert_eq!(at("1s").as_deref(), Some("2026-11-28T09:00:01.000Z"));
        assert_eq!(at("6m0s").as_deref(), Some("2026-11-28T09:06:00.000Z"));
        assert_eq!(at("2m59.56s").as_deref(), Some("2026-11-28T09:02:59.560Z"));
        assert_eq!(at("20ms").as_deref(), Some("2026-11-28T09:00:00.020Z"));
        assert_eq!(at("1h2m3.5s").as_deref(), Some("2026-11-28T10:02:03.500Z"));
        assert_eq!(at("30").as_deref(), Some("2026-11-28T09:00:30.000Z"), "seconds from now");
        assert_eq!(at("1795856400").as_deref(), Some("2026-11-28T09:00:00.000Z"), "an epoch in seconds");
        assert_eq!(at("1795856400000").as_deref(), Some("2026-11-28T09:00:00.000Z"), "an epoch in milliseconds");
        assert_eq!(at("2026-11-28T09:30:00+01:00").as_deref(), Some("2026-11-28T08:30:00.000Z"));
        for bad in ["", "soon", "5x", "m", "-3", "1.2.3s", "NaN", "inf"] {
            assert_eq!(at(bad), None, "{bad:?} is malformed");
        }
    }

    #[test]
    fn a_missing_or_malformed_header_gives_no_limit() {
        let mut h = headers(OPENAI);
        h.remove("x-ratelimit-remaining-requests");
        assert!(limits_from_headers(&h, now()).iter().all(|l| l.id != "requests"), "no remaining: no limit");
        let mut h = headers(OPENAI);
        h.insert("x-ratelimit-limit-requests".into(), "many".into());
        assert!(limits_from_headers(&h, now()).iter().all(|l| l.id != "requests"), "not a number");
        let mut h = headers(OPENAI);
        h.insert("x-ratelimit-remaining-requests".into(), "61".into());
        assert!(limits_from_headers(&h, now()).iter().all(|l| l.id != "requests"), "more left than the limit");
        let mut h = headers(OPENAI);
        h.insert("x-ratelimit-limit-requests".into(), "0".into());
        h.insert("x-ratelimit-remaining-requests".into(), "0".into());
        assert!(limits_from_headers(&h, now()).iter().all(|l| l.id != "requests"), "a limit of 0");
        let mut h = headers(OPENAI);
        h.insert("x-ratelimit-reset-requests".into(), "soon".into());
        assert!(limits_from_headers(&h, now()).iter().all(|l| l.id != "requests"), "an unreadable reset drops it");
        assert_eq!(limits_from_headers(&h, now()).len(), 1, "the tokens limit is untouched");
        let mut h = headers(OPENAI);
        h.remove("x-ratelimit-reset-requests");
        assert_eq!(limits_from_headers(&h, now())[0].resets_at, None, "no reset header: the limit stays, with no reset time");
        assert!(limits_from_headers(&Headers::new(), now()).is_empty());
        assert!(limits_from_headers(&headers(&[("x-ratelimit-limit", "5")]), now()).is_empty(), "a bare name is not one of the families");
    }

    #[test]
    fn retry_after_is_seconds_or_a_date() {
        assert_eq!(retry_after(&headers(&[("retry-after", "20")]), now()).unwrap().to_rfc3339(), "2026-11-28T09:00:20+00:00");
        // FIXTURE: from documentation, not recorded (RFC 9110 IMF-fixdate).
        assert_eq!(retry_after(&headers(&[("retry-after", "Sat, 28 Nov 2026 09:05:00 GMT")]), now()).unwrap().to_rfc3339(), "2026-11-28T09:05:00+00:00");
        assert_eq!(retry_after(&headers(&[("retry-after", "later")]), now()), None);
        assert_eq!(retry_after(&Headers::new(), now()), None);
    }
}
