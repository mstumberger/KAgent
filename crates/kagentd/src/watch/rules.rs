//! Pure alert rules (docs/specs/alerts.md section 3): journal entries and command
//! outputs in, alert hits out. No I/O, so every rule has fixtures.

use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Review,
    High,
}

/// A rule match. The manager turns hits into alerts (threshold, dedupe, cooldown).
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub rule: &'static str,
    pub key: String,
    pub severity: Severity,
    pub title: String,
    pub cause: String,
    pub evidence: String,
    pub proposal: String,
    /// Raise only after `count` hits within `window`.
    pub threshold: Option<(u32, Duration)>,
    pub cooldown: Duration,
}

/// The journal fields rules look at (`journalctl -o json`).
#[derive(Debug, Clone, Default)]
pub struct Entry {
    pub message: String,
    pub unit: String,
    pub ident: String,
    pub kernel: bool,
}

impl Entry {
    pub fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let message = match v.get("MESSAGE") {
            Some(Value::String(m)) => m.clone(),
            // Binary messages come as byte arrays.
            Some(Value::Array(a)) => String::from_utf8_lossy(&a.iter().filter_map(|b| b.as_u64().map(|b| b as u8)).collect::<Vec<_>>()).into_owned(),
            _ => return None,
        };
        Some(Self { message, unit: s("_SYSTEMD_UNIT"), ident: s("SYSLOG_IDENTIFIER"), kernel: s("_TRANSPORT") == "kernel" })
    }
}

macro_rules! re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| Regex::new($pat).unwrap())
        }
    };
}
re!(re_oom, r"Out of memory: Killed process (\d+) \(([^)]+)\)");
re!(re_oom2, r"oom-kill:.*task=([^,]+),pid=(\d+)");
re!(re_kwarn, r"^(WARNING:|BUG:|kernel BUG|general protection fault|Oops)");
re!(re_segv, r"(\S+)\[(\d+)\]: segfault at");
re!(re_ssh_fail, r"(?:Failed password for (?:invalid user )?(\S+)|Invalid user (\S+)) from (\S+)");
re!(re_ssh_ok, r"Accepted (?:password|publickey|keyboard-interactive/pam) for (\S+) from (\S+)");
re!(re_aa, r#"apparmor="DENIED".*?profile="([^"]+)""#);

const H: Duration = Duration::from_secs(3600);
const M: Duration = Duration::from_secs(60);

fn hit(rule: &'static str, key: String, severity: Severity, title: String, cause: &str, evidence: &str, proposal: &str, threshold: Option<(u32, Duration)>, cooldown: Duration) -> Hit {
    Hit { rule, key, severity, title, cause: cause.into(), evidence: evidence.into(), proposal: proposal.into(), threshold, cooldown }
}

/// Journal-stream rules. `quiet` = inside quiet hours (owner likely away/asleep).
pub fn journal_rules(e: &Entry, quiet: bool) -> Vec<Hit> {
    let mut out = Vec::new();
    let m = e.message.as_str();
    if e.kernel {
        if let Some(c) = re_oom().captures(m) {
            let comm = c[2].to_string();
            out.push(hit("oom-kill", format!("oom:{comm}"), Severity::Review, format!("Out of memory: the kernel killed {comm} (pid {})", &c[1]), "RAM and swap were exhausted", m, "check memory hogs with process_list sort=mem; consider more swap/zram or limiting the culprit", None, H));
        } else if let Some(c) = re_oom2().captures(m) {
            let comm = c[1].to_string();
            out.push(hit("oom-kill", format!("oom:{comm}"), Severity::Review, format!("Out of memory: {comm} was killed"), "RAM and swap were exhausted", m, "check memory hogs with process_list sort=mem", None, H));
        }
        if re_kwarn().is_match(m) {
            let head: String = m.chars().take(60).collect();
            out.push(hit("kernel-warning", format!("kwarn:{head}"), Severity::Review, format!("Kernel problem: {head}"), "the kernel logged a warning/bug/oops", m, "inspect kernel_info section=log around this time; check drivers and taint", None, 6 * H));
        }
        if let Some(c) = re_segv().captures(m) {
            let comm = c[1].to_string();
            out.push(hit("segfault", format!("segv:{comm}"), Severity::Info, format!("{comm} keeps crashing (segfault)"), "repeated segmentation faults", m, "look at crashes for coredumps of this program", Some((3, 10 * M)), 6 * H));
        }
        if let Some(c) = re_aa().captures(m) {
            let p = c[1].to_string();
            out.push(hit("apparmor-denied", format!("aa:{p}"), Severity::Info, format!("AppArmor is blocking {p}"), "repeated AppArmor denials", m, "check whether the profile is too strict or the program misbehaves", Some((5, 10 * M)), 6 * H));
        }
    }
    let ssh = e.ident == "sshd" || e.unit.starts_with("ssh");
    if ssh {
        if let Some(c) = re_ssh_fail().captures(m) {
            let ip = c[3].to_string();
            let sev = if is_private(&ip) { Severity::Review } else { Severity::High };
            out.push(hit("ssh-bruteforce", format!("ssh-bf:{ip}"), sev, format!("Repeated failed SSH logins from {ip}"), "password guessing against sshd", m, &format!("block {ip} (router_block_ip if a MikroTik is configured, or ufw deny from {ip}); disable password auth in sshd"), Some((10, 5 * M)), H));
        }
        if let Some(c) = re_ssh_ok().captures(m) {
            if quiet {
                let (user, ip) = (c[1].to_string(), c[2].to_string());
                out.push(hit("ssh-login", format!("ssh-in:{ip}:{user}"), Severity::Review, format!("SSH login as {user} from {ip} during quiet hours"), "a successful login while you are usually away", m, "if this was not you: kill the session, rotate keys, check persistence alerts", None, Duration::ZERO));
            }
        }
    }
    out
}

fn is_private(ip: &str) -> bool {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v)) => v.is_private() || v.is_loopback() || v.is_link_local(),
        Ok(std::net::IpAddr::V6(v)) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// `systemctl --failed --output=json` -> failed unit names.
pub fn failed_units(json_out: &str) -> Vec<String> {
    serde_json::from_str::<Value>(json_out).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default().iter().filter_map(|u| u.get("unit").and_then(Value::as_str).map(String::from)).collect()
}

/// Unit name -> stable template for grouping: instance ids dropped, systemd escapes
/// decoded, app launcher paths reduced to the program name.
/// `app-\x2fusr\x2flib\x2fdrkonqi\x2dcoredump\x2dlauncher@abc.service` -> `drkonqi-coredump-launcher`.
pub fn unit_template(unit: &str) -> String {
    let base = unit.split('@').next().unwrap_or(unit).trim_end_matches(".service");
    let mut out = String::new();
    let mut chars = base.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'x') {
            chars.next();
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(b) = u8::from_str_radix(&hex, 16) {
                out.push(b as char);
                continue;
            }
        }
        out.push(c);
    }
    let out = out.strip_prefix("app-").unwrap_or(&out).to_string();
    out.rsplit('/').next().unwrap_or(&out).to_string()
}

/// Failed units grouped by template: (template, instances, example unit).
pub fn group_failed(units: &[String]) -> Vec<(String, usize, String)> {
    let mut map: std::collections::BTreeMap<String, (usize, String)> = Default::default();
    for u in units {
        let e = map.entry(unit_template(u)).or_insert((0, u.clone()));
        e.0 += 1;
    }
    map.into_iter().map(|(t, (n, ex))| (t, n, ex)).collect()
}

/// `df -P --output=target,pcent,fstype` -> (mount, percent) for real filesystems.
pub fn disk_usage(df: &str) -> Vec<(String, u32)> {
    df.lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let (mount, pct, fs) = (f.first()?, f.get(1)?, f.get(2)?);
            if !["ext4", "ext3", "btrfs", "xfs", "vfat", "f2fs", "zfs"].contains(fs) {
                return None;
            }
            Some((mount.to_string(), pct.trim_end_matches('%').parse().ok()?))
        })
        .collect()
}

pub fn disk_hit(mount: &str, pct: u32) -> Option<Hit> {
    let sev = if pct >= 97 { Severity::High } else if pct >= 90 { Severity::Review } else { return None };
    Some(hit("disk-space", format!("disk:{mount}"), sev, format!("{mount} is {pct}% full"), "low free space breaks updates, logs and apps", &format!("{mount} {pct}%"), "run the disk-full playbook: biggest dirs, journal vacuum, apt clean, old snaps", None, 12 * H))
}

/// `sensors -j` -> (label, temp, high) for every temperature input.
pub fn temperatures(sensors_json: &str) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    let Ok(Value::Object(chips)) = serde_json::from_str::<Value>(sensors_json) else { return out };
    for (chip, v) in chips {
        let Value::Object(feats) = v else { continue };
        for (label, f) in feats {
            let Value::Object(f) = f else { continue };
            let input = f.iter().find(|(k, _)| k.starts_with("temp") && k.ends_with("_input")).and_then(|(_, v)| v.as_f64());
            let high = f.iter().find(|(k, _)| k.starts_with("temp") && (k.ends_with("_max") || k.ends_with("_high"))).and_then(|(_, v)| v.as_f64()).filter(|h| *h > 20.0 && *h < 150.0);
            if let Some(t) = input {
                out.push((format!("{chip}/{label}"), t, high.unwrap_or(90.0)));
            }
        }
    }
    out
}

pub fn thermal_hit(label: &str, temp: f64, high: f64) -> Hit {
    hit("thermal", format!("thermal:{label}"), Severity::Review, format!("{label} is running hot: {temp:.0} C (limit {high:.0} C)"), "sustained high temperature (throttling, fan or dust problem)", &format!("{label} {temp:.1} C"), "check fans and dust, what is using CPU/GPU (process_list), and power_profile", None, H)
}

pub fn evidence_json(h: &Hit) -> Value {
    json!([{"kind": "log", "text": h.evidence}])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kern(m: &str) -> Entry {
        Entry { message: m.into(), kernel: true, ..Default::default() }
    }
    fn sshd(m: &str) -> Entry {
        Entry { message: m.into(), ident: "sshd".into(), unit: "ssh.service".into(), kernel: false }
    }

    #[test]
    fn journal_entry_parsing() {
        let v = serde_json::json!({"MESSAGE": [104, 105], "_TRANSPORT": "kernel"});
        let e = Entry::from_json(&v).unwrap();
        assert_eq!(e.message, "hi");
        assert!(e.kernel);
        assert!(Entry::from_json(&serde_json::json!({})).is_none());
    }

    #[test]
    fn kernel_rules() {
        let h = journal_rules(&kern("Out of memory: Killed process 4242 (chrome) total-vm:1kB"), false);
        assert_eq!((h[0].rule, h[0].key.as_str()), ("oom-kill", "oom:chrome"));
        assert_eq!(journal_rules(&kern("WARNING: CPU: 3 PID: 1 at drivers/x.c:10"), false)[0].rule, "kernel-warning");
        let s = journal_rules(&kern("firefox[1234]: segfault at 0 ip 0 sp 0 error 4"), false);
        assert_eq!((s[0].key.as_str(), s[0].threshold.unwrap().0), ("segv:firefox", 3));
        assert_eq!(journal_rules(&kern(r#"audit: apparmor="DENIED" operation="open" profile="snap.x" name="/y""#), false)[0].key, "aa:snap.x");
        assert!(journal_rules(&kern("usb 1-1: new device"), false).is_empty());
        // Kernel patterns in user-space messages are ignored.
        assert!(journal_rules(&Entry { message: "WARNING: foo".into(), ..Default::default() }, false).is_empty());
    }

    #[test]
    fn ssh_rules() {
        let h = journal_rules(&sshd("Failed password for invalid user admin from 203.0.113.9 port 4242 ssh2"), false);
        assert_eq!((h[0].key.as_str(), h[0].severity, h[0].threshold.unwrap().0), ("ssh-bf:203.0.113.9", Severity::High, 10));
        assert_eq!(journal_rules(&sshd("Invalid user test from 192.168.1.5 port 1"), false)[0].severity, Severity::Review);
        let ok = "Accepted publickey for marko from 10.0.0.2 port 5 ssh2: ED25519";
        assert!(journal_rules(&sshd(ok), false).is_empty());
        assert_eq!(journal_rules(&sshd(ok), true)[0].rule, "ssh-login");
    }

    #[test]
    fn unit_grouping() {
        let a = r"app-\x2fusr\x2flib\x2fx86_64\x2dlinux\x2dgnu\x2flibexec\x2fdrkonqi\x2dcoredump\x2dlauncher@e9d5a.service";
        assert_eq!(unit_template(a), "drkonqi-coredump-launcher");
        assert_eq!(unit_template("nginx.service"), "nginx");
        assert_eq!(unit_template("getty@tty1.service"), "getty");
        let g = group_failed(&[a.to_string(), a.replace("e9d5a", "ffff"), "nginx.service".into()]);
        assert_eq!(g.len(), 2);
        assert!(g.iter().any(|(t, n, _)| t == "drkonqi-coredump-launcher" && *n == 2));
    }

    #[test]
    fn poll_parsers() {
        assert_eq!(failed_units(r#"[{"unit":"foo.service","load":"loaded"}]"#), vec!["foo.service"]);
        assert!(failed_units("[]").is_empty());
        let df = "Mounted on Use% Type\n/ 93% ext4\n/run 1% tmpfs\n/boot/efi 6% vfat\n";
        assert_eq!(disk_usage(df), vec![("/".into(), 93), ("/boot/efi".into(), 6)]);
        assert_eq!(disk_hit("/", 93).unwrap().severity, Severity::Review);
        assert_eq!(disk_hit("/", 98).unwrap().severity, Severity::High);
        assert!(disk_hit("/", 80).is_none());
        let sj = r#"{"k10temp-pci-00c3":{"Adapter":"PCI adapter","Tctl":{"temp1_input":71.5}},"nvme-pci":{"Composite":{"temp1_input":45.0,"temp1_max":84.85}}}"#;
        let t = temperatures(sj);
        assert!(t.contains(&("k10temp-pci-00c3/Tctl".into(), 71.5, 90.0)));
        assert!(t.contains(&("nvme-pci/Composite".into(), 45.0, 84.85)));
    }
}
