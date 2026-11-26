//! A fake browser for the tests: a DevTools WebSocket server on 127.0.0.1 that answers
//! what the session asks, and the port file in a profile folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMsg;

/// The fake's side of the socket: no TLS on the loopback.
type FakeWs = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// A fake browser: a DevTools WebSocket server, and the port file in a profile.
pub(super) struct Fake {
    pub(super) profile: PathBuf,
    /// (id, title, url) of the pages.
    pub(super) targets: Mutex<Vec<(String, String, String)>>,
    pub(super) connections: AtomicUsize,
    /// The pages the agent opened: T101, T102, ... (no id is given twice).
    pub(super) created: AtomicUsize,
    /// Every `Browser.getVersion` (the first is the connect, the rest are pings).
    pub(super) pings: AtomicUsize,
    /// Session id -> target id.
    pub(super) sessions: Mutex<HashMap<String, String>>,
    /// (target, expression) of every `Runtime.evaluate`, the last one last.
    pub(super) evals: Mutex<Vec<(String, String)>>,
    pub(super) methods: Mutex<Vec<String>>,
    /// The newest socket, for `event`.
    pub(super) socket: tokio::sync::Mutex<Option<Arc<tokio::sync::Mutex<futures::stream::SplitSink<FakeWs, WsMsg>>>>>,
    pub(super) drop: tokio::sync::Notify,
    /// The socket is closed when nothing arrives for this long (as an idle network might).
    pub(super) idle: Duration,
}

impl Fake {
    /// Two pages of the owner's, and the port file that points at this server.
    pub(super) async fn start(profile: &Path, idle: Duration) -> Arc<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::create_dir_all(profile).unwrap();
        std::fs::write(profile.join("DevToolsActivePort"), format!("{port}\n/devtools/browser/fake-1\n")).unwrap();
        let fake = Arc::new(Self {
            profile: profile.to_path_buf(),
            targets: Mutex::new(vec![("T1".into(), "Mail".into(), "https://mail.example/".into()), ("T2".into(), "First".into(), "https://first.example/".into())]),
            connections: AtomicUsize::new(0),
            created: AtomicUsize::new(0),
            pings: AtomicUsize::new(0),
            sessions: Mutex::new(HashMap::new()),
            evals: Mutex::new(Vec::new()),
            methods: Mutex::new(Vec::new()),
            socket: tokio::sync::Mutex::new(None),
            drop: tokio::sync::Notify::new(),
            idle,
        });
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let f = fake.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                f.connections.fetch_add(1, Ordering::SeqCst);
                let f = f.clone();
                tokio::spawn(async move { f.serve(stream).await });
            }
        });
        fake
    }

    async fn serve(&self, stream: tokio::net::TcpStream) {
        let Ok(ws) = tokio_tungstenite::accept_async(stream).await else { return };
        let (tx, mut rx) = ws.split();
        let tx: Arc<tokio::sync::Mutex<futures::stream::SplitSink<FakeWs, WsMsg>>> = Arc::new(tokio::sync::Mutex::new(tx));
        *self.socket.lock().await = Some(tx.clone());
        loop {
            let text = tokio::select! {
                m = tokio::time::timeout(self.idle, rx.next()) => match m { Ok(Some(Ok(WsMsg::Text(t)))) => t.to_string(), _ => break },
                _ = self.drop.notified() => break,
            };
            let Some(reply) = self.answer(&text) else { continue };
            let sent = { tx.lock().await.send(WsMsg::Text(reply.into())).await.is_ok() };
            if !sent {
                break;
            }
        }
        // Really closed: the socket dies with its last half, so drop ours.
        let mine = self.socket.lock().await.as_ref().is_some_and(|s| Arc::ptr_eq(s, &tx));
        if mine {
            self.socket.lock().await.take();
        }
    }

    /// The answer to one command, or `None` for a message it does not answer.
    fn answer(&self, raw: &str) -> Option<String> {
        let v: Value = serde_json::from_str(raw).ok()?;
        let id = v["id"].as_u64()?;
        let method = v["method"].as_str().unwrap_or("");
        let sid = v["sessionId"].as_str().unwrap_or("").to_string();
        let params = &v["params"];
        self.methods.lock().unwrap().push(if sid.is_empty() { method.to_string() } else { format!("{method}@{}", self.sessions.lock().unwrap().get(&sid).cloned().unwrap_or_default()) });
        let result = match (sid.is_empty(), method) {
            (true, "Browser.getVersion") => {
                self.pings.fetch_add(1, Ordering::SeqCst);
                json!({"product": "Fake/9.9.9"})
            }
            (true, "Target.getTargets") => json!({"targetInfos": self.targets.lock().unwrap().iter().map(|(id, title, url)| json!({"targetId": id, "type": "page", "title": title, "url": url})).collect::<Vec<_>>()}),
            (true, "Target.attachToTarget") => {
                let mut sessions = self.sessions.lock().unwrap();
                let session = format!("S{}", sessions.len() + 1);
                sessions.insert(session.clone(), params["targetId"].as_str().unwrap_or("").to_string());
                json!({"sessionId": session})
            }
            (true, "Target.detachFromTarget") => {
                self.sessions.lock().unwrap().remove(params["sessionId"].as_str().unwrap_or(""));
                json!({})
            }
            (true, "Target.createTarget") => {
                let mut targets = self.targets.lock().unwrap();
                let id = format!("T{}", 101 + self.created.fetch_add(1, Ordering::SeqCst));
                targets.push((id.clone(), String::new(), params["url"].as_str().unwrap_or("").to_string()));
                json!({"targetId": id})
            }
            (true, "Target.closeTarget") => {
                let id = params["targetId"].as_str().unwrap_or("");
                self.targets.lock().unwrap().retain(|(t, _, _)| t != id);
                json!({"success": true})
            }
            (true, "Target.activateTarget") => json!({}),
            (false, "Runtime.evaluate") => {
                let target = self.sessions.lock().unwrap().get(&sid).cloned().unwrap_or_default();
                let expression = params["expression"].as_str().unwrap_or("").to_string();
                self.evals.lock().unwrap().push((target.clone(), expression.clone()));
                json!({"result": {"value": format!("{target}:{expression}")}})
            }
            (false, _) => json!({}),
            _ => return Some(json!({"id": id, "error": {"code": -32601, "message": format!("unknown {method}")}}).to_string()),
        };
        let mut reply = json!({"id": id, "result": result});
        if !sid.is_empty() {
            reply["sessionId"] = json!(sid);
        }
        Some(reply.to_string())
    }

    /// An event of a page, sent the way the browser sends it: with the session id.
    pub(super) async fn event(&self, target: &str, method: &str, params: Value) -> bool {
        let Some((sid, _)) = self.sessions.lock().unwrap().iter().find(|(_, t)| t.as_str() == target).map(|(s, t)| (s.clone(), t.clone())) else { return false };
        let Some(tx) = self.socket.lock().await.clone() else { return false };
        let sent = tx.lock().await.send(WsMsg::Text(json!({"sessionId": sid, "method": method, "params": params}).to_string().into())).await;
        sent.is_ok()
    }

    /// The browser closes every socket now (a restart, or Allow withdrawn).
    pub(super) fn drop_socket(&self) {
        self.drop.notify_one();
    }

    pub(super) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(super) fn targets(&self) -> Vec<String> {
        self.targets.lock().unwrap().iter().map(|(id, _, _)| id.clone()).collect()
    }

    pub(super) fn last_eval(&self) -> String {
        self.evals.lock().unwrap().last().map(|(t, _)| t.clone()).expect("something was evaluated")
    }
}
