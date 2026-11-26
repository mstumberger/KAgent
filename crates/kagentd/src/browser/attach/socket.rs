//! The one socket to the browser, and when it is open: a command and its answer by
//! message id, the events of a tab by session id, a ping every 10 s, closed after ten
//! idle minutes and opened again by the next call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message as WsMsg;

use super::profile::parse_devtools_active_port;
use super::{BrowserSession, Failure, Inner, CONNECT_TIMEOUT};

const PING_TIMEOUT: Duration = Duration::from_secs(5);

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A command that got no answer in time.
#[derive(Debug)]
struct TimedOut;

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "timed out")
    }
}

impl std::error::Error for TimedOut {}

#[derive(Default)]
struct Shared {
    /// The running calls, by message id.
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    /// The attached tabs that listen for events, by session id.
    listeners: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
}

/// One open socket to the browser.
pub(super) struct Conn {
    tx: tokio::sync::Mutex<futures::stream::SplitSink<Ws, WsMsg>>,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    /// Set once the socket is gone: the next use opens a new one.
    closed: Arc<AtomicBool>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Conn {
    /// Wrap an open WebSocket; its reader starts at once.
    fn open(ws: Ws) -> Arc<Self> {
        let (tx, rx) = ws.split();
        let conn = Arc::new(Self { tx: tokio::sync::Mutex::new(tx), shared: Arc::default(), next_id: AtomicU64::new(1), closed: Arc::new(AtomicBool::new(false)), reader: Mutex::new(None) });
        let reader = tokio::spawn(read_loop(rx, conn.shared.clone(), conn.closed.clone()));
        *conn.reader.lock().unwrap() = Some(reader);
        conn
    }

    pub(super) fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    /// One command: on the browser (`session` None) or in one tab (its session id).
    pub(super) async fn request(&self, session: Option<&str>, method: &str, params: Value, limit: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply, rx) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(id, reply);
        let mut frame = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            frame["sessionId"] = json!(s);
        }
        let sent = {
            let mut tx = self.tx.lock().await;
            tx.send(WsMsg::Text(frame.to_string().into())).await
        };
        if let Err(e) = sent {
            self.shared.pending.lock().unwrap().remove(&id);
            return Err(self.gone(e));
        }
        match tokio::time::timeout(limit, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!("{method}: {e}")),
            Ok(Err(_)) => Err(self.gone(anyhow!("the browser closed it"))),
            Err(_) => Err(anyhow::Error::new(TimedOut).context(format!("{method}: no answer in {} s", limit.as_secs()))),
        }
    }

    fn gone(&self, e: impl std::fmt::Display) -> anyhow::Error {
        self.closed.store(true, Ordering::Relaxed);
        anyhow!("the connection to the browser is gone ({e})")
    }

    /// The events of a tab's session, from now until it is forgotten.
    pub(super) fn listen(&self, session: &str) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.shared.listeners.lock().unwrap().insert(session.to_string(), tx);
        rx
    }

    pub(super) fn forget(&self, session: &str) {
        self.shared.listeners.lock().unwrap().remove(session);
    }

    /// Close the socket the polite way (a close frame), then make sure nothing waits on it.
    pub(super) async fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        if let Ok(mut tx) = tokio::time::timeout(Duration::from_secs(2), self.tx.lock()).await {
            let _ = tx.close().await;
        }
        self.shutdown();
    }

    pub(super) fn shutdown(&self) {
        self.closed.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.lock().unwrap().take() {
            reader.abort();
        }
        self.shared.pending.lock().unwrap().clear();
    }
}

/// Answers go to the call that asked; events go to the tab with that session id.
async fn read_loop(mut rx: futures::stream::SplitStream<Ws>, shared: Arc<Shared>, closed: Arc<AtomicBool>) {
    while let Some(Ok(msg)) = rx.next().await {
        let WsMsg::Text(text) = msg else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            let Some(reply) = shared.pending.lock().unwrap().remove(&id) else { continue };
            let answer = match v.get("error") {
                Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = reply.send(answer);
        } else if let Some(tx) = v.get("sessionId").and_then(Value::as_str).and_then(|sid| shared.listeners.lock().unwrap().get(sid).cloned()) {
            let _ = tx.send(v);
        }
    }
    // The socket is gone: every running call fails, and the next one opens a new socket.
    closed.store(true, Ordering::Relaxed);
    shared.pending.lock().unwrap().clear();
}

/// A `Browser.getVersion` now and then: a socket nobody writes to may be dropped.
async fn keep_alive(conn: Arc<Conn>, every: Duration) {
    loop {
        tokio::time::sleep(every).await;
        if !conn.is_open() || conn.request(None, "Browser.getVersion", json!({}), PING_TIMEOUT).await.is_err() {
            conn.shutdown();
            break;
        }
    }
}

/// Closes the socket after `idle` without a call, so the browser stops showing it as controlled.
async fn idle_watcher(inner: Arc<Mutex<Inner>>, idle: Duration) {
    loop {
        tokio::time::sleep((idle / 4).max(Duration::from_millis(20))).await;
        let due = {
            let g = inner.lock().unwrap();
            g.conn.as_ref().is_some_and(|c| c.is_open()) && g.last_used.is_some_and(|used| used.elapsed() >= idle)
        };
        let conn = inner.lock().unwrap().conn.take_if(|_| due);
        if let Some(conn) = conn {
            conn.close().await;
        }
    }
}

/// When the socket is open: lazily on a call (the browser asks "Allow"), closed by the
/// idle watcher, and reopened by the call after that.
impl BrowserSession {
    /// The open socket, if there is one; its use counts as activity.
    fn live(&self) -> Option<Arc<Conn>> {
        let mut inner = self.inner.lock().unwrap();
        let conn = inner.conn.clone().filter(|c| c.is_open());
        if conn.is_some() {
            inner.last_used = Some(Instant::now());
        }
        conn
    }

    fn just_failed(&self) -> Option<Failure> {
        self.inner.lock().unwrap().failed.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(3)).map(|(_, f)| f.clone())
    }

    /// One connect attempt, whatever went before: the route `attach/connect`.
    pub async fn connect(&self) -> Result<()> {
        self.attempt(false).await
    }

    /// The socket for a call, opened on demand: the first call waits for "Allow".
    pub(super) async fn conn(&self) -> Result<Arc<Conn>> {
        if let Some(c) = self.live() {
            return Ok(c);
        }
        // The calls of one batch share one attempt: the others get its answer at once.
        self.attempt(true).await?;
        self.live().ok_or_else(|| anyhow!("the connection to the browser closed"))
    }

    async fn attempt(&self, skip_if_just_failed: bool) -> Result<()> {
        let _one = self.connecting.lock().await;
        if self.live().is_some() {
            return Ok(());
        }
        if skip_if_just_failed {
            if let Some(f) = self.just_failed() {
                return Err(f.error(&self.name()));
            }
        }
        if self.watcher.set(()).is_ok() {
            tokio::spawn(idle_watcher(self.inner.clone(), self.idle));
        }
        match self.open_socket().await {
            Ok((conn, port, product)) => {
                let mut inner = self.inner.lock().unwrap();
                inner.conn = Some(conn);
                inner.port = Some(port);
                inner.product = Some(product);
                inner.tabs = 0;
                inner.last_used = Some(Instant::now());
                inner.failed = None;
                // Tabs of an earlier browser run (a restart) are gone: they keep their ids, which no new tab has.
                for t in &mut inner.agent_tabs {
                    t.seen = false;
                }
                Ok(())
            }
            Err(f) => {
                let e = f.error(&self.name());
                self.inner.lock().unwrap().failed = Some((Instant::now(), f));
                Err(e)
            }
        }
    }

    /// Read `DevToolsActivePort`, open the socket and ask the browser its name.
    async fn open_socket(&self) -> std::result::Result<(Arc<Conn>, u16, String), Failure> {
        let (dir, name) = self.profile.lock().unwrap().clone().ok_or(Failure::NoProfile)?;
        let file = dir.join("DevToolsActivePort");
        let text = std::fs::read_to_string(&file).map_err(|_| Failure::Off)?;
        let (port, path) = parse_devtools_active_port(&text).ok_or_else(|| Failure::Other(format!("{} holds no port and browser path", file.display())))?;
        let url = format!("ws://127.0.0.1:{port}{path}");
        let started = Instant::now();
        let ws = match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(&url)).await {
            Err(_) => return Err(Failure::Waiting),
            Ok(Err(e)) => return Err(Failure::Other(format!("the browser did not accept the connection to {url}: {e}"))),
            Ok(Ok((ws, _))) => ws,
        };
        let conn = Conn::open(ws);
        let left = CONNECT_TIMEOUT.saturating_sub(started.elapsed());
        let version = match conn.request(None, "Browser.getVersion", json!({}), left).await {
            Ok(v) => v,
            Err(e) => {
                conn.shutdown();
                return Err(if e.downcast_ref::<TimedOut>().is_some() { Failure::Waiting } else { Failure::Other(format!("{e:#}")) });
            }
        };
        let product = version.get("product").and_then(Value::as_str).filter(|p| !p.is_empty()).unwrap_or(&name).to_string();
        tokio::spawn(keep_alive(conn.clone(), self.ping));
        Ok((conn, port, product))
    }

    /// Close the socket: the browser's "controlled" indicator goes away. The next call connects again.
    pub async fn disconnect(&self) {
        let conn = self.inner.lock().unwrap().conn.take();
        if let Some(conn) = conn {
            conn.close().await;
        }
    }
}
