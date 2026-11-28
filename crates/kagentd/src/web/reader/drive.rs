//! One page read in a tab of the reader's browser (web.md section 8): navigate, wait for
//! the load and for the network to be quiet, wait for a challenge to clear, take the
//! rendered markup. The reader reads: it never clicks, types, accepts a dialog or takes
//! a download. Events are answered while the driver waits for an answer (the socket of
//! `cdp::Session` is read through `send` and `recv`).

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::time::Instant;

use super::challenge::is_challenge;
use crate::browser::cdp::{self, Session};
use crate::web::page_reader::ReaderError;
use crate::web::MAX_BODY;

/// How long a page may take to fire its load event; one without it is read as it is.
const LOAD_WAIT: Duration = Duration::from_secs(25);
/// No request in flight for this long, and at most this long after the load.
const QUIET: Duration = Duration::from_millis(500);
const QUIET_CAP: Duration = Duration::from_secs(5);
/// How long a challenge may take to clear by itself (web.md section 9).
pub const CHALLENGE_WAIT: Duration = Duration::from_secs(20);
/// What the answer to one command may take.
const CALL_WAIT: Duration = Duration::from_secs(20);
/// How much of the page's text the challenge check reads.
const TEXT_PROBE: usize = 4000;

/// A page as the browser rendered it.
pub struct Read {
    pub url: String,
    pub html: String,
}

struct Snapshot {
    url: String,
    title: String,
    text: String,
    html: String,
}

struct Driver {
    session: Session,
    loaded: bool,
    inflight: HashSet<String>,
    /// The status of the last main document.
    status: Option<u16>,
}

fn failed(what: impl std::fmt::Display) -> ReaderError {
    ReaderError::Failed(what.to_string())
}

/// Read `url` in the first tab of the browser on `port`. `challenge_wait` is how long a human-check page may take to clear.
pub async fn read(port: u16, url: &str, challenge_wait: Duration) -> Result<Read, ReaderError> {
    let tabs = cdp::tabs(port).await.map_err(failed)?;
    let tab = match tabs.first() {
        Some(tab) => tab.clone(),
        None => cdp::new_tab(port, "about:blank").await.map_err(failed)?,
    };
    let session = Session::connect(&tab).await.map_err(failed)?;
    let mut driver = Driver { session, loaded: false, inflight: HashSet::new(), status: None };
    let result = driver.run(url, challenge_wait).await;
    // A window the page opened is closed, and the tab is left empty for the next read.
    for other in cdp::tabs(port).await.unwrap_or_default().iter().filter(|t| t.id != tab.id) {
        let _ = cdp::close(port, &other.id).await;
    }
    let _ = driver.call("Page.navigate", json!({"url": "about:blank"})).await;
    result
}

impl Driver {
    async fn run(&mut self, url: &str, challenge_wait: Duration) -> Result<Read, ReaderError> {
        self.call("Page.enable", json!({})).await?;
        self.call("Network.enable", json!({})).await?;
        // Downloads are denied; the call is deprecated in newer browsers and may be unknown.
        let _ = self.call("Page.setDownloadBehavior", json!({"behavior": "deny"})).await;
        let nav = self.call("Page.navigate", json!({"url": url})).await?;
        if nav.get("isDownload").and_then(Value::as_bool) == Some(true) {
            return Err(failed("the address is a download, which the reader never takes"));
        }
        if let Some(error) = nav.get("errorText").and_then(Value::as_str) {
            return Err(failed(format!("the page could not be loaded ({error})")));
        }
        self.wait_loaded().await?;
        self.wait_quiet().await?;
        let mut snap = self.snapshot().await?;
        if is_challenge(&snap.title, &snap.text, &snap.html) {
            let until = Instant::now() + challenge_wait;
            while is_challenge(&snap.title, &snap.text, &snap.html) {
                if Instant::now() >= until {
                    return Err(ReaderError::Challenge(snap.url));
                }
                self.pump(Instant::now() + Duration::from_secs(1)).await?;
                snap = self.snapshot().await?;
            }
            self.wait_quiet().await?;
            snap = self.snapshot().await?;
        }
        if snap.url.starts_with("chrome-error:") {
            return Err(failed("the page could not be loaded"));
        }
        if let Some(code) = self.status.filter(|code| *code >= 400) {
            return Err(ReaderError::Status(code));
        }
        Ok(Read { url: snap.url, html: snap.html })
    }

    /// One command, and its answer; the events that come first are handled.
    async fn call(&mut self, method: &str, params: Value) -> Result<Value, ReaderError> {
        let id = self.session.send(method, params).await.map_err(failed)?;
        let until = Instant::now() + CALL_WAIT;
        loop {
            let Some(message) = self.session.recv(until).await else { return Err(failed(format!("{method}: no answer in time"))) };
            let message = message.map_err(failed)?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                self.handle(&message).await;
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(failed(format!("{method}: {}", error.get("message").and_then(Value::as_str).unwrap_or("error"))));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Handle what comes until `until`; at most one message.
    async fn pump(&mut self, until: Instant) -> Result<bool, ReaderError> {
        match self.session.recv(until).await {
            None => Ok(false),
            Some(message) => {
                let message = message.map_err(failed)?;
                self.handle(&message).await;
                Ok(true)
            }
        }
    }

    async fn handle(&mut self, event: &Value) {
        let params = event.get("params");
        let text = |pointer: &str| params.and_then(|p| p.pointer(pointer)).and_then(Value::as_str).unwrap_or_default().to_string();
        match event.get("method").and_then(Value::as_str) {
            Some("Page.loadEventFired") => self.loaded = true,
            // A dialog is dismissed, never accepted.
            Some("Page.javascriptDialogOpening") => {
                let _ = self.session.send("Page.handleJavaScriptDialog", json!({"accept": false})).await;
            }
            Some("Network.requestWillBeSent") => {
                self.inflight.insert(text("/requestId"));
            }
            Some("Network.loadingFinished" | "Network.loadingFailed") => {
                self.inflight.remove(&text("/requestId"));
            }
            Some("Network.responseReceived") if text("/type") == "Document" => {
                self.status = params.and_then(|p| p.pointer("/response/status")).and_then(Value::as_f64).map(|s| s as u16);
            }
            _ => {}
        }
    }

    async fn wait_loaded(&mut self) -> Result<(), ReaderError> {
        let until = Instant::now() + LOAD_WAIT;
        while !self.loaded && Instant::now() < until {
            self.pump(until).await?;
        }
        Ok(())
    }

    /// Until no request has been in flight for `QUIET`, at most `QUIET_CAP`.
    async fn wait_quiet(&mut self) -> Result<(), ReaderError> {
        let cap = Instant::now() + QUIET_CAP;
        let mut quiet_from = Instant::now();
        while Instant::now() < cap {
            self.pump((quiet_from + QUIET).min(cap)).await?;
            if !self.inflight.is_empty() {
                quiet_from = Instant::now();
            } else if quiet_from.elapsed() >= QUIET {
                break;
            }
        }
        Ok(())
    }

    async fn snapshot(&mut self) -> Result<Snapshot, ReaderError> {
        let script = format!("JSON.stringify({{u: location.href, t: document.title, x: (document.body ? document.body.innerText : '').slice(0, {TEXT_PROBE}), h: document.documentElement ? document.documentElement.outerHTML : ''}})");
        let answer = self.call("Runtime.evaluate", json!({"expression": script, "returnByValue": true})).await?;
        let value = answer.pointer("/result/value").and_then(Value::as_str).ok_or_else(|| failed("the page could not be read"))?;
        let parsed: Value = serde_json::from_str(value).map_err(failed)?;
        let get = |key: &str| parsed.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let snapshot = Snapshot { url: get("u"), title: get("t"), text: get("x"), html: get("h") };
        if snapshot.html.len() > MAX_BODY {
            return Err(ReaderError::TooLarge);
        }
        Ok(snapshot)
    }
}
