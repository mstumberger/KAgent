//! The body of a 429 (usage.md section 3.2): Google's `QuotaFailure` and `RetryInfo` (Gemini), the OpenAI-style message
//! ("Limit 3, Used 3 ... try again in 20s"), `insufficient_quota`, and any other 429 with only `retry-after`.
//! A 429 never invents a number it was not given.
//!
//! The samples of the tests are written from the providers' documentation (Google's error model and the Gemini API's
//! "429 RESOURCE_EXHAUSTED", OpenAI's error codes, Anthropic's errors page), NOT recorded from real answers.

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;

use super::headers::{parse_duration, retry_after};
use super::{Headers, Limit, Source, Unit};

/// The limits a 429 says. `body` is the text of the answer (JSON or not).
pub fn limits_from_error(body: &str, headers: &Headers, now: DateTime<Utc>) -> Vec<Limit> {
    let json: Option<Value> = serde_json::from_str(body.trim()).ok().map(|v: Value| match v {
        Value::Array(mut a) if !a.is_empty() => a.remove(0),
        other => other,
    });
    let header_reset = retry_after(headers, now);
    if let Some(error) = json.as_ref().and_then(|v| v.get("error")) {
        let google = google(error, now);
        if !google.is_empty() {
            return google;
        }
        if let Some(l) = openai_message(error, header_reset, now) {
            return vec![l];
        }
        let code = error.get("code").and_then(Value::as_str).unwrap_or("");
        let kind = error.get("type").and_then(Value::as_str).unwrap_or("");
        if code == "insufficient_quota" || kind == "insufficient_quota" {
            return vec![Limit::new("quota", "Quota", None, None, Unit::Requests, header_reset, Source::Provider)];
        }
    }
    vec![Limit::new("rate", "Rate limit", None, None, Unit::Requests, header_reset, Source::Provider)]
}

/// `error.details[]`: a `QuotaFailure` with its violations, and the `RetryInfo` delay.
fn google(error: &Value, now: DateTime<Utc>) -> Vec<Limit> {
    let details = error.get("details").and_then(Value::as_array).cloned().unwrap_or_default();
    let of_type = |suffix: &str| details.iter().find(|d| d.get("@type").and_then(Value::as_str).is_some_and(|t| t.ends_with(suffix)));
    let delay = of_type("RetryInfo").and_then(|d| d.get("retryDelay")).and_then(Value::as_str).and_then(parse_duration).map(|d| now + d);
    let Some(failure) = of_type("QuotaFailure") else { return Vec::new() };
    let mut out = Vec::new();
    for v in failure.get("violations").and_then(Value::as_array).into_iter().flatten() {
        let id = v.get("quotaId").and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(|| v.get("quotaMetric").and_then(Value::as_str)).unwrap_or("quota");
        let total = match v.get("quotaValue") {
            Some(Value::String(s)) => s.trim().parse::<u64>().ok(),
            Some(Value::Number(n)) => n.as_u64(),
            _ => None,
        };
        let daily = id.contains("PerDay");
        // The retry delay Google gives is for the next try, not the day's end: a daily quota has no reset time here.
        let reset = if daily { None } else { delay };
        let unit = if id.contains("Token") { Unit::Tokens } else { Unit::Requests };
        out.push(Limit::new(id, &humanize(id), total, total, unit, reset, Source::Provider));
    }
    out
}

/// "GenerateRequestsPerDayPerProjectPerModel-FreeTier" as "Requests per day (free tier)".
fn humanize(id: &str) -> String {
    let (main, tier) = id.split_once('-').unwrap_or((id, ""));
    let what = if main.contains("InputToken") {
        "Input tokens"
    } else if main.contains("Token") {
        "Tokens"
    } else {
        "Requests"
    };
    let per = if main.contains("PerDay") {
        " per day"
    } else if main.contains("PerMinute") {
        " per minute"
    } else if main.contains("PerMonth") {
        " per month"
    } else {
        ""
    };
    let tier = match tier {
        "" => String::new(),
        t => format!(" ({})", t.chars().fold(String::new(), |mut s, c| {
            if c.is_uppercase() && !s.is_empty() {
                s.push(' ');
            }
            s.extend(c.to_lowercase());
            s
        })),
    };
    format!("{what}{per}{tier}")
}

/// "Rate limit reached for gpt-4 in organization org-x on requests per min (RPM): Limit 3, Used 3, Requested 1. Please try again in 20s."
fn openai_message(error: &Value, header_reset: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Option<Limit> {
    let message = error.get("message").and_then(Value::as_str)?;
    let re = Regex::new(r"(?i)on ([a-z ]+?) \(([a-z]+)\): Limit (\d+), Used (\d+)").expect("a fixed pattern");
    let c = re.captures(message)?;
    let phrase = c[1].to_lowercase();
    let (total, used): (u64, u64) = (c[3].parse().ok()?, c[4].parse().ok()?);
    let unit = if phrase.contains("token") { Unit::Tokens } else { Unit::Requests };
    let name = {
        let long = phrase.replace(" per min", " per minute").replace(" per sec", " per second");
        let mut cs = long.chars();
        cs.next().map(|f| f.to_uppercase().collect::<String>() + cs.as_str()).unwrap_or_default()
    };
    let wait = Regex::new(r"(?i)try again in ([0-9hms.]+)").expect("a fixed pattern").captures(message).and_then(|w| parse_duration(w[1].trim_end_matches('.'))).map(|d| now + d);
    Some(Limit::new(&c[2].to_lowercase(), &name, Some(used), Some(total), unit, wait.or(header_reset), Source::Provider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::Level;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-28T09:00:00Z").unwrap().with_timezone(&Utc)
    }
    fn none() -> Headers {
        Headers::new()
    }

    // FIXTURE: from documentation, not recorded. Gemini API, 429 RESOURCE_EXHAUSTED with both detail types.
    const GEMINI: &str = r#"{"error": {"code": 429, "message": "You exceeded your current quota, please check your plan and billing details.", "status": "RESOURCE_EXHAUSTED",
      "details": [
        {"@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [
          {"quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests", "quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier", "quotaDimensions": {"location": "global", "model": "gemini-2.0-flash"}, "quotaValue": "15"},
          {"quotaMetric": "generativelanguage.googleapis.com/generate_content_free_tier_requests", "quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier", "quotaValue": "1500"}]},
        {"@type": "type.googleapis.com/google.rpc.Help", "links": [{"description": "Learn more", "url": "https://ai.google.dev/gemini-api/docs/rate-limits"}]},
        {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "7s"}]}}"#;

    #[test]
    fn gemini_quota_failure_and_retry_info() {
        let got = limits_from_error(GEMINI, &none(), now());
        assert_eq!(got.len(), 2);
        let minute = &got[0];
        assert_eq!((minute.id.as_str(), minute.name.as_str()), ("GenerateRequestsPerMinutePerProjectPerModel-FreeTier", "Requests per minute (free tier)"));
        assert_eq!((minute.used, minute.total, minute.level, minute.unit), (Some(15), Some(15), Level::Out, Unit::Requests));
        assert_eq!(minute.resets_at.as_deref(), Some("2026-11-28T09:00:07Z"), "now plus the retry delay");
        let day = &got[1];
        assert_eq!((day.name.as_str(), day.total, day.resets_at.clone()), ("Requests per day (free tier)", Some(1500), None), "a daily quota gets no reset from a retry delay");
    }

    #[test]
    fn gemini_through_the_openai_endpoint_comes_in_an_array() {
        let wrapped = format!("[{GEMINI}]");
        assert_eq!(limits_from_error(&wrapped, &none(), now()).len(), 2);
    }

    #[test]
    fn a_token_quota_is_in_tokens() {
        let body = r#"{"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "details": [{"@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [{"quotaId": "GenerateContentInputTokensPerModelPerMinute-FreeTier", "quotaValue": 250000}]}]}}"#;
        let got = limits_from_error(body, &none(), now());
        assert_eq!((got[0].unit, got[0].total, got[0].name.as_str()), (Unit::Tokens, Some(250000), "Input tokens per minute (free tier)"));
    }

    // FIXTURE: from documentation, not recorded. OpenAI, error code "rate_limit_exceeded".
    const OPENAI: &str = r#"{"error": {"message": "Rate limit reached for gpt-4 in organization org-abc on requests per min (RPM): Limit 3, Used 3, Requested 1. Please try again in 20s. Visit https://platform.openai.com/account/rate-limits to learn more.", "type": "requests", "param": null, "code": "rate_limit_exceeded"}}"#;

    #[test]
    fn openai_message_with_limit_used_and_wait() {
        let got = limits_from_error(OPENAI, &none(), now());
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].id.as_str(), got[0].name.as_str(), got[0].used, got[0].total, got[0].level), ("rpm", "Requests per minute", Some(3), Some(3), Level::Out));
        assert_eq!(got[0].resets_at.as_deref(), Some("2026-11-28T09:00:20Z"));
        let tokens = OPENAI.replace("requests per min (RPM): Limit 3, Used 3", "tokens per min (TPM): Limit 40000, Used 39000").replace("20s", "1m30.5s");
        let got = limits_from_error(&tokens, &none(), now());
        assert_eq!((got[0].unit, got[0].used, got[0].level), (Unit::Tokens, Some(39000), Level::NearlyOut));
        assert_eq!(got[0].resets_at.as_deref(), Some("2026-11-28T09:01:30Z"));
    }

    #[test]
    fn insufficient_quota_has_no_numbers() {
        // FIXTURE: from documentation, not recorded.
        let body = r#"{"error": {"message": "You exceeded your current quota, please check your plan and billing details.", "type": "insufficient_quota", "param": null, "code": "insufficient_quota"}}"#;
        let got = limits_from_error(body, &none(), now());
        assert_eq!((got[0].id.as_str(), got[0].used, got[0].total, got[0].level, got[0].resets_at.clone()), ("quota", None, None, Level::Out, None));
    }

    #[test]
    fn anthropic_and_unknown_bodies_give_only_the_retry_after() {
        // FIXTURE: from documentation, not recorded. Anthropic, "Errors": the type is rate_limit_error, the wait is in `retry-after`.
        let body = r#"{"type": "error", "error": {"type": "rate_limit_error", "message": "This request would exceed your organization's rate limit of 50 requests per minute."}}"#;
        let mut h = Headers::new();
        h.insert("retry-after".into(), "12".into());
        let got = limits_from_error(body, &h, now());
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].id.as_str(), got[0].used, got[0].total, got[0].level), ("rate", None, None, Level::Out), "the 50 in the prose is not a parsed number");
        assert_eq!(got[0].resets_at.as_deref(), Some("2026-11-28T09:00:12Z"));
        let got = limits_from_error("<html>Too Many Requests</html>", &h, now());
        assert_eq!((got.len(), got[0].resets_at.as_deref()), (1, Some("2026-11-28T09:00:12Z")), "not JSON: the retry-after alone");
        assert_eq!(limits_from_error("", &none(), now())[0].resets_at, None, "nothing known: out, with no time");
    }

    #[test]
    fn a_response_that_is_not_a_429_gives_only_header_limits() {
        let mut h = Headers::new();
        h.insert("x-ratelimit-limit-requests".into(), "10".into());
        h.insert("x-ratelimit-remaining-requests".into(), "9".into());
        assert_eq!(crate::usage::limits_of_response(200, &h, GEMINI, now()).len(), 1, "a 200 body is not read for quota errors");
        assert_eq!(crate::usage::limits_of_response(500, &h, GEMINI, now()).len(), 1);
        assert_eq!(crate::usage::limits_of_response(429, &h, OPENAI, now()).len(), 2, "the header limit and the one of the body");
    }
}
