//! MikroTik RouterOS v7 REST client (`https://<router>/rest/...`). Self-signed router
//! certificates are accepted only when their SHA-256 matches the pinned fingerprint
//! (trust on first use: the owner's "connect" call pins it).

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RouterConfig {
    pub host: String,
    pub user: String,
    pub fingerprint: String,
    pub flow_port: u16,
    pub honeypot_ip: String,
    pub trap_user: String,
    pub https: bool,
}

pub struct Router {
    pub cfg: Mutex<RouterConfig>,
    pass: Mutex<String>,
    http: reqwest::Client,
}

pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

impl Router {
    pub fn new(cfg: RouterConfig, pass: String) -> Self {
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true) // pinned below instead
            .tls_info(true)
            .timeout(Duration::from_secs(10))
            .build()
            .expect("http client");
        Self { cfg: Mutex::new(cfg), pass: Mutex::new(pass), http }
    }

    pub fn configured(&self) -> bool {
        let c = self.cfg.lock().unwrap();
        !c.host.is_empty() && !c.user.is_empty()
    }

    pub fn set(&self, cfg: RouterConfig, pass: Option<String>) {
        *self.cfg.lock().unwrap() = cfg;
        if let Some(p) = pass {
            *self.pass.lock().unwrap() = p;
        }
    }

    fn url(&self, path: &str) -> String {
        let c = self.cfg.lock().unwrap();
        format!("{}://{}/rest{}", if c.https { "https" } else { "http" }, c.host, path)
    }

    /// Verify (or, when unset, learn) the certificate fingerprint of this response.
    fn check_pin(&self, resp: &reqwest::Response) -> anyhow::Result<()> {
        let mut c = self.cfg.lock().unwrap();
        if !c.https {
            return Ok(());
        }
        let der = resp.extensions().get::<reqwest::tls::TlsInfo>().and_then(|i| i.peer_certificate()).map(|d| d.to_vec());
        let Some(der) = der else { anyhow::bail!("router sent no TLS certificate") };
        let fp = fingerprint(&der);
        if c.fingerprint.is_empty() {
            c.fingerprint = fp;
            return Ok(());
        }
        if c.fingerprint != fp {
            anyhow::bail!("router certificate changed (pinned {}, got {fp}): possible interception; re-pin in Settings if you replaced the certificate", c.fingerprint);
        }
        Ok(())
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> anyhow::Result<Value> {
        if !self.configured() {
            anyhow::bail!("MikroTik router not configured (Settings -> Network)");
        }
        let (user, pass) = (self.cfg.lock().unwrap().user.clone(), self.pass.lock().unwrap().clone());
        let resp = req.basic_auth(user, Some(pass)).send().await?;
        self.check_pin(&resp)?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            anyhow::bail!("router {status}: {}", body.get("detail").or(body.get("message")).and_then(Value::as_str).unwrap_or(""));
        }
        Ok(body)
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<Value> {
        self.send(self.http.get(self.url(path))).await
    }
    pub async fn put(&self, path: &str, body: Value) -> anyhow::Result<Value> {
        self.send(self.http.put(self.url(path)).json(&body)).await
    }
    pub async fn delete(&self, path: &str) -> anyhow::Result<Value> {
        self.send(self.http.delete(self.url(path))).await
    }

    pub async fn block(&self, address: &str, timeout: &str, comment: &str) -> anyhow::Result<Value> {
        self.put("/ip/firewall/address-list", json!({"list": "kagent-block", "address": address, "timeout": timeout, "comment": comment})).await
    }

    pub async fn unblock(&self, address: &str) -> anyhow::Result<usize> {
        let entries = self.get(&format!("/ip/firewall/address-list?list=kagent-block&address={address}")).await?;
        let mut n = 0;
        for e in entries.as_array().into_iter().flatten() {
            if let Some(id) = e.get(".id").and_then(Value::as_str) {
                self.delete(&format!("/ip/firewall/address-list/{id}")).await?;
                n += 1;
            }
        }
        Ok(n)
    }
}

/// Merge DHCP leases and ARP into (mac, ip, hostname).
pub fn lan_devices(leases: &Value, arp: &Value) -> Vec<(String, String, String)> {
    let mut out: std::collections::BTreeMap<String, (String, String)> = Default::default();
    for l in leases.as_array().into_iter().flatten() {
        let mac = l.get("mac-address").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if mac.is_empty() {
            continue;
        }
        let ip = l.get("active-address").or(l.get("address")).and_then(Value::as_str).unwrap_or("").to_string();
        let host = l.get("host-name").and_then(Value::as_str).unwrap_or("").to_string();
        out.insert(mac, (ip, host));
    }
    for a in arp.as_array().into_iter().flatten() {
        let mac = a.get("mac-address").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if mac.is_empty() || a.get("complete").and_then(Value::as_str) == Some("false") {
            continue;
        }
        let ip = a.get("address").and_then(Value::as_str).unwrap_or("").to_string();
        out.entry(mac).or_insert((ip, String::new()));
    }
    out.into_iter().map(|(m, (i, h))| (m, i, h)).collect()
}

/// `login failure for user X from Y via Z` -> (user, ip).
pub fn login_failure(msg: &str) -> Option<(String, String)> {
    let rest = msg.split("login failure for user ").nth(1)?;
    let (user, rest) = rest.split_once(" from ")?;
    let ip = rest.split_whitespace().next()?;
    Some((user.to_string(), ip.to_string()))
}

/// `user X logged in from Y via Z` -> (user, ip).
pub fn login_success(msg: &str) -> Option<(String, String)> {
    let rest = msg.strip_prefix("user ")?;
    let (user, rest) = rest.split_once(" logged in from ")?;
    Some((user.to_string(), rest.split_whitespace().next()?.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_merge_and_log_parsing() {
        let leases = json!([{"mac-address": "AA:BB:CC:00:00:01", "address": "192.168.88.10", "host-name": "phone"}, {"mac-address": ""}]);
        let arp = json!([{"mac-address": "aa:bb:cc:00:00:01", "address": "192.168.88.10"}, {"mac-address": "aa:bb:cc:00:00:02", "address": "192.168.88.11", "complete": "true"}, {"mac-address": "aa:bb:cc:00:00:03", "address": "192.168.88.12", "complete": "false"}]);
        let d = lan_devices(&leases, &arp);
        assert_eq!(d, vec![("aa:bb:cc:00:00:01".into(), "192.168.88.10".into(), "phone".into()), ("aa:bb:cc:00:00:02".into(), "192.168.88.11".into(), String::new())]);
        assert_eq!(login_failure("login failure for user admin from 192.168.88.66 via ssh"), Some(("admin".into(), "192.168.88.66".into())));
        assert_eq!(login_success("user admin2 logged in from 10.0.0.9 via winbox"), Some(("admin2".into(), "10.0.0.9".into())));
        assert!(login_failure("dhcp lease").is_none());
        assert_eq!(fingerprint(b"x").len(), 32 * 3 - 1);
    }
}
