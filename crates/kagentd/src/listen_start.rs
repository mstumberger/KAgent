//! Listen on start (voice.md section 14): after login the daemon opens its window with a one-time
//! marker and the page starts a live voice session by itself. The marker is what makes the start the
//! daemon's own decision: an address that merely holds a flag never starts the microphone.

mod greeting;
mod http;
pub mod login;
pub mod start;

pub use greeting::{greeting_now, spoken_turn};
pub use http::listen_start as route;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a marker is good for, once.
pub const MARKER_TTL: Duration = Duration::from_secs(120);
/// After the marker was used, the first live session that connects within this time is the one started at login.
pub const CLAIM_WINDOW: Duration = Duration::from_secs(60);

/// What a start at login is to do, fixed when the marker is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Say the greeting at the start of the session (`startup.greeting.spoken`).
    pub greeting: bool,
    /// End the session after this long without speech of the owner.
    pub idle: Duration,
}

struct Marker {
    value: String,
    until: Instant,
    plan: Plan,
}

struct Armed {
    until: Instant,
    plan: Plan,
}

#[derive(Default)]
struct Inner {
    marker: Option<Marker>,
    armed: Option<Armed>,
    at_login: bool,
}

/// The marker, and the session started at login. In memory only.
#[derive(Default)]
pub struct ListenStart {
    inner: Mutex<Inner>,
}

/// The live session started at login, for as long as it is open.
pub struct LoginSession {
    owner: Arc<ListenStart>,
    pub plan: Plan,
}

impl Drop for LoginSession {
    fn drop(&mut self) {
        self.owner.inner.lock().unwrap().at_login = false;
    }
}

impl ListenStart {
    /// A new marker (the previous one stops being valid): 64 hex characters from the system's randomness.
    pub fn issue(&self, plan: Plan, now: Instant) -> String {
        let value = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
        self.inner.lock().unwrap().marker = Some(Marker { value: value.clone(), until: now + MARKER_TTL, plan });
        value
    }

    /// The page presents a marker. True once, for the marker made by `issue`, while it is valid: a wrong,
    /// used or old one is false and does not use up the right one.
    pub fn consume(&self, presented: &str, now: Instant) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.marker.as_ref().is_some_and(|m| now >= m.until) {
            inner.marker = None;
        }
        let matches = inner.marker.as_ref().is_some_and(|m| crate::server::token_eq(presented, &m.value));
        if !matches {
            return false;
        }
        let plan = inner.marker.take().map(|m| m.plan).unwrap_or(Plan { greeting: false, idle: Duration::from_secs(600) });
        inner.armed = Some(Armed { until: now + CLAIM_WINDOW, plan });
        true
    }

    /// A live session connects: it is the one started at login when a marker was used a moment ago.
    pub fn claim(self: &Arc<Self>, now: Instant) -> Option<LoginSession> {
        let mut inner = self.inner.lock().unwrap();
        let armed = inner.armed.take().filter(|a| now < a.until)?;
        inner.at_login = true;
        Some(LoginSession { owner: self.clone(), plan: armed.plan })
    }

    /// A session started at login is open (the tray's tooltip says so).
    pub fn at_login(&self) -> bool {
        self.inner.lock().unwrap().at_login
    }
}

/// A session with no speech of the owner for `idle` ends by itself.
pub fn idle_expired(since_speech: Duration, idle: Duration) -> bool {
    since_speech >= idle
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: Plan = Plan { greeting: true, idle: Duration::from_secs(600) };

    #[test]
    fn a_marker_is_made_by_the_daemon_and_works_once() {
        let l = ListenStart::default();
        let t0 = Instant::now();
        let m = l.issue(PLAN, t0);
        assert_eq!(m.len(), 64);
        assert!(m.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(m, l.issue(PLAN, t0), "every marker is new, and the older one is gone");
        let m = l.issue(PLAN, t0);
        assert!(l.consume(&m, t0 + Duration::from_secs(3)));
        assert!(!l.consume(&m, t0 + Duration::from_secs(4)), "used once");
    }

    #[test]
    fn what_is_not_the_marker_starts_nothing_and_does_not_burn_it() {
        let l = ListenStart::default();
        let t0 = Instant::now();
        assert!(!l.consume("", t0), "none was made");
        assert!(!l.consume("anything", t0));
        let m = l.issue(PLAN, t0);
        for bad in ["", "1", "true", "autostart", "listen=1", &m[..63], &format!("{m}0"), &m.to_uppercase(), "00000000000000000000000000000000000000000000000000000000000000000"] {
            assert!(!l.consume(bad, t0 + Duration::from_secs(1)), "{bad:?}");
        }
        assert!(l.claim_for_test(t0).is_none(), "nothing was started by those");
        assert!(l.consume(&m, t0 + Duration::from_secs(2)), "the right one still works");
    }

    #[test]
    fn a_marker_is_good_for_two_minutes() {
        let l = ListenStart::default();
        let t0 = Instant::now();
        let m = l.issue(PLAN, t0);
        assert!(!l.consume(&m, t0 + MARKER_TTL), "at two minutes it is over");
        let m = l.issue(PLAN, t0);
        assert!(l.consume(&m, t0 + MARKER_TTL - Duration::from_millis(1)));
        assert!(!l.consume(&m, t0), "and used");
    }

    #[test]
    fn the_session_started_at_login_is_the_first_one_after_the_marker_was_used() {
        let l = Arc::new(ListenStart::default());
        let t0 = Instant::now();
        assert!(l.claim(t0).is_none(), "a session nobody asked for");
        let m = l.issue(PLAN, t0);
        assert!(l.claim(t0).is_none(), "a marker that was only made, not used");
        assert!(l.consume(&m, t0));
        assert!(!l.at_login());
        let session = l.claim(t0 + Duration::from_secs(5)).expect("the first one");
        assert_eq!(session.plan, PLAN);
        assert!(l.at_login(), "the tray says so while it is open");
        assert!(l.claim(t0 + Duration::from_secs(6)).is_none(), "only one");
        drop(session);
        assert!(!l.at_login());
        // Used, but no session came for a minute: a later one is the owner's own.
        let m = l.issue(PLAN, t0);
        assert!(l.consume(&m, t0));
        assert!(l.claim(t0 + CLAIM_WINDOW).is_none());
    }

    #[test]
    fn the_idle_rule() {
        let ten = Duration::from_secs(600);
        assert!(!idle_expired(Duration::from_secs(599), ten));
        assert!(idle_expired(ten, ten));
        assert!(idle_expired(Duration::from_secs(3600), ten));
        assert!(!idle_expired(Duration::ZERO, ten));
    }

    impl ListenStart {
        fn claim_for_test(&self, now: Instant) -> Option<Plan> {
            self.inner.lock().unwrap().armed.take().filter(|a| now < a.until).map(|a| a.plan)
        }
    }
}
