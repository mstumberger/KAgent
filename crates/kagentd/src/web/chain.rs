//! The state behind the chain of search engines (web.md section 3.1): the cache of
//! answers, the slot of each engine (pacing with a queue), the cooldowns that double,
//! and the count of searches of a session (the burst refusal). The clock is a trait: the
//! tests use one whose `sleep` moves it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use super::search::Hit;
use super::settings::Engine;

/// A query answered is kept this long, and this many (web.md section 3.1).
pub const CACHE_TTL_MS: u64 = 15 * 60_000;
pub const CACHE_ENTRIES: usize = 50;
/// A request waits at most this long for the slot of its engine; more is "busy".
pub const MAX_QUEUE_MS: u64 = 20_000;
/// The first cooldown of an engine, and the longest.
pub const COOLDOWN_MS: u64 = 5 * 60_000;
pub const COOLDOWN_MAX_MS: u64 = 60 * 60_000;
/// More than this many searches of one session within the window are refused.
pub const BURST_SEARCHES: usize = 3;
pub const BURST_WINDOW_MS: u64 = 10_000;

/// Time as the chain needs it: milliseconds since some start, and a sleep.
#[async_trait]
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
    async fn sleep(&self, ms: u64);
}

/// The real clock.
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

#[async_trait]
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }

    async fn sleep(&self, ms: u64) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

/// One request at most every this long to an engine (web.md section 3.1).
pub fn interval_ms(engine: Engine) -> u64 {
    match engine {
        Engine::DuckDuckGo => 3_000,
        Engine::Reader => 5_000,
        Engine::Brave => 1_000,
        Engine::Searxng | Engine::None => 0,
    }
}

/// A query as the cache knows it: trimmed, lowercased, whitespace collapsed.
pub fn normalise(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[derive(Default)]
struct Slot {
    /// The next moment a request to the engine may start.
    next_free: u64,
    cooldown_until: u64,
    /// Failures in a row: the cooldown doubles with it.
    failures: u32,
}

#[derive(Default)]
struct Inner {
    /// Newest first: (query, when, engine, hits).
    cache: Vec<(String, u64, Engine, Vec<Hit>)>,
    slots: HashMap<Engine, Slot>,
    searches: HashMap<String, Vec<u64>>,
}

/// The chain's memory, one per `Web`.
pub struct Chain {
    pub clock: Arc<dyn Clock>,
    inner: Mutex<Inner>,
}

impl Default for Chain {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemClock::default()))
    }
}

impl Chain {
    pub fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self { clock, inner: Mutex::new(Inner::default()) }
    }

    /// The answer kept for a normalised query, and which engine gave it.
    pub fn cached(&self, key: &str) -> Option<(Engine, Vec<Hit>)> {
        let now = self.clock.now_ms();
        let mut inner = self.inner.lock().unwrap();
        let at = inner.cache.iter().position(|(k, ..)| k == key)?;
        if now.saturating_sub(inner.cache[at].1) > CACHE_TTL_MS {
            inner.cache.remove(at);
            return None;
        }
        let entry = inner.cache.remove(at);
        let out = (entry.2, entry.3.clone());
        inner.cache.insert(0, entry);
        Some(out)
    }

    pub fn keep(&self, key: String, engine: Engine, hits: Vec<Hit>) {
        let now = self.clock.now_ms();
        let mut inner = self.inner.lock().unwrap();
        inner.cache.retain(|(k, when, ..)| *k != key && now.saturating_sub(*when) <= CACHE_TTL_MS);
        inner.cache.insert(0, (key, now, engine, hits));
        inner.cache.truncate(CACHE_ENTRIES);
    }

    /// Whether this session may search now. A refused search is not counted.
    pub fn allow_search(&self, session: &str) -> bool {
        let now = self.clock.now_ms();
        let mut inner = self.inner.lock().unwrap();
        let times = inner.searches.entry(session.to_string()).or_default();
        times.retain(|t| now.saturating_sub(*t) < BURST_WINDOW_MS);
        if times.len() >= BURST_SEARCHES {
            return false;
        }
        times.push(now);
        inner.searches.retain(|_, v| !v.is_empty());
        true
    }

    /// The minutes left of a cooldown (rounded up), when the engine is cooling.
    pub fn cooling(&self, engine: Engine) -> Option<u64> {
        let now = self.clock.now_ms();
        let inner = self.inner.lock().unwrap();
        let until = inner.slots.get(&engine)?.cooldown_until;
        (until > now).then(|| (until - now).div_ceil(60_000))
    }

    /// Take the next slot of the engine: how long to wait for it, or `None` when that is
    /// over the queue's limit (the slot is then not taken).
    pub fn reserve(&self, engine: Engine) -> Option<u64> {
        let now = self.clock.now_ms();
        let mut inner = self.inner.lock().unwrap();
        let slot = inner.slots.entry(engine).or_default();
        let start = slot.next_free.max(now);
        if start - now > MAX_QUEUE_MS {
            return None;
        }
        slot.next_free = start + interval_ms(engine);
        Some(start - now)
    }

    /// The engine failed: a cooldown of 5 minutes, doubled for each failure in a row, at most an hour.
    pub fn failed(&self, engine: Engine) {
        let now = self.clock.now_ms();
        let mut inner = self.inner.lock().unwrap();
        let slot = inner.slots.entry(engine).or_default();
        let length = (COOLDOWN_MS << slot.failures.min(8)).min(COOLDOWN_MAX_MS);
        slot.cooldown_until = now + length;
        slot.failures += 1;
    }

    pub fn answered(&self, engine: Engine) {
        let mut inner = self.inner.lock().unwrap();
        let slot = inner.slots.entry(engine).or_default();
        slot.failures = 0;
        slot.cooldown_until = 0;
    }
}

/// A clock a test moves: `sleep` advances it at once.
#[cfg(test)]
#[derive(Default)]
pub struct FakeClock(pub std::sync::atomic::AtomicU64);

#[cfg(test)]
impl FakeClock {
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
#[async_trait]
impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn sleep(&self, ms: u64) {
        self.advance(ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> (Chain, Arc<FakeClock>) {
        let clock = Arc::new(FakeClock::default());
        (Chain::with_clock(clock.clone()), clock)
    }

    #[test]
    fn queries_are_normalised() {
        assert_eq!(normalise("  Linux   KERNEL\n"), "linux kernel");
    }

    #[test]
    fn slots_are_three_seconds_apart_and_a_long_queue_is_busy() {
        let (chain, clock) = chain();
        let waits: Vec<_> = (0..5).map(|_| chain.reserve(Engine::DuckDuckGo)).collect();
        assert_eq!(waits, [Some(0), Some(3000), Some(6000), Some(9000), Some(12000)]);
        for _ in 0..3 {
            chain.reserve(Engine::DuckDuckGo);
        }
        assert_eq!(chain.reserve(Engine::DuckDuckGo), None, "24 s of queue is over the limit of 20 s");
        assert_eq!(chain.reserve(Engine::Searxng), Some(0), "another engine has its own queue");
        clock.advance(100_000);
        assert_eq!(chain.reserve(Engine::DuckDuckGo), Some(0), "the queue is gone with time");
    }

    #[test]
    fn a_cooldown_doubles_up_to_an_hour_ends_and_a_success_forgives() {
        let (chain, clock) = chain();
        assert_eq!(chain.cooling(Engine::Brave), None);
        let mut lengths = Vec::new();
        for _ in 0..6 {
            chain.failed(Engine::Brave);
            let minutes = chain.cooling(Engine::Brave).unwrap();
            lengths.push(minutes);
            clock.advance(minutes * 60_000);
            assert_eq!(chain.cooling(Engine::Brave), None, "it ends");
        }
        assert_eq!(lengths, [5, 10, 20, 40, 60, 60]);
        chain.failed(Engine::Brave);
        chain.answered(Engine::Brave);
        assert_eq!(chain.cooling(Engine::Brave), None, "a success ends it");
        chain.failed(Engine::Brave);
        assert_eq!(chain.cooling(Engine::Brave), Some(5), "and the next one starts again at 5");
    }

    #[test]
    fn the_cache_keeps_fifteen_minutes_and_fifty_entries() {
        let (chain, clock) = chain();
        chain.keep("a".into(), Engine::Brave, vec![Hit::default()]);
        assert_eq!(chain.cached("a").map(|c| c.0), Some(Engine::Brave));
        clock.advance(CACHE_TTL_MS + 1);
        assert!(chain.cached("a").is_none());
        for n in 0..60 {
            chain.keep(format!("q{n}"), Engine::Searxng, vec![]);
        }
        assert!(chain.cached("q0").is_none() && chain.cached("q59").is_some());
    }

    #[test]
    fn the_fourth_search_in_ten_seconds_is_refused() {
        let (chain, clock) = chain();
        assert!([(); 3].iter().all(|_| chain.allow_search("s1")));
        assert!(!chain.allow_search("s1"), "the fourth");
        assert!(chain.allow_search("s2"), "another session");
        clock.advance(BURST_WINDOW_MS);
        assert!(chain.allow_search("s1"), "ten seconds later");
    }
}
