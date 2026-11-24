//! Minimal Chrome DevTools Protocol client: HTTP endpoints for tabs, one WebSocket per
//! call for commands. Works with Brave, Chrome, Chromium, Edge.

use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as WsMsg;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Tab {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub web_socket_debugger_url: String,
}

pub fn base(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(5)).no_proxy().build().expect("reqwest client")
}

pub async fn version(port: u16) -> anyhow::Result<Value> {
    Ok(http().get(format!("{}/json/version", base(port))).send().await?.error_for_status()?.json().await?)
}

/// Page tabs (no service workers, extensions, iframes).
pub async fn tabs(port: u16) -> anyhow::Result<Vec<Tab>> {
    let all: Vec<Tab> = http().get(format!("{}/json/list", base(port))).send().await?.error_for_status()?.json().await?;
    Ok(all.into_iter().filter(|t| t.kind == "page" && !t.url.starts_with("chrome-extension://")).collect())
}

pub async fn new_tab(port: u16, url: &str) -> anyhow::Result<Tab> {
    let u = format!("{}/json/new?{}", base(port), url);
    // Chrome 111+ requires PUT for /json/new.
    Ok(http().put(u).send().await?.error_for_status()?.json().await?)
}

pub async fn activate(port: u16, id: &str) -> anyhow::Result<()> {
    http().get(format!("{}/json/activate/{id}", base(port))).send().await?.error_for_status()?;
    Ok(())
}

pub async fn close(port: u16, id: &str) -> anyhow::Result<()> {
    http().get(format!("{}/json/close/{id}", base(port))).send().await?.error_for_status()?;
    Ok(())
}

/// Tab by id, or by part of its title/URL (case-insensitive); default the first page.
pub fn pick<'a>(tabs: &'a [Tab], needle: &str) -> Option<&'a Tab> {
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return tabs.first();
    }
    tabs.iter().find(|t| t.id == needle).or_else(|| tabs.iter().find(|t| t.title.to_lowercase().contains(&n) || t.url.to_lowercase().contains(&n)))
}

/// One CDP session on a tab.
pub struct Session {
    ws: tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    next: u64,
}

impl Session {
    pub async fn connect(tab: &Tab) -> anyhow::Result<Self> {
        if tab.web_socket_debugger_url.is_empty() {
            bail!("tab {} is attached to another DevTools client", tab.id);
        }
        let (ws, _) = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(&tab.web_socket_debugger_url)).await.context("CDP connect timeout")??;
        Ok(Self { ws, next: 1 })
    }

    pub async fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next;
        self.next += 1;
        self.ws.send(WsMsg::Text(json!({"id": id, "method": method, "params": params}).to_string().into())).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let msg = tokio::time::timeout_at(deadline, self.ws.next()).await.map_err(|_| anyhow!("{method}: timeout"))?;
            let Some(msg) = msg else { bail!("{method}: connection closed") };
            let WsMsg::Text(t) = msg? else { continue };
            let v: Value = serde_json::from_str(&t)?;
            if v.get("id").and_then(Value::as_u64) != Some(id) {
                continue; // events
            }
            if let Some(e) = v.get("error") {
                bail!("{method}: {}", e.get("message").and_then(Value::as_str).unwrap_or("error"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Console messages, uncaught exceptions and browser log entries of the page.
    /// Chrome replays what was logged before `Runtime.enable` / `Log.enable`, so this
    /// also returns messages from before the call; then it listens for `listen`.
    pub async fn console(&mut self, listen: Duration, max: usize) -> anyhow::Result<Vec<String>> {
        let mut out = Vec::new();
        for m in ["Runtime.enable", "Log.enable"] {
            let id = self.next;
            self.next += 1;
            self.ws.send(WsMsg::Text(json!({"id": id, "method": m, "params": {}}).to_string().into())).await?;
        }
        let deadline = tokio::time::Instant::now() + listen.max(Duration::from_millis(800));
        while let Ok(Some(msg)) = tokio::time::timeout_at(deadline, self.ws.next()).await {
            let WsMsg::Text(t) = msg? else { continue };
            let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
            if let Some(line) = console_line(&v) {
                out.push(line);
                if out.len() >= max {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Evaluate an expression and return its JSON value (awaits promises).
    pub async fn eval(&mut self, expr: &str) -> anyhow::Result<Value> {
        let r = self.call("Runtime.evaluate", json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true})).await?;
        if let Some(ex) = r.get("exceptionDetails") {
            let text = ex.pointer("/exception/description").or_else(|| ex.get("text")).and_then(Value::as_str).unwrap_or("exception");
            bail!("page script error: {}", text.lines().next().unwrap_or(text));
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    pub async fn click_at(&mut self, x: f64, y: f64) -> anyhow::Result<()> {
        for t in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.call("Input.dispatchMouseEvent", json!({"type": t, "x": x, "y": y, "button": "left", "clickCount": 1})).await?;
        }
        Ok(())
    }

    pub async fn key(&mut self, key: &str) -> anyhow::Result<()> {
        let (code, vk, text) = key_info(key).ok_or_else(|| anyhow!("unsupported key {key}"))?;
        let mut down = json!({"type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk});
        if let Some(t) = text {
            down["text"] = json!(t);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk})).await?;
        Ok(())
    }
}

/// One CDP event -> a console line (`[error] message (url:line)`), or None.
pub fn console_line(v: &Value) -> Option<String> {
    let p = v.get("params")?;
    match v.get("method")?.as_str()? {
        "Runtime.consoleAPICalled" => {
            let args: Vec<String> = p.get("args")?.as_array()?.iter().map(|a| a.get("value").map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).or_else(|| a.get("description").and_then(Value::as_str).map(String::from)).unwrap_or_default()).collect();
            Some(format!("[{}] {}", p.get("type")?.as_str()?, args.join(" ")))
        }
        "Runtime.exceptionThrown" => {
            let d = p.get("exceptionDetails")?;
            let desc = d.pointer("/exception/description").and_then(Value::as_str).or_else(|| d.get("text").and_then(Value::as_str)).unwrap_or("exception");
            let frames: Vec<&str> = desc.lines().take(4).collect();
            Some(format!("[exception] {}", frames.join("\n    ")))
        }
        "Log.entryAdded" => {
            let e = p.get("entry")?;
            let url = e.get("url").and_then(Value::as_str).unwrap_or("");
            Some(format!("[{} {}] {}{}", e.get("source")?.as_str()?, e.get("level")?.as_str()?, e.get("text")?.as_str()?, if url.is_empty() { String::new() } else { format!(" ({url})") }))
        }
        _ => None,
    }
}

/// DOM key -> (code, virtual key code, text).
pub fn key_info(key: &str) -> Option<(&'static str, u32, Option<&'static str>)> {
    Some(match key {
        "Enter" => ("Enter", 13, Some("\r")),
        "Tab" => ("Tab", 9, None),
        "Escape" => ("Escape", 27, None),
        "Backspace" => ("Backspace", 8, None),
        "Delete" => ("Delete", 46, None),
        "ArrowDown" => ("ArrowDown", 40, None),
        "ArrowUp" => ("ArrowUp", 38, None),
        "ArrowLeft" => ("ArrowLeft", 37, None),
        "ArrowRight" => ("ArrowRight", 39, None),
        "PageDown" => ("PageDown", 34, None),
        "PageUp" => ("PageUp", 33, None),
        "Home" => ("Home", 36, None),
        "End" => ("End", 35, None),
        " " | "Space" => ("Space", 32, Some(" ")),
        _ => return None,
    })
}

/// Page snapshot: title, URL, visible text and numbered interactive elements. Elements
/// get a `data-kagent` attribute so a later click/type can find them by number.
pub const SNAPSHOT_JS: &str = r#"(() => {
  const vis = el => { const r = el.getBoundingClientRect(); const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== 'hidden' && s.display !== 'none'; };
  const sel = 'a[href],button,input:not([type=hidden]),textarea,select,[role=button],[role=link],[role=tab],[role=menuitem],[role=checkbox],[contenteditable=true],summary';
  const items = []; let n = 0;
  document.querySelectorAll('[data-kagent]').forEach(e => e.removeAttribute('data-kagent'));
  for (const el of document.querySelectorAll(sel)) {
    if (!vis(el) || n >= MAX) continue;
    n++; el.setAttribute('data-kagent', String(n));
    const tag = el.tagName.toLowerCase();
    const label = (el.getAttribute('aria-label') || el.innerText || el.value || el.placeholder || el.title || el.name || el.getAttribute('href') || '').trim().replace(/\s+/g, ' ').slice(0, 80);
    const type = tag === 'input' ? (el.type || 'text') : (el.getAttribute('role') || tag);
    items.push(`[${n}] ${type}${el.disabled ? ' (disabled)' : ''}: ${label}`);
  }
  const text = (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, TEXT);
  return {title: document.title, url: location.href, text, elements: items};
})()"#;

pub fn snapshot_js(max_elements: usize, text_chars: usize) -> String {
    SNAPSHOT_JS.replace("MAX", &max_elements.to_string()).replace("TEXT", &text_chars.to_string())
}

/// JS that focuses the page's main search/text box and returns its centre (or null).
pub const SEARCH_BOX_JS: &str = r#"(() => {
  const el = document.querySelector('input[type=search],input[name=q],input[name=search],[role=searchbox],input[type=text],input:not([type]),textarea');
  if (!el) return null; el.scrollIntoView({block: 'center'}); const r = el.getBoundingClientRect();
  return {x: r.left + r.width / 2, y: r.top + r.height / 2};
})()"#;

/// JS that finds an element by snapshot number, CSS selector or visible text, scrolls it
/// into view and returns its centre (or null).
pub fn locate_js(element: Option<u64>, selector: &str, text: &str) -> String {
    let find = if let Some(n) = element {
        format!("document.querySelector('[data-kagent=\"{n}\"]')")
    } else if !selector.is_empty() {
        format!("document.querySelector({})", serde_json::to_string(selector).unwrap_or_default())
    } else {
        let t = serde_json::to_string(&text.to_lowercase()).unwrap_or_default();
        format!("[...document.querySelectorAll('a,button,[role=button],[role=link],input[type=submit],summary,label')].find(e => (e.innerText || e.value || e.getAttribute('aria-label') || '').toLowerCase().includes({t}))")
    };
    format!("(() => {{ const el = {find}; if (!el) return null; el.scrollIntoView({{block: 'center'}}); const r = el.getBoundingClientRect(); return {{x: r.left + r.width / 2, y: r.top + r.height / 2, tag: el.tagName.toLowerCase(), label: (el.innerText || el.value || el.getAttribute('aria-label') || el.placeholder || '').trim().slice(0, 60)}}; }})()")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_parsing_and_pick() {
        let raw = r#"[{"id":"A","type":"page","title":"GitHub","url":"https://github.com","webSocketDebuggerUrl":"ws://x/A"},
                     {"id":"B","type":"service_worker","title":"sw","url":"https://x"},
                     {"id":"C","type":"page","title":"YouTube - Music","url":"https://youtube.com/watch"}]"#;
        let all: Vec<Tab> = serde_json::from_str(raw).unwrap();
        let pages: Vec<Tab> = all.into_iter().filter(|t| t.kind == "page").collect();
        assert_eq!(pick(&pages, "").unwrap().id, "A");
        assert_eq!(pick(&pages, "youtube").unwrap().id, "C");
        assert_eq!(pick(&pages, "C").unwrap().id, "C");
        assert!(pick(&pages, "reddit").is_none());
    }

    #[test]
    fn console_events() {
        let c: Value = serde_json::from_str(r#"{"method":"Runtime.consoleAPICalled","params":{"type":"error","args":[{"type":"string","value":"boom"},{"type":"number","value":3}]}}"#).unwrap();
        assert_eq!(console_line(&c).unwrap(), "[error] boom 3");
        let e: Value = serde_json::from_str(r#"{"method":"Runtime.exceptionThrown","params":{"exceptionDetails":{"text":"Uncaught","exception":{"description":"TypeError: destroy_ is not a function\n    at a\n    at b"}}}}"#).unwrap();
        assert!(console_line(&e).unwrap().starts_with("[exception] TypeError: destroy_ is not a function"));
        let l: Value = serde_json::from_str(r#"{"method":"Log.entryAdded","params":{"entry":{"source":"network","level":"error","text":"404","url":"http://x/a.js"}}}"#).unwrap();
        assert_eq!(console_line(&l).unwrap(), "[network error] 404 (http://x/a.js)");
        assert!(console_line(&serde_json::json!({"id": 1, "result": {}})).is_none());
    }

    #[test]
    fn js_builders_escape_input() {
        let js = locate_js(None, "", "Sign \"in\"");
        assert!(js.contains(r#"includes("sign \"in\"")"#), "{js}");
        assert!(locate_js(Some(4), "", "").contains(r#"[data-kagent="4"]"#));
        assert!(locate_js(None, "#q", "").contains(r##"querySelector("#q")"##));
        let s = snapshot_js(50, 4000);
        assert!(s.contains("n >= 50") && s.contains("slice(0, 4000)"));
        assert_eq!(key_info("Enter"), Some(("Enter", 13, Some("\r"))));
        assert!(key_info("F13").is_none());
    }
}
