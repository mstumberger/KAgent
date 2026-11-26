//! The guards of the client (bots.md section 8): the clock it waits on, how many calls
//! it makes, the addresses and the token it is built with, and the file paths Telegram
//! answers with.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

/// At most this many Bot API calls in a minute.
pub const CALLS_PER_MINUTE: usize = 20;

/// Time as the client uses it: read, waited on, and bounded. The daemon's clock is
/// tokio's; a test's returns at once and can be moved by hand.
#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
    /// Wait for `wait`.
    async fn sleep(&self, wait: Duration);
    /// `false` when `future` did not end within `wait` (it is dropped then).
    async fn within(&self, wait: Duration, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> bool;
}

/// `future` bounded by `wait` on `clock`: `None` when it did not end in time.
pub async fn bounded<T: Send + 'static>(clock: &dyn Clock, wait: Duration, future: impl Future<Output = T> + Send + 'static) -> Option<T> {
    let slot = Arc::new(Mutex::new(None));
    let writer = slot.clone();
    let ended = clock.within(wait, Box::pin(async move { *writer.lock().unwrap() = Some(future.await); })).await;
    ended.then(|| slot.lock().unwrap().take()).flatten()
}

/// The clock of the daemon.
pub struct TokioClock;

#[async_trait]
impl Clock for TokioClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    async fn sleep(&self, wait: Duration) {
        tokio::time::sleep(wait).await;
    }

    async fn within(&self, wait: Duration, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> bool {
        tokio::time::timeout(wait, future).await.is_ok()
    }
}

/// Calls in a sliding window: at most `limit` in any `per`.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    limit: usize,
    per: Duration,
    calls: VecDeque<Instant>,
}

impl Default for RateLimiter {
    /// The limit of the Bot API: 20 calls a minute.
    fn default() -> Self {
        Self::new(CALLS_PER_MINUTE, Duration::from_secs(60))
    }
}

impl RateLimiter {
    pub fn new(limit: usize, per: Duration) -> Self {
        Self { limit, per, calls: VecDeque::new() }
    }

    /// How long before a call at `now` may go: zero when it may at once, and it is
    /// counted then. A call that has to wait is not counted until it is made.
    pub fn reserve(&mut self, now: Instant) -> Duration {
        while self.calls.front().is_some_and(|t| now.saturating_duration_since(*t) >= self.per) {
            self.calls.pop_front();
        }
        if self.calls.len() < self.limit {
            self.calls.push_back(now);
            return Duration::ZERO;
        }
        self.calls.front().map_or(Duration::ZERO, |first| first.checked_add(self.per).map_or(Duration::MAX, |until| until.saturating_duration_since(now)))
    }
}

/// A file path Telegram answers (`getFile`) is used only as a plain relative path: no
/// scheme, no `..`, no leading slash, no backslash, no query; letters, digits, `.`, `_`,
/// `-` and `/` only. So `https://evil.example/x`, `../secret` and `/etc/passwd` are not.
pub fn safe_file_path(path: &str) -> Option<&str> {
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/');
    let part_ok = |part: &str| !part.is_empty() && part != "." && part != "..";
    let ok = !path.is_empty() && !path.starts_with('/') && !path.ends_with('/') && path.bytes().all(plain) && path.split('/').all(part_ok);
    ok.then_some(path)
}

/// A bot token as @BotFather gives it: digits, a colon, then letters, digits, `-` and
/// `_`. It goes into a URL path, so nothing else may be in it. A refusal never repeats
/// the token.
pub fn check_token(token: &str) -> Result<(), String> {
    const SHAPE: &str = "a bot token is <digits>:<key>, as @BotFather gives it (letters, digits, '-' and '_')";
    let (id, key) = token.split_once(':').ok_or_else(|| SHAPE.to_string())?;
    let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_');
    if (1..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) && (20..=80).contains(&key.len()) && key.bytes().all(plain) {
        return Ok(());
    }
    Err(SHAPE.to_string())
}

/// `https://` anywhere, or plain `http` on loopback (a test's fake): nothing else.
pub fn check_base(base: &str) -> Result<(), String> {
    let ok = (|| {
        let url = reqwest::Url::parse(base).ok()?;
        let loopback = matches!(url.host_str()?, "127.0.0.1" | "localhost" | "[::1]" | "::1");
        Some(url.username().is_empty() && url.password().is_none() && (url.scheme() == "https" || (url.scheme() == "http" && loopback)))
    })()
    .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(format!("refused base address '{base}': only https, or http on loopback"))
    }
}

#[cfg(test)]
pub(crate) use fake::FakeClock;

#[cfg(test)]
mod fake {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use super::*;

    /// The clock of a test: it never waits. A sleep moves the time and is recorded; with
    /// `expire` on, a bounded call ends at once, as a timed-out one.
    pub struct FakeClock {
        now: Mutex<Instant>,
        sleeps: Mutex<Vec<Duration>>,
        timeouts: Mutex<Vec<Duration>>,
        expire: AtomicBool,
    }

    impl FakeClock {
        pub fn new() -> Self {
            Self { now: Mutex::new(Instant::now()), sleeps: Mutex::new(Vec::new()), timeouts: Mutex::new(Vec::new()), expire: AtomicBool::new(false) }
        }

        /// Move the time forward.
        pub fn advance(&self, by: Duration) {
            *self.now.lock().unwrap() += by;
        }

        /// How long the client asked to wait, each time.
        pub fn slept(&self) -> Vec<Duration> {
            self.sleeps.lock().unwrap().clone()
        }

        /// Within what each bounded call had to end.
        pub fn timeouts(&self) -> Vec<Duration> {
            self.timeouts.lock().unwrap().clone()
        }

        /// From now on every bounded call ends at once, as a timed-out one.
        pub fn expire_timeouts(&self, yes: bool) {
            self.expire.store(yes, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }

        async fn sleep(&self, wait: Duration) {
            self.sleeps.lock().unwrap().push(wait);
            self.advance(wait);
        }

        async fn within(&self, wait: Duration, future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> bool {
            self.timeouts.lock().unwrap().push(wait);
            if self.expire.load(Ordering::SeqCst) {
                return false;
            }
            future.await;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    #[test]
    fn twenty_calls_a_minute_then_wait() {
        let t0 = Instant::now();
        let mut limiter = RateLimiter::new(20, MINUTE);
        for sec in 0..20 {
            assert_eq!(limiter.reserve(t0 + Duration::from_secs(sec)), Duration::ZERO, "call {} goes at once", sec + 1);
        }
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(19)), Duration::from_secs(41), "the 21st waits until the first call is a minute old");
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(59)), Duration::from_secs(1));
        assert_eq!(limiter.reserve(t0 + MINUTE), Duration::ZERO, "a waiting call is not counted: the slot is free at the minute");
        // The window slides: at 61 s the calls at 0 and 1 are gone.
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(61)), Duration::ZERO);
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(61)), Duration::from_secs(1), "the oldest call left is at 2 s: its slot frees at 62 s");
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(62)), Duration::ZERO);
    }

    #[test]
    fn the_window_is_of_the_last_minute_not_of_a_fixed_one() {
        let t0 = Instant::now();
        let mut limiter = RateLimiter::new(3, MINUTE);
        for sec in [0, 10, 20] {
            assert_eq!(limiter.reserve(t0 + Duration::from_secs(sec)), Duration::ZERO);
        }
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(30)), Duration::from_secs(30), "full until the call at 0 is a minute old");
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(61)), Duration::ZERO);
        assert_eq!(limiter.reserve(t0 + Duration::from_secs(62)), Duration::from_secs(8), "the call at 10 frees the next slot");
    }

    #[test]
    fn the_turn_limit_is_the_same_limiter() {
        let t0 = Instant::now();
        let mut turns = RateLimiter::new(3, MINUTE);
        for n in 0..3 {
            assert_eq!(turns.reserve(t0), Duration::ZERO, "turn {}", n + 1);
        }
        assert_eq!(turns.reserve(t0 + Duration::from_secs(5)), Duration::from_secs(55), "the fourth message in a minute waits");
    }

    #[test]
    fn file_paths() {
        for path in ["file_10.oga", "voice/file_10.oga", "documents/KAgent-bot/file_1.pdf", "a-b_c.d/e"] {
            assert_eq!(safe_file_path(path), Some(path), "{path}");
        }
        for path in [
            "https://evil.example/x",
            "HTTP://evil.example/x",
            "//evil.example/x",
            "/etc/passwd",
            "../secret",
            "voice/../secret",
            "a/./b",
            "a//b",
            "a/",
            "a\\b",
            "voice/file_1.oga?x=1",
            "voice/file_1.oga#x",
            "voice/file 1.oga",
            "voice/file\n1.oga",
            "file\x00.oga",
            "файл.oga",
            "",
            ".",
            "..",
        ] {
            assert_eq!(safe_file_path(path), None, "{path:?} is not a plain relative path");
        }
    }
}
