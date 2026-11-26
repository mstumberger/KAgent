//! Hash-chained audit log (config.md section 6): every tool call, approval and
//! change leaves a record; each line hashes the previous one.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditKind {
    ToolCall,
    Approval,
    Change,
    Undo,
    Alert,
    Containment,
    Start,
    Stop,
    /// A hook command ran (hooks.md section 2).
    Hook,
}

impl AuditKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::ToolCall => "toolCall",
            Self::Approval => "approval",
            Self::Change => "change",
            Self::Undo => "undo",
            Self::Alert => "alert",
            Self::Containment => "containment",
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }
}

/// Input fields whose values are never written in clear.
const REDACT_KEYS: [&str; 5] = ["password", "apikey", "token", "secret", "content"];

pub fn redact_input(input: &Value) -> Value {
    match input {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let lower = k.to_lowercase();
                if REDACT_KEYS.contains(&lower.as_str()) {
                    let prefix = hash_hex(v).chars().take(8).collect::<String>();
                    (k.clone(), json!(format!("<redacted:{prefix}>")))
                } else {
                    (k.clone(), redact_input(v))
                }
            })
            .collect::<serde_json::Map<String, Value>>()
            .into(),
        Value::Array(a) => Value::Array(a.iter().map(redact_input).collect()),
        other => other.clone(),
    }
}

fn hash_hex(v: &Value) -> String {
    let mut h = Sha256::new();
    h.update(v.to_string().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Canonical JSON: keys sorted, no whitespace (serde_json maps are BTreeMap-ordered
/// when using the preserve_order feature off, which is our default).
fn canonical(v: &Value) -> String {
    serde_json::to_string(&sorted(v)).unwrap_or_default()
}

fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                out.insert(k.clone(), sorted(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

pub struct AuditLog {
    file: Mutex<std::fs::File>,
    last_hash: Mutex<String>,
}

impl AuditLog {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let last = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.lines().filter(|l| !l.trim().is_empty()).last().map(str::to_string))
            .and_then(|line| serde_json::from_str::<Value>(&line).ok())
            .and_then(|v| v.get("hash").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| GENESIS.to_string());
        let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file: Mutex::new(file), last_hash: Mutex::new(last) })
    }

    pub fn append(&self, kind: AuditKind, session_id: &str, data: Value) -> anyhow::Result<String> {
        let prev = self.last_hash.lock().unwrap().clone();
        let record = json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "kind": kind.as_str(),
            "sessionId": session_id,
            "data": data,
            "prev": prev,
        });
        let hash = {
            let mut h = Sha256::new();
            h.update(prev.as_bytes());
            h.update(b"\n");
            h.update(canonical(&record).as_bytes());
            h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let mut line = record;
        line["hash"] = json!(hash);
        let mut f = self.file.lock().unwrap();
        writeln!(f, "{}", line)?;
        f.sync_all().ok();
        *self.last_hash.lock().unwrap() = hash.clone();
        Ok(hash)
    }

    /// Convenience for daemon start records.
    pub fn append_start(&self, version: &str) -> anyhow::Result<String> {
        self.append(AuditKind::Start, "", json!({"version": version}))
    }
}

/// Verify the chain; on break, `(line_number, reason)` (1-based).
pub fn verify(path: &Path) -> Result<usize, (usize, String)> {
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let mut prev = GENESIS.to_string();
    let mut count = 0usize;
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        count += 1;
        let v: Value = serde_json::from_str(line).map_err(|e| (i + 1, format!("unparsable line: {e}")))?;
        let hash = v.get("hash").and_then(Value::as_str).ok_or((i + 1, "missing hash".into()))?;
        let prev_field = v.get("prev").and_then(Value::as_str).ok_or((i + 1, "missing prev".into()))?;
        if prev_field != prev {
            return Err((i + 1, format!("prev {prev_field:.12}... does not chain to previous hash")));
        }
        let mut without = v.clone();
        without.as_object_mut().unwrap().remove("hash");
        let mut h = Sha256::new();
        h.update(prev_field.as_bytes());
        h.update(b"\n");
        h.update(canonical(&without).as_bytes());
        let want: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if want != hash {
            return Err((i + 1, "hash mismatch (record tampered)".into()));
        }
        prev = hash.to_string();
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_and_tamper_detection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let log = AuditLog::open(&path).unwrap();
        log.append_start("0.1").unwrap();
        for i in 0..3 {
            log.append(
                AuditKind::ToolCall,
                "s1",
                json!({"callId": format!("c{i}"), "tool": "shell", "input": {"command": "ls"}, "status": "ok"}),
            )
            .unwrap();
        }
        assert_eq!(verify(&path).unwrap(), 4);

        // Tamper with one byte in line 3.
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
        let flip = if lines[2].as_bytes()[60] == b'a' { "b" } else { "a" };
        lines[2].replace_range(60..61, flip);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        match verify(&path) {
            Err((line, _)) => assert_eq!(line, 3),
            Ok(n) => panic!("tamper not detected ({n} records)"),
        }
    }

    #[test]
    fn redaction() {
        let input = json!({"password": "hunter2", "apiKey": "sk-x", "nested": {"token": "t"}, "cmd": "ls"});
        let out = redact_input(&input);
        assert_eq!(out["password"].as_str().unwrap().starts_with("<redacted:"), true);
        assert_eq!(out["apiKey"].as_str().unwrap().starts_with("<redacted:"), true);
        assert_eq!(out["nested"]["token"].as_str().unwrap().starts_with("<redacted:"), true);
        assert_eq!(out["cmd"], "ls");
    }
}
