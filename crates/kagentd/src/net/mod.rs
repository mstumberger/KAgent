//! LAN monitoring through a MikroTik router (S15): Traffic Flow collector, device and
//! router-log watchers, router tools. Everything is idle until a router is configured.

pub mod flows;
pub mod mikrotik;
pub mod store;
pub mod tools;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::watch::{AlertManager, Hit, Severity};
use mikrotik::{Router, RouterConfig};
pub use store::BUCKET;
use store::{Agg, FlowStore};

pub struct Net {
    pub router: Router,
    pub store: Option<FlowStore>,
    secret_path: PathBuf,
    alerts: Arc<AlertManager>,
    started: Mutex<Option<CancellationToken>>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

impl Net {
    pub fn new(cfg: RouterConfig, config_dir: &std::path::Path, data_dir: &std::path::Path, alerts: Arc<AlertManager>) -> Arc<Self> {
        let secret_path = config_dir.join("router.secret");
        let pass = std::fs::read_to_string(&secret_path).unwrap_or_default().trim().to_string();
        let store = FlowStore::open(&data_dir.join("net.sqlite")).map_err(|e| tracing::warn!("net store: {e:#}")).ok();
        Arc::new(Self { router: Router::new(cfg, pass), store, secret_path, alerts, started: Mutex::new(None) })
    }

    pub fn save_secret(&self, pass: &str) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&self.secret_path, pass)?;
        std::fs::set_permissions(&self.secret_path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    /// (Re)start collector + pollers for the current config. No-op when unconfigured.
    pub fn start(self: &Arc<Self>) {
        if let Some(old) = self.started.lock().unwrap().take() {
            old.cancel();
        }
        if !self.router.configured() {
            return;
        }
        let cancel = CancellationToken::new();
        *self.started.lock().unwrap() = Some(cancel.clone());
        let (a, c) = (self.clone(), cancel.clone());
        tokio::spawn(async move {
            if let Err(e) = a.collect(c).await {
                tracing::warn!("flow collector stopped: {e:#}");
            }
        });
        let (a, c) = (self.clone(), cancel);
        tokio::spawn(async move { a.poll(c).await });
    }

    async fn collect(self: Arc<Self>, cancel: CancellationToken) -> anyhow::Result<()> {
        let (port, router_host) = {
            let c = self.router.cfg.lock().unwrap();
            (if c.flow_port == 0 { 2055 } else { c.flow_port }, c.host.split(':').next().unwrap_or("").to_string())
        };
        let sock = tokio::net::UdpSocket::bind(("0.0.0.0", port)).await?;
        tracing::info!("Traffic Flow collector on UDP {port} (accepting only {router_host})");
        let mut decoder = flows::Decoder::new();
        let mut agg = Agg::new();
        let mut bucket = now() / BUCKET * BUCKET;
        let mut buf = vec![0u8; 65535];
        let mut flush = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                r = sock.recv_from(&mut buf) => {
                    let (n, from) = r?;
                    // Only the configured router may feed flows (spoofing would poison the baseline).
                    if from.ip().to_string() != router_host {
                        continue;
                    }
                    for f in decoder.decode(&from.ip().to_string(), &buf[..n]) {
                        store::add_flow(&mut agg, &f);
                    }
                }
                _ = flush.tick() => {
                    let b = now() / BUCKET * BUCKET;
                    if b != bucket {
                        let done = std::mem::take(&mut agg);
                        self.finish_bucket(bucket, &done);
                        bucket = b;
                    }
                }
                _ = cancel.cancelled() => return Ok(()),
            }
        }
    }

    fn finish_bucket(&self, bucket: i64, agg: &Agg) {
        let Some(s) = &self.store else { return };
        use chrono::Timelike;
        let hour = chrono::Local::now().hour();
        let honeypot = self.router.cfg.lock().unwrap().honeypot_ip.clone();
        for h in store::bucket_rules(s, bucket, agg, hour, (!honeypot.is_empty()).then_some(honeypot.as_str())) {
            self.alerts.hit(h);
        }
        if let Err(e) = s.ingest(bucket, agg) {
            tracing::warn!("flow ingest: {e:#}");
        }
        s.prune(bucket);
    }

    async fn poll(self: Arc<Self>, cancel: CancellationToken) {
        let mut failures = 0u32;
        let mut seen_scanners = std::collections::HashSet::new();
        let mut last_log_id = String::new();
        let mut first = true;
        loop {
            match self.poll_once(&mut seen_scanners, &mut last_log_id, first).await {
                Ok(()) => {
                    failures = 0;
                    first = false;
                }
                Err(e) => {
                    failures += 1;
                    tracing::warn!("router poll failed ({failures}): {e:#}");
                    if failures == 5 {
                        self.alerts.hit(Hit { rule: "router-unreachable", key: "rtr-down".into(), severity: Severity::Info, title: "MikroTik router unreachable".into(), cause: format!("{e:#}"), evidence: String::new(), proposal: "check the router and the REST service (www-ssl)".into(), threshold: None, cooldown: Duration::from_secs(6 * 3600) });
                    }
                }
            }
            tokio::select! { _ = tokio::time::sleep(Duration::from_secs(120)) => {}, _ = cancel.cancelled() => return }
        }
    }

    async fn poll_once(&self, seen_scanners: &mut std::collections::HashSet<String>, last_log_id: &mut String, first: bool) -> anyhow::Result<()> {
        let leases = self.router.get("/ip/dhcp-server/lease").await.unwrap_or(Value::Null);
        let arp = self.router.get("/ip/arp").await?;
        if let Some(s) = &self.store {
            let fresh_db = s.device_count() == 0;
            for (mac, ip, host) in mikrotik::lan_devices(&leases, &arp) {
                // The first inventory is the baseline, not a flood of "new device".
                if s.upsert_device(&mac, &ip, &host, now())? && !fresh_db {
                    self.alerts.hit(Hit { rule: "lan-new-device", key: format!("newdev:{mac}"), severity: Severity::Review, title: format!("New device on the LAN: {} ({ip}, {mac})", if host.is_empty() { "unknown" } else { &host }), cause: "a MAC address KAgent has never seen joined the network".into(), evidence: format!("{mac} {ip} {host}"), proposal: "if you do not recognise it: check the router's wireless clients and change the Wi-Fi password".into(), threshold: None, cooldown: Duration::ZERO });
                }
            }
        }
        if let Ok(list) = self.router.get("/ip/firewall/address-list?list=scanners").await {
            for e in list.as_array().into_iter().flatten() {
                let addr = e.get("address").and_then(Value::as_str).unwrap_or("").to_string();
                if !addr.is_empty() && seen_scanners.insert(addr.clone()) && !first {
                    let inside = addr.parse::<std::net::IpAddr>().is_ok_and(|ip| flows::is_lan(&ip));
                    self.alerts.hit(Hit { rule: "router-scanner", key: format!("scan:{addr}"), severity: if inside { Severity::Review } else { Severity::Info }, title: format!("Port scan from {addr}{}", if inside { " (inside your LAN)" } else { "" }), cause: "the router's port-scan detector flagged this address".into(), evidence: addr.clone(), proposal: if inside { "find the device and check it" } else { "internet background noise unless it repeats; router_block_ip if it does" }.into(), threshold: None, cooldown: Duration::from_secs(24 * 3600) });
                }
            }
        }
        let trap = self.router.cfg.lock().unwrap().trap_user.clone();
        let log = self.router.get("/log").await?;
        let entries: Vec<&Value> = log.as_array().map(|a| a.iter().collect()).unwrap_or_default();
        let start = entries.iter().position(|e| e.get(".id").and_then(Value::as_str) == Some(last_log_id.as_str())).map(|i| i + 1).unwrap_or(if first { entries.len() } else { 0 });
        for e in &entries[start.min(entries.len())..] {
            let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
            if let Some((user, ip)) = mikrotik::login_failure(msg) {
                let sev = if !trap.is_empty() && user == trap { Severity::High } else { Severity::Review };
                self.alerts.hit(Hit { rule: if sev == Severity::High { "router-trap-account" } else { "router-login-failed" }, key: format!("rtr-login:{ip}"), severity: sev, title: format!("Router login failure as {user} from {ip}"), cause: "someone tried to log into the router".into(), evidence: msg.into(), proposal: "if repeated: block the source and check the router's exposed services".into(), threshold: if sev == Severity::High { None } else { Some((3, Duration::from_secs(600))) }, cooldown: Duration::from_secs(3600) });
            }
            if let Some((user, ip)) = mikrotik::login_success(msg) {
                if !trap.is_empty() && user == trap {
                    self.alerts.hit(Hit { rule: "router-trap-account", key: format!("rtr-trap:{ip}"), severity: Severity::High, title: format!("Decoy router account '{user}' logged in from {ip}"), cause: "the trap account's password was used: someone has it".into(), evidence: msg.into(), proposal: "treat as compromise: change router passwords, check the router config and the device at that IP".into(), threshold: None, cooldown: Duration::ZERO });
                }
            }
        }
        if let Some(last) = entries.last().and_then(|e| e.get(".id")).and_then(Value::as_str) {
            *last_log_id = last.to_string();
        }
        Ok(())
    }

    pub async fn status(&self) -> Value {
        let resource = self.router.get("/system/resource").await;
        let tracking = self.router.get("/ip/firewall/connection/tracking").await.ok();
        let ifaces = self.router.get("/interface").await.ok();
        match resource {
            Ok(r) => json!({"ok": true, "resource": r, "connections": tracking.and_then(|t| t.get("total-entries").cloned()), "interfaces": ifaces}),
            Err(e) => json!({"ok": false, "error": format!("{e:#}")}),
        }
    }
}

/// `kagentd mikrotik-setup <pc-ip>`: RouterOS commands for the owner to paste.
pub fn print_setup(pc_ip: &str) {
    println!("# Paste into the MikroTik terminal (review first). KAgent never logs into the router as admin.");
    println!("/user group add name=kagent-ro policy=read,api,rest-api,!write,!policy,!sensitive");
    println!("/user add name=kagent-ro group=kagent-ro password=<choose-a-strong-password>");
    println!("/ip service set www-ssl disabled=no   ;# REST over HTTPS (create a certificate if none)");
    println!("/ip traffic-flow set enabled=yes interfaces=all");
    println!("/ip traffic-flow target add dst-address={pc_ip} port=2055 version=ipfix");
    println!("/ip firewall filter add chain=input protocol=tcp psd=21,3s,3,1 action=add-src-to-address-list address-list=scanners address-list-timeout=1d comment=kagent");
    println!();
    println!("# Optional containment (KAgent can then block IPs, always after your click):");
    println!("/user group add name=kagent-rw policy=read,write,api,rest-api,!policy,!sensitive");
    println!("/ip firewall raw add chain=prerouting src-address-list=kagent-block action=drop comment=kagent");
    println!();
    println!("# Optional trap account: never use it; any login with it raises a high alert.");
    println!("/user add name=admin2 group=read password=<a-tempting-but-unique-password>");
    println!("# Then in KAgent: Settings -> Network: host, user, password, trap user = admin2.");
}
