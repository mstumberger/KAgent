//! Router tools for the agent (docs/specs/tools.md section 11) and /api/net handlers.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use kagent_core::tools::privop::record_user;

use super::Net;

pub struct RouterTool {
    contract: ToolContract,
    net: Arc<Net>,
}

fn ro(name: &str, desc: &str, schema: Value) -> ToolContract {
    ToolContract::read_only(name, desc, schema)
}

pub fn all(net: Arc<Net>) -> Vec<Arc<dyn Tool>> {
    let mk = |contract: ToolContract| Arc::new(RouterTool { contract, net: net.clone() }) as Arc<dyn Tool>;
    vec![
        mk(ro("router_status", "MikroTik router: model, version, uptime, CPU, memory, interfaces with traffic counters, connection count.", json!({"type":"object","properties":{}}))),
        mk(ro("lan_devices", "Devices on the LAN (router DHCP leases + ARP) with first/last seen from KAgent's history.", json!({"type":"object","properties":{}}))),
        mk(ro("router_log", "Recent router log lines (logins, DHCP, firewall), optional text filter.", json!({"type":"object","properties":{"grep":{"type":"string"},"limit":{"type":"integer"}}}))),
        mk(ro("router_lists", "Router firewall address lists (scanners, kagent-block, ...).", json!({"type":"object","properties":{"list":{"type":"string"}}}))),
        mk(ro("traffic_top", "Top traffic from Traffic Flow: per device or whole LAN, last N minutes: remote address, port, bytes.", json!({"type":"object","properties":{"device":{"type":"string","description":"LAN IP; omit for all"},"minutes":{"type":"integer","description":"default 60"}}}))),
        mk(ro("router_block_ip", "Block an IP at the router (address list kagent-block with a timeout). Always asks; undoable.", json!({"type":"object","required":["address"],"properties":{"address":{"type":"string"},"timeout":{"type":"string","description":"RouterOS time, default 1d"},"comment":{"type":"string"}}}))
            .mutating(SideEffect::Network, Risk::High)),
        mk(ro("router_unblock_ip", "Remove an IP from the router block list.", json!({"type":"object","required":["address"],"properties":{"address":{"type":"string"}}}))
            .mutating(SideEffect::Network, Risk::Medium)),
    ]
}

fn valid_ip(s: &str) -> bool {
    match s.split_once('/') {
        None => s.parse::<std::net::IpAddr>().is_ok(),
        Some((ip, p)) => match (ip.parse::<std::net::IpAddr>(), p.parse::<u8>()) {
            (Ok(std::net::IpAddr::V4(_)), Ok(p)) => p <= 32,
            (Ok(std::net::IpAddr::V6(_)), Ok(p)) => p <= 128,
            _ => false,
        },
    }
}

#[async_trait]
impl Tool for RouterTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let addr = input.get("address").and_then(Value::as_str).unwrap_or("");
        let summary = format!("{} {addr}", self.contract.name).trim().to_string();
        let mut a = CallAssessment::from_contract(&self.contract, summary.clone(), summary);
        if self.contract.name == "router_block_ip" {
            a.always_ask = true; // containment is never remembered
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let net = &self.net;
        let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match self.contract.name.as_str() {
            "router_status" => Ok(ToolOutput::json(&net.status().await)),
            "lan_devices" => {
                let devs = net.store.as_ref().map(|s| s.devices()).unwrap_or_default();
                if devs.is_empty() {
                    let live = super::mikrotik::lan_devices(&net.router.get("/ip/dhcp-server/lease").await.unwrap_or(Value::Null), &net.router.get("/ip/arp").await?);
                    return Ok(ToolOutput::ok(live.iter().map(|(m, i, h)| format!("{i} {m} {h}")).collect::<Vec<_>>().join("\n")));
                }
                let rows: Vec<String> = devs
                    .iter()
                    .map(|d| format!("{} {} {} first {} last {}", d["ip"].as_str().unwrap_or(""), d["mac"].as_str().unwrap_or(""), d["hostname"].as_str().unwrap_or(""), fmt_ts(d["firstSeen"].as_i64()), fmt_ts(d["lastSeen"].as_i64())))
                    .collect();
                Ok(ToolOutput::ok(rows.join("\n")))
            }
            "router_log" => {
                let log = net.router.get("/log").await?;
                let grep = s("grep").to_lowercase();
                let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(60) as usize;
                let lines: Vec<String> = log.as_array().into_iter().flatten().filter_map(|e| {
                    let line = format!("{} [{}] {}", e["time"].as_str().unwrap_or(""), e["topics"].as_str().unwrap_or(""), e["message"].as_str().unwrap_or(""));
                    (grep.is_empty() || line.to_lowercase().contains(&grep)).then_some(line)
                }).collect();
                Ok(ToolOutput::ok(lines[lines.len().saturating_sub(limit)..].join("\n")))
            }
            "router_lists" => {
                let list = s("list");
                let path = if list.is_empty() { "/ip/firewall/address-list".to_string() } else { format!("/ip/firewall/address-list?list={list}") };
                let v = net.router.get(&path).await?;
                let rows: Vec<String> = v.as_array().into_iter().flatten().map(|e| format!("{} {} timeout={} {}", e["list"].as_str().unwrap_or(""), e["address"].as_str().unwrap_or(""), e["timeout"].as_str().unwrap_or("-"), e["comment"].as_str().unwrap_or(""))).collect();
                Ok(ToolOutput::ok(if rows.is_empty() { "empty".into() } else { rows.join("\n") }))
            }
            "traffic_top" => {
                let Some(store) = &net.store else { return Ok(ToolOutput::error("traffic store unavailable")) };
                let minutes = input.get("minutes").and_then(Value::as_i64).unwrap_or(60);
                let dev = s("device");
                let since = (chrono::Utc::now().timestamp() - minutes * 60) / super::BUCKET * super::BUCKET;
                let rows = store.top((!dev.is_empty()).then_some(dev.as_str()), since);
                if rows.is_empty() {
                    return Ok(ToolOutput::ok("no flow data yet (is Traffic Flow exporting to this PC?)"));
                }
                Ok(ToolOutput::ok(rows.iter().map(|r| format!("{} -> {}:{} proto {} {:.1} MiB", r["device"].as_str().unwrap_or(""), r["remote"].as_str().unwrap_or(""), r["port"], r["proto"], r["bytes"].as_i64().unwrap_or(0) as f64 / 1048576.0)).collect::<Vec<_>>().join("\n")))
            }
            "router_block_ip" => {
                let addr = s("address");
                if !valid_ip(&addr) {
                    return Ok(ToolOutput::error("address must be an IP or CIDR"));
                }
                let timeout = if s("timeout").is_empty() { "1d".to_string() } else { s("timeout") };
                let comment = if s("comment").is_empty() { "blocked by KAgent".to_string() } else { s("comment") };
                let r = net.router.block(&addr, &timeout, &comment).await;
                let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "kagentd".into());
                let undo = json!({"kind": "user", "action": "command", "argv": [exe, "router-unblock", addr]});
                record_user(&ctx, "router_block_ip", &format!("router block {addr} for {timeout}"), json!({"kind": "router", "action": "block", "address": addr}), r.is_ok(), Some(undo), None, None, &r.map(|v| v.to_string()).unwrap_or_else(|e| format!("{e:#}")))
            }
            "router_unblock_ip" => {
                let addr = s("address");
                match net.router.unblock(&addr).await {
                    Ok(n) => Ok(ToolOutput::ok(format!("removed {n} block entries for {addr}"))),
                    Err(e) => Ok(ToolOutput::error(format!("{e:#}"))),
                }
            }
            other => Ok(ToolOutput::error(format!("unknown router tool {other}"))),
        }
    }
}

fn fmt_ts(t: Option<i64>) -> String {
    t.and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()).unwrap_or_default()
}

pub mod http {
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::extract::{Query, State};
    use axum::response::{IntoResponse, Response};
    use axum::Json;
    use serde_json::{json, Value};

    use crate::net::mikrotik::RouterConfig;
    use crate::AppState;

    pub async fn status(State(st): State<Arc<AppState>>) -> Response {
        let cfg = st.net.router.cfg.lock().unwrap().clone();
        let configured = st.net.router.configured();
        let live = if configured { st.net.status().await } else { json!({"ok": false, "error": "not configured"}) };
        Json(json!({"configured": configured, "config": cfg, "router": live})).into_response()
    }

    /// Owner configures the router. Saving with a host pins the certificate on the first
    /// successful call (trust on first use).
    pub async fn configure(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
        let mut cfg: RouterConfig = serde_json::from_value(body.clone()).unwrap_or_default();
        if cfg.flow_port == 0 {
            cfg.flow_port = 2055;
        }
        let pass = body.get("password").and_then(Value::as_str).map(String::from);
        if let Some(p) = &pass {
            if let Err(e) = st.net.save_secret(p) {
                return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"code": "internal", "message": e.to_string()}}))).into_response();
            }
        }
        st.net.router.set(cfg, pass);
        let live = st.net.status().await;
        // Persist (including the pinned fingerprint learned by the status call).
        let saved = st.net.router.cfg.lock().unwrap().clone();
        {
            let mut s = st.settings.write().unwrap();
            s.extra.insert("mikrotik".into(), serde_json::to_value(&saved).unwrap_or_default());
            let _ = s.save(&crate::paths::Paths::resolve().settings());
        }
        st.net.start();
        Json(json!({"config": saved, "router": live})).into_response()
    }

    pub async fn devices(State(st): State<Arc<AppState>>) -> Response {
        Json(st.net.store.as_ref().map(|s| s.devices()).unwrap_or_default()).into_response()
    }

    pub async fn top(State(st): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Response {
        let minutes: i64 = q.get("minutes").and_then(|m| m.parse().ok()).unwrap_or(60);
        let since = (chrono::Utc::now().timestamp() - minutes * 60) / crate::net::BUCKET * crate::net::BUCKET;
        let dev = q.get("device").map(String::as_str).filter(|d| !d.is_empty());
        Json(st.net.store.as_ref().map(|s| s.top(dev, since)).unwrap_or_default()).into_response()
    }

    pub async fn unblock(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
        let addr = body.get("address").and_then(Value::as_str).unwrap_or("");
        match st.net.router.unblock(addr).await {
            Ok(n) => Json(json!({"removed": n})).into_response(),
            Err(e) => (axum::http::StatusCode::BAD_GATEWAY, Json(json!({"error": {"code": "router", "message": format!("{e:#}")}}))).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ip_validation() {
        assert!(super::valid_ip("203.0.113.9") && super::valid_ip("10.0.0.0/8") && super::valid_ip("2001:db8::1"));
        assert!(!super::valid_ip("evil; /system reset") && !super::valid_ip("1.2.3.4/99"));
    }
}
