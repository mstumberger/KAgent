//! The machine snapshot of the system prompt (`prompt.rs`): taken at start, then taken
//! again in the background when a turn finds it older than ten minutes. That turn uses
//! the snapshot it has; the next one gets the new one. At most one refresh runs at a time.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use serde_json::Value;

/// How old the snapshot may get before a turn asks for a new one.
pub const MAX_AGE: Duration = Duration::from_secs(600);

type Take = Arc<dyn Fn() -> BoxFuture<'static, Value> + Send + Sync>;

pub struct MachineSnapshot {
    current: RwLock<(Instant, Value)>,
    refreshing: Arc<AtomicBool>,
    max_age: Duration,
    take: Take,
}

impl MachineSnapshot {
    /// The daemon's: `system_snapshot`, now and every ten minutes at most.
    pub async fn system() -> Arc<Self> {
        let take: Take = Arc::new(|| Box::pin(kagent_core::tools::sys::system_snapshot()));
        Self::with(take, MAX_AGE).await
    }

    pub async fn with(take: Take, max_age: Duration) -> Arc<Self> {
        let first = take().await;
        Arc::new(Self { current: RwLock::new((Instant::now(), first)), refreshing: Arc::new(AtomicBool::new(false)), max_age, take })
    }

    /// Make the snapshot `by` older (tests: no clock to wait for).
    #[cfg(test)]
    fn age(&self, by: Duration) {
        let mut current = self.current.write().unwrap();
        current.0 = current.0.checked_sub(by).expect("the clock is far enough from its start");
    }

    /// The snapshot as it is; one that is too old is taken again in the background.
    pub fn current(self: &Arc<Self>) -> Value {
        let (taken, value) = self.current.read().unwrap().clone();
        if taken.elapsed() >= self.max_age && !self.refreshing.swap(true, Ordering::AcqRel) {
            let this = self.clone();
            match tokio::runtime::Handle::try_current() {
                Ok(rt) => {
                    rt.spawn(async move {
                        let next = (this.take)().await;
                        *this.current.write().unwrap() = (Instant::now(), next);
                        this.refreshing.store(false, Ordering::Release);
                    });
                }
                Err(_) => self.refreshing.store(false, Ordering::Release),
            }
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn counting() -> (Take, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        let take: Take = Arc::new(move || {
            let k = c.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { serde_json::json!({ "round": k }) })
        });
        (take, n)
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn the_snapshot_is_taken_again_at_most_every_ten_minutes() {
        let (take, n) = counting();
        let snap = MachineSnapshot::with(take, MAX_AGE).await;
        assert_eq!(n.load(Ordering::SeqCst), 1, "taken at start");
        for _ in 0..5 {
            assert_eq!(snap.current()["round"], 0);
        }
        settle().await;
        assert_eq!(n.load(Ordering::SeqCst), 1, "fresh: not taken again");

        snap.age(MAX_AGE);
        // The turn that finds it old still gets the old one; a second turn meanwhile asks for nothing more.
        assert_eq!(snap.current()["round"], 0);
        assert_eq!(snap.current()["round"], 0);
        settle().await;
        assert_eq!(n.load(Ordering::SeqCst), 2, "one refresh");
        assert_eq!(snap.current()["round"], 1, "the next turn gets the new one");
        snap.age(MAX_AGE - Duration::from_secs(1));
        snap.current();
        settle().await;
        assert_eq!(n.load(Ordering::SeqCst), 2, "not before ten minutes have passed again");
    }
}
