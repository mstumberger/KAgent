//! What the owner is told when a provider fails and when it works again (notices.md
//! section 1): the class of an error, the key of its notice, and the providers that
//! failed and have not answered since.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::protocol::AgentEvent;

/// Why a provider could not answer. The key of a notice is `provider:<id>:<class>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Quota,
    Key,
    Busy,
    Down,
    Other,
}

impl ErrorClass {
    /// Quota is named first (a 429 that says "invalid api key" is still a quota), then a
    /// rejected key, then a busy server, then a server that cannot be reached.
    pub fn of(error: &str) -> Self {
        let e = error.to_lowercase();
        let has = |words: &[&str]| words.iter().any(|w| e.contains(w));
        if has(&["429", "quota", "rate limit", "resource_exhausted", "resource exhausted"]) {
            Self::Quota
        } else if has(&["key", "unauthenticated", "permission_denied", "401 unauthorized", "403 forbidden"]) {
            Self::Key
        } else if has(&["503", "overloaded", "high demand", "unavailable", "busy"]) {
            Self::Busy
        } else if has(&["connection refused", "connection reset", "unreachable", "timed out", "dns error", "error sending request", "connect error"]) {
            Self::Down
        } else {
            Self::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Key => "key",
            Self::Busy => "busy",
            Self::Down => "down",
            Self::Other => "other",
        }
    }

    /// What the model "does", for the sentence of a notice.
    pub fn why(self) -> &'static str {
        match self {
            Self::Quota => "is out of quota",
            Self::Key => "rejected the API key (Providers)",
            Self::Busy => "is busy",
            Self::Down => "cannot be reached",
            Self::Other => "failed",
        }
    }
}

/// The provider of a model reference `provider::model`; a reference without `::` is its own provider.
pub fn provider_of(model_ref: &str) -> &str {
    model_ref.split("::").next().unwrap_or(model_ref)
}

pub fn provider_key(provider: &str, class: ErrorClass) -> String {
    format!("provider:{provider}:{}", class.name())
}

/// The models whose error notice is open, by model reference: the key of that notice.
#[derive(Default)]
pub struct Failing(Mutex<HashMap<String, String>>);

impl Failing {
    pub fn failed(&self, model_ref: &str, key: String) {
        self.0.lock().unwrap().insert(model_ref.to_string(), key);
    }

    /// The provider answered: the recovery of its last error, once. `None` when it had not failed.
    pub fn answered(&self, model_ref: &str) -> Option<AgentEvent> {
        let key = self.0.lock().unwrap().remove(model_ref)?;
        Some(AgentEvent::recovery(format!("{model_ref} answers again"), key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_of_an_error_and_its_key() {
        for (error, class) in [
            ("provider error 429: resource exhausted", ErrorClass::Quota),
            ("429: invalid api key", ErrorClass::Quota),
            ("401 Unauthorized: invalid API key provided", ErrorClass::Key),
            ("503 Service Unavailable", ErrorClass::Busy),
            ("The model is overloaded", ErrorClass::Busy),
            ("error sending request: Connection refused", ErrorClass::Down),
            ("something odd", ErrorClass::Other),
        ] {
            assert_eq!(ErrorClass::of(error), class, "{error}");
        }
        assert_eq!(provider_key(provider_of("gemini::gemini-3.8-flash"), ErrorClass::Quota), "provider:gemini:quota");
    }

    #[test]
    fn a_recovery_has_the_key_of_the_error_and_comes_once() {
        let failing = Failing::default();
        assert!(failing.answered("gemini::a").is_none(), "no recovery without a previous error");
        failing.failed("gemini::a", "provider:gemini:quota".into());
        assert!(failing.answered("gemini::b").is_none(), "another model of the provider is not the one that failed");
        let Some(AgentEvent::Notice { key, kind, level, .. }) = failing.answered("gemini::a") else { panic!("recovery") };
        assert_eq!((key.as_deref(), kind.as_deref(), level.as_str()), (Some("provider:gemini:quota"), Some("recovery"), "info"));
        assert!(failing.answered("gemini::a").is_none(), "once");
    }

    #[test]
    fn the_event_carries_its_new_fields_only_when_it_has_them() {
        let plain = serde_json::to_value(AgentEvent::notice("info", "hi")).unwrap();
        assert_eq!(plain, serde_json::json!({"type": "notice", "level": "info", "text": "hi", "kind": "info"}));
        let keyed = serde_json::to_value(AgentEvent::keyed_notice("warn", "x", "hook:a").in_session("s1")).unwrap();
        assert_eq!(keyed, serde_json::json!({"type": "notice", "level": "warn", "text": "x", "key": "hook:a", "sessionId": "s1", "kind": "error"}));
    }
}
