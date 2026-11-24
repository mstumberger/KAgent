//! Traffic store (docs/specs/config.md section 10) and traffic rules
//! (docs/specs/alerts.md section 5). Flows aggregate per LAN device per 5 minutes.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};

use super::flows::{is_lan, Flow};
use crate::watch::{Hit, Severity};

pub const BUCKET: i64 = 300;
const MIB: u64 = 1024 * 1024;

/// One bucket's traffic per (device, remote, dport, proto) -> (bytes, packets).
pub type Agg = HashMap<(String, String, u16, u8), (u64, u64)>;

/// Attribute a flow to its LAN side: (device, remote). Pure LAN<->LAN flows keep src
/// as device (needed for inside port-scan detection).
pub fn attribute(f: &Flow) -> Option<(IpAddr, IpAddr)> {
    match (is_lan(&f.src), is_lan(&f.dst)) {
        (true, _) => Some((f.src, f.dst)),
        (false, true) => Some((f.dst, f.src)),
        _ => None, // transit traffic: not ours
    }
}

pub fn add_flow(agg: &mut Agg, f: &Flow) {
    if let Some((dev, remote)) = attribute(f) {
        let port = if dev == f.src { f.dport } else { f.sport };
        let e = agg.entry((dev.to_string(), remote.to_string(), port, f.proto)).or_default();
        e.0 += f.bytes;
        e.1 += f.packets;
    }
}

pub struct FlowStore {
    conn: Mutex<Connection>,
}

impl FlowStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS devices(mac TEXT PRIMARY KEY, first_seen INTEGER, last_seen INTEGER, last_ip TEXT, hostname TEXT, vendor TEXT, note TEXT);
             CREATE TABLE IF NOT EXISTS flows_5m(bucket INTEGER, src TEXT, dst TEXT, dport INTEGER, proto INTEGER, bytes INTEGER, packets INTEGER, PRIMARY KEY(bucket, src, dst, dport, proto));
             CREATE TABLE IF NOT EXISTS dest_seen(src TEXT, dst TEXT, first_seen INTEGER, PRIMARY KEY(src, dst));
             CREATE INDEX IF NOT EXISTS flows_src ON flows_5m(src, bucket);",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn ingest(&self, bucket: i64, agg: &Agg) -> anyhow::Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO flows_5m(bucket,src,dst,dport,proto,bytes,packets) VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(bucket,src,dst,dport,proto) DO UPDATE SET bytes=bytes+excluded.bytes, packets=packets+excluded.packets",
            )?;
            for ((src, dst, dport, proto), (b, p)) in agg {
                st.execute(params![bucket, src, dst, dport, proto, *b as i64, *p as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns true when the MAC was never seen before.
    pub fn upsert_device(&self, mac: &str, ip: &str, hostname: &str, now: i64) -> anyhow::Result<bool> {
        let c = self.conn.lock().unwrap();
        let existed: Option<i64> = c.query_row("SELECT first_seen FROM devices WHERE mac=?1", [mac], |r| r.get(0)).optional()?;
        c.execute(
            "INSERT INTO devices(mac,first_seen,last_seen,last_ip,hostname) VALUES(?1,?2,?2,?3,?4)
             ON CONFLICT(mac) DO UPDATE SET last_seen=?2, last_ip=?3, hostname=CASE WHEN ?4='' THEN hostname ELSE ?4 END",
            params![mac, now, ip, hostname],
        )?;
        Ok(existed.is_none())
    }

    pub fn device_count(&self) -> i64 {
        self.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0)).unwrap_or(0)
    }

    pub fn devices(&self) -> Vec<serde_json::Value> {
        let c = self.conn.lock().unwrap();
        let Ok(mut st) = c.prepare("SELECT mac,first_seen,last_seen,last_ip,hostname FROM devices ORDER BY last_seen DESC") else { return vec![] };
        st.query_map([], |r| {
            Ok(serde_json::json!({"mac": r.get::<_, String>(0)?, "firstSeen": r.get::<_, i64>(1)?, "lastSeen": r.get::<_, i64>(2)?, "ip": r.get::<_, String>(3)?, "hostname": r.get::<_, Option<String>>(4)?.unwrap_or_default()}))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    /// true when this device never talked to this /24 (v4) or /48 (v6) before.
    pub fn first_contact(&self, device: &str, remote_net: &str, now: i64) -> bool {
        let c = self.conn.lock().unwrap();
        c.execute("INSERT OR IGNORE INTO dest_seen(src,dst,first_seen) VALUES(?1,?2,?3)", params![device, remote_net, now]).map(|n| n == 1).unwrap_or(false)
    }

    /// Mean bytes per active bucket for a device over the last 7 days before `bucket`.
    pub fn baseline(&self, device: &str, bucket: i64) -> f64 {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT AVG(t) FROM (SELECT SUM(bytes) t FROM flows_5m WHERE src=?1 AND bucket<?2 AND bucket>=?3 GROUP BY bucket)",
            params![device, bucket, bucket - 7 * 86400],
            |r| r.get::<_, Option<f64>>(0),
        )
        .ok()
        .flatten()
        .unwrap_or(0.0)
    }

    pub fn top(&self, device: Option<&str>, since_bucket: i64) -> Vec<serde_json::Value> {
        let c = self.conn.lock().unwrap();
        let sql = "SELECT src,dst,dport,proto,SUM(bytes) b FROM flows_5m WHERE bucket>=?1 AND (?2='' OR src=?2) GROUP BY src,dst,dport,proto ORDER BY b DESC LIMIT 40";
        let Ok(mut st) = c.prepare(sql) else { return vec![] };
        st.query_map(params![since_bucket, device.unwrap_or("")], |r| {
            Ok(serde_json::json!({"device": r.get::<_, String>(0)?, "remote": r.get::<_, String>(1)?, "port": r.get::<_, i64>(2)?, "proto": r.get::<_, i64>(3)?, "bytes": r.get::<_, i64>(4)?}))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn prune(&self, now: i64) {
        let _ = self.conn.lock().unwrap().execute("DELETE FROM flows_5m WHERE bucket < ?1", [now - 14 * 86400]);
    }
}

fn net_of(ip: &str) -> String {
    match ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(v)) => {
            let o = v.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        Ok(IpAddr::V6(v)) => {
            let s = v.segments();
            format!("{:x}:{:x}:{:x}::/48", s[0], s[1], s[2])
        }
        Err(_) => ip.to_string(),
    }
}

fn hit(rule: &'static str, key: String, severity: Severity, title: String, cause: &str, evidence: String, proposal: &str, cooldown_h: u64) -> Hit {
    Hit { rule, key, severity, title, cause: cause.into(), evidence, proposal: proposal.into(), threshold: None, cooldown: Duration::from_secs(cooldown_h * 3600) }
}

/// Traffic rules over one completed bucket. `local_hour` decides "night".
pub fn bucket_rules(store: &FlowStore, bucket: i64, agg: &Agg, local_hour: u32, honeypot: Option<&str>) -> Vec<Hit> {
    let mut out = Vec::new();
    let mut per_dev: HashMap<&str, (u64, HashSet<u16>, HashSet<&str>)> = HashMap::new();
    for ((dev, remote, port, _), (bytes, _)) in agg {
        let e = per_dev.entry(dev).or_default();
        e.0 += bytes;
        e.1.insert(*port);
        if remote.parse::<IpAddr>().is_ok_and(|r| is_lan(&r)) {
            e.2.insert(remote);
        }
        if honeypot == Some(remote.as_str()) {
            out.push(hit("flow-honeypot-touch", format!("canary:{dev}"), Severity::High, format!("{dev} connected to the honeypot ({remote}:{port})"), "nothing legitimate talks to the honeypot", format!("{dev} -> {remote}:{port}, {bytes} B"), "find the device (lan_devices), isolate it, check it for malware", 1));
        }
        let night = (1..6).contains(&local_hour);
        let public = remote.parse::<IpAddr>().is_ok_and(|r| !is_lan(&r));
        if public && store.first_contact(dev, &net_of(remote), bucket) && night && *bytes > MIB {
            out.push(hit("flow-new-destination", format!("newdst:{dev}:{}", net_of(remote)), Severity::Info, format!("{dev} sent {} MiB to a new destination at night ({remote})", bytes / MIB), "first contact with this network, outside normal hours", format!("{dev} -> {remote}:{port}"), "check what the device is; cloud backups and updates also do this", 24));
        }
    }
    for (dev, (bytes, ports, lan_hosts)) in &per_dev {
        if ports.len() >= 50 || lan_hosts.len() >= 30 {
            out.push(hit("flow-port-fanout", format!("fanout:{dev}"), Severity::High, format!("{dev} is scanning: {} ports / {} LAN hosts in 5 minutes", ports.len(), lan_hosts.len()), "port or host scanning from inside the LAN", format!("{} distinct ports, {} LAN hosts", ports.len(), lan_hosts.len()), "identify the device; if it is not you running nmap, isolate it (router_block_ip / quarantine VLAN)", 1));
        }
        let base = store.baseline(dev, bucket);
        if *bytes > 200 * MIB && base > 0.0 && *bytes as f64 > 10.0 * base {
            out.push(hit("flow-volume-spike", format!("spike:{dev}"), Severity::Review, format!("{dev} moved {} MiB in 5 minutes ({}x its normal)", bytes / MIB, (*bytes as f64 / base) as u64), "traffic far above this device's 7-day average", format!("{bytes} B vs avg {base:.0} B"), "traffic_top for this device shows where it went", 6));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(src: &str, dst: &str, sport: u16, dport: u16, bytes: u64) -> Flow {
        Flow { src: src.parse().unwrap(), dst: dst.parse().unwrap(), sport, dport, proto: 6, bytes, packets: 1 }
    }

    #[test]
    fn attribution_and_aggregation() {
        let mut agg = Agg::new();
        add_flow(&mut agg, &f("192.168.88.10", "1.1.1.1", 5000, 443, 100));
        add_flow(&mut agg, &f("1.1.1.1", "192.168.88.10", 443, 5000, 900)); // reply
        add_flow(&mut agg, &f("8.8.8.8", "9.9.9.9", 1, 2, 5)); // transit
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[&("192.168.88.10".into(), "1.1.1.1".into(), 443, 6)].0, 1000);
    }

    #[test]
    fn fanout_honeypot_spike_and_store() {
        let d = tempfile::tempdir().unwrap();
        let s = FlowStore::open(&d.path().join("net.sqlite")).unwrap();
        let mut agg = Agg::new();
        for port in 1..=60 {
            add_flow(&mut agg, &f("192.168.88.66", "192.168.88.1", 40000, port, 60));
        }
        add_flow(&mut agg, &f("192.168.88.20", "192.168.88.250", 4000, 22, 60));
        let hits = bucket_rules(&s, 1000 * BUCKET, &agg, 12, Some("192.168.88.250"));
        let rules: Vec<&str> = hits.iter().map(|h| h.rule).collect();
        assert!(rules.contains(&"flow-port-fanout") && rules.contains(&"flow-honeypot-touch"), "{rules:?}");

        // Baseline then spike.
        let dev = "192.168.88.30";
        for b in 0..10 {
            let mut a = Agg::new();
            add_flow(&mut a, &f(dev, "5.5.5.5", 1, 443, 10 * MIB));
            s.ingest((900 + b) * BUCKET, &a).unwrap();
        }
        let mut big = Agg::new();
        add_flow(&mut big, &f(dev, "5.5.5.5", 1, 443, 300 * MIB));
        let hits = bucket_rules(&s, 1000 * BUCKET, &big, 12, None);
        assert!(hits.iter().any(|h| h.rule == "flow-volume-spike"));
        assert_eq!(s.top(Some(dev), 0)[0]["bytes"], 100 * MIB as i64);
    }

    #[test]
    fn new_destination_only_at_night_and_once() {
        let d = tempfile::tempdir().unwrap();
        let s = FlowStore::open(&d.path().join("net.sqlite")).unwrap();
        let mut agg = Agg::new();
        add_flow(&mut agg, &f("192.168.88.40", "45.1.2.3", 1, 443, 5 * MIB));
        assert!(bucket_rules(&s, BUCKET, &agg, 3, None).iter().any(|h| h.rule == "flow-new-destination"));
        assert!(!bucket_rules(&s, 2 * BUCKET, &agg, 3, None).iter().any(|h| h.rule == "flow-new-destination"));
        assert!(s.upsert_device("aa:bb", "192.168.88.40", "phone", 1).unwrap());
        assert!(!s.upsert_device("aa:bb", "192.168.88.41", "", 2).unwrap());
        assert_eq!(s.devices()[0]["hostname"], "phone");
    }
}
