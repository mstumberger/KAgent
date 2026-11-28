//! `kagentd mcp`: MCP server over stdio (newline-delimited JSON-RPC 2.0) exposing
//! KAgent's typed system tools to Claude Code, ZCode, Codex, Goose...
//!
//! A thin client of the running daemon: every call goes through
//! `POST /api/tools/:name/call`, so permissions, approval cards (in the KAgent UI),
//! the change journal and the audit log apply. The route is the MCP entry: the session is `mcp`
//! and the origin `agent`, so every call that is not read-only asks, in every permission mode.
//! Hand-rolled instead of an SDK: the server surface is four methods.

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOL: &str = "2025-06-18";

struct Daemon {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl Daemon {
    async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let r = self.http.get(format!("{}{path}", self.base)).bearer_auth(&self.token).send().await?;
        Ok(r.error_for_status()?.json().await?)
    }
    async fn post(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        let r = self.http.post(format!("{}{path}", self.base)).bearer_auth(&self.token).json(&body).send().await?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!("{}", v.pointer("/error/message").and_then(Value::as_str).unwrap_or("daemon error"));
        }
        Ok(v)
    }
}

/// MCP tool entry from a contract (annotations drive client-side UX).
pub fn tool_entry(c: &Value) -> Value {
    json!({
        "name": c["name"],
        "description": c["description"],
        "inputSchema": c["inputSchema"],
        "annotations": {
            "readOnlyHint": c["readOnly"],
            "destructiveHint": c["destructive"],
            "openWorldHint": false,
        }
    })
}

async fn handle(d: &Daemon, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?; // notifications get no response
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "kagent", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "KAgent: typed, audited tools for this Linux machine. Mutating and root actions ask the owner for approval in the KAgent UI; a denied call returns 'not executed'."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => match d.get("/api/tools?for=mcp").await {
            Ok(v) => {
                let tools: Vec<Value> = v["tools"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t["mcp"].as_bool() == Some(true))
                    .map(|t| tool_entry(&t["contract"]))
                    .collect();
                Ok(json!({"tools": tools}))
            }
            Err(e) => Err((-32000, format!("kagentd unreachable: {e}"))),
        },
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            match d.post(&format!("/api/tools/{name}/call"), json!({"input": args})).await {
                Ok(v) => {
                    let status = v["status"].as_str().unwrap_or("error");
                    Ok(json!({
                        "content": [{"type": "text", "text": v["output"].as_str().unwrap_or("")}],
                        "isError": status != "ok"
                    }))
                }
                Err(e) => Ok(json!({"content": [{"type": "text", "text": format!("KAgent error: {e}")}], "isError": true})),
            }
        }
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    })
}

pub async fn serve_stdio(paths: &crate::paths::Paths) -> anyhow::Result<()> {
    let token = std::fs::read_to_string(paths.token())
        .map_err(|_| anyhow::anyhow!("no token at {}: start kagentd first", paths.token().display()))?
        .trim()
        .to_string();
    let d = Daemon { base: "http://127.0.0.1:7878".into(), token, http: reqwest::Client::new() };
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&d, &msg).await,
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        };
        if let Some(r) = reply {
            out.write_all(format!("{r}\n").as_bytes()).await?;
            out.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_entry_maps_annotations() {
        let c = json!({"name": "sysctl", "description": "d", "inputSchema": {"type": "object"}, "readOnly": false, "destructive": true});
        let e = tool_entry(&c);
        assert_eq!(e["annotations"]["destructiveHint"], true);
        assert_eq!(e["inputSchema"]["type"], "object");
    }

    #[tokio::test]
    async fn notifications_get_no_reply_and_unknown_methods_error() {
        let d = Daemon { base: "http://127.0.0.1:1".into(), token: String::new(), http: reqwest::Client::new() };
        assert!(handle(&d, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await.is_none());
        let r = handle(&d, &json!({"jsonrpc": "2.0", "id": 7, "method": "nope"})).await.unwrap();
        assert_eq!(r["error"]["code"], -32601);
        let r = handle(&d, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "kagent");
    }
}
