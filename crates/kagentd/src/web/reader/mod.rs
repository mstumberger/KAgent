//! The reader (web.md sections 8 and 9): KAgent's own headless browser for reading
//! pages. Not browser attach and not the `browser` tool's window: it only reads. It finds
//! a Chromium-family browser, starts it on its own profile (`<data>/reader-profile`),
//! reads one page at a time with the daemon's CDP client, and closes the browser after
//! two idle minutes and at daemon shutdown.

mod browser;
pub mod challenge;
pub mod config;
mod drive;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;

use super::guard::Egress;
use super::page_reader::{PageReader, ReaderError, Rendered};
use browser::Running;

/// The browser is closed after this long without a read (web.md section 8).
pub const IDLE: Duration = Duration::from_secs(120);
/// One read, challenge wait included.
const READ_LIMIT: Duration = Duration::from_secs(45);

/// Where the browser to run is found: the daemon's discovery, or a fixed one in a test.
pub type Finder = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct Reader {
    data: PathBuf,
    finder: Finder,
    idle: Duration,
    egress: Arc<dyn Egress>,
    /// The browser, when it runs. The lock is the reader's "one page at a time".
    state: tokio::sync::Mutex<Option<Running>>,
    /// Counts the reads: an idle timer closes the browser only if no read came after it.
    generation: AtomicU64,
    /// How long a challenge may take to clear, in milliseconds (a test shortens it).
    challenge_wait_ms: AtomicU64,
    me: Weak<Reader>,
}

impl Reader {
    pub fn new(data: &Path, finder: Finder, idle: Duration, egress: Arc<dyn Egress>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            data: data.to_path_buf(),
            finder,
            idle,
            egress,
            state: tokio::sync::Mutex::new(None),
            generation: AtomicU64::new(0),
            challenge_wait_ms: AtomicU64::new(drive::CHALLENGE_WAIT.as_millis() as u64),
            me: me.clone(),
        })
    }

    /// The reader as the daemon runs it: the browser of the `browser` tool's list, two idle minutes.
    pub fn for_daemon(data: &Path, egress: Arc<dyn Egress>) -> Arc<Self> {
        Self::new(data, Arc::new(crate::browser::find_chromium), IDLE, egress)
    }

    #[cfg(test)]
    pub fn set_challenge_wait(&self, wait: Duration) {
        self.challenge_wait_ms.store(wait.as_millis() as u64, Ordering::SeqCst);
    }

    pub fn profile(&self) -> PathBuf {
        browser::profile_dir(&self.data)
    }

    /// The browser the reader would run, if any.
    pub fn browser(&self) -> Option<String> {
        (self.finder)()
    }

    /// Whether the browser runs now (a read is on, or it waits for its idle time).
    pub async fn running(&self) -> bool {
        match self.state.try_lock() {
            Ok(mut state) => state.as_mut().is_some_and(Running::alive),
            Err(_) => true,
        }
    }

    /// Close the browser: the daemon is stopping.
    pub async fn shutdown(&self) {
        let running = self.state.lock().await.take();
        if let Some(running) = running {
            running.close().await;
        }
    }

    /// The idle timer of one read: after `idle`, close the browser unless a read came since.
    fn arm_idle(&self, generation: u64) {
        let me = self.me.clone();
        let idle = self.idle;
        tokio::spawn(async move {
            tokio::time::sleep(idle).await;
            let Some(reader) = me.upgrade() else { return };
            if reader.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            // A read in progress holds the lock: the browser is not idle.
            let running = match reader.state.try_lock() {
                Ok(mut state) if reader.generation.load(Ordering::SeqCst) == generation => state.take(),
                _ => None,
            };
            if let Some(running) = running {
                running.close().await;
            }
        });
    }
}

#[async_trait]
impl PageReader for Reader {
    async fn render(&self, url: &str) -> Result<Rendered, ReaderError> {
        if let Err(refusal) = self.egress.admit(url).await {
            return Err(ReaderError::Refused { refusal, redirected: false });
        }
        let bin = (self.finder)().ok_or(ReaderError::NoBrowser)?;
        let mut state = self.state.lock().await;
        self.generation.fetch_add(1, Ordering::SeqCst);
        if !state.as_mut().is_some_and(Running::alive) {
            *state = Some(browser::start_headless(&bin, &self.profile()).await.map_err(ReaderError::Failed)?);
        }
        let port = state.as_ref().map(|r| r.port).unwrap_or_default();
        let result = tokio::time::timeout(READ_LIMIT, drive::read(port, url, self.egress.clone(), Duration::from_millis(self.challenge_wait_ms.load(Ordering::SeqCst)))).await.unwrap_or(Err(ReaderError::Timeout));
        if !state.as_mut().is_some_and(Running::alive) {
            *state = None;
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.arm_idle(generation);
        result.map(|read| Rendered { url: read.url, html: read.html })
    }
}

#[cfg(test)]
mod tests;

