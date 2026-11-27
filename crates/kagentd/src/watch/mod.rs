//! Watchers and alerts (docs/specs/alerts.md). Rules produce hits; the manager applies
//! thresholds, dedupe and cooldown, stores alerts, notifies, and feeds the UI/audit.

mod investigate;
pub mod rules;
pub mod sources;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use kagent_core::protocol::AgentEvent;
pub use rules::{Hit, Severity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertStatus {
    Open,
    Acked,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub id: String,
    pub ts: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub count: u32,
    pub rule: String,
    pub severity: Severity,
    pub status: AlertStatus,
    pub title: String,
    pub cause: String,
    pub evidence: Value,
    pub proposal: String,
    pub dedupe_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

struct Inner {
    alerts: Vec<Alert>,
    hits: HashMap<String, VecDeque<Instant>>,
    last_raised: HashMap<String, Instant>,
}

pub struct AlertManager {
    path: PathBuf,
    inner: Mutex<Inner>,
    events: broadcast::Sender<AgentEvent>,
    audit: Option<Arc<kagent_core::audit::AuditLog>>,
    notify: bool,
    quiet: Mutex<Option<(u32, u32)>>,
    /// The Telegram push (bots.md section 5); `None` until the bots module registers it.
    telegram: Mutex<Option<Arc<dyn AlertPush>>>,
    /// The hooks of the `AlertRaised` event (hooks.md); `None` until the hooks hub registers.
    hooks: Mutex<Option<Arc<dyn AlertPush>>>,
}

const MAX_ALERTS: usize = 500;

/// What the Telegram push of alerts needs; the bots module registers it.
pub trait AlertPush: Send + Sync {
    /// An alert was raised, or its severity rose: the cases the desktop is told of.
    fn raised(&self, alert: &Alert);
}

/// "23:00-07:00" -> minutes-of-day range.
pub fn parse_quiet(s: &str) -> Option<(u32, u32)> {
    let (a, b) = s.split_once('-')?;
    let m = |t: &str| -> Option<u32> {
        let (h, mi) = t.trim().split_once(':')?;
        Some(h.parse::<u32>().ok()? * 60 + mi.parse::<u32>().ok()?)
    };
    Some((m(a)?, m(b)?))
}

pub fn in_quiet(range: Option<(u32, u32)>, minute_of_day: u32) -> bool {
    match range {
        Some((a, b)) if a <= b => minute_of_day >= a && minute_of_day < b,
        Some((a, b)) => minute_of_day >= a || minute_of_day < b,
        None => false,
    }
}

impl AlertManager {
    pub fn new(path: PathBuf, events: broadcast::Sender<AgentEvent>, audit: Option<Arc<kagent_core::audit::AuditLog>>, notify: bool) -> Arc<Self> {
        let alerts: Vec<Alert> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Arc::new(Self {
            path,
            inner: Mutex::new(Inner { alerts, hits: HashMap::new(), last_raised: HashMap::new() }),
            events,
            audit,
            notify,
            quiet: Mutex::new(parse_quiet("23:00-07:00")),
            telegram: Mutex::new(None),
            hooks: Mutex::new(None),
        })
    }

    /// Register the Telegram push (S29).
    pub fn set_alert_push(&self, push: Arc<dyn AlertPush>) {
        *self.telegram.lock().unwrap() = Some(push);
    }

    /// Register the hooks of the alerts (hooks.md section 4).
    pub fn set_alert_hooks(&self, hooks: Arc<dyn AlertPush>) {
        *self.hooks.lock().unwrap() = Some(hooks);
    }

    pub fn set_quiet_hours(&self, s: &str) {
        *self.quiet.lock().unwrap() = parse_quiet(s);
    }

    pub fn quiet_now(&self) -> bool {
        use chrono::Timelike;
        let now = chrono::Local::now();
        in_quiet(*self.quiet.lock().unwrap(), now.hour() * 60 + now.minute())
    }

    fn save(&self, alerts: &[Alert]) {
        let tmp = self.path.with_extension("tmp");
        if let Ok(b) = serde_json::to_vec_pretty(alerts) {
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn list(&self, open_only: bool, limit: usize) -> Vec<Alert> {
        let g = self.inner.lock().unwrap();
        g.alerts.iter().rev().filter(|a| !open_only || a.status == AlertStatus::Open).take(limit).cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<Alert> {
        self.inner.lock().unwrap().alerts.iter().find(|a| a.id == id).cloned()
    }

    pub fn open_count(&self) -> usize {
        self.inner.lock().unwrap().alerts.iter().filter(|a| a.status == AlertStatus::Open).count()
    }

    pub fn ack(&self, id: &str, note: Option<String>) -> Option<Alert> {
        let mut g = self.inner.lock().unwrap();
        let a = g.alerts.iter_mut().find(|a| a.id == id)?;
        a.status = AlertStatus::Acked;
        a.note = note;
        let out = a.clone();
        let snapshot = g.alerts.clone();
        drop(g);
        self.save(&snapshot);
        let _ = self.events.send(AgentEvent::AlertUpdated { alert: serde_json::to_value(&out).unwrap_or_default() });
        Some(out)
    }

    /// Feed one rule hit. Returns the alert when one was raised or updated.
    pub fn hit(&self, h: Hit) -> Option<Alert> {
        self.hit_at(h, Instant::now())
    }

    pub fn hit_at(&self, h: Hit, now: Instant) -> Option<Alert> {
        let mut g = self.inner.lock().unwrap();
        if let Some((count, window)) = h.threshold {
            let q = g.hits.entry(h.key.clone()).or_default();
            q.push_back(now);
            while q.front().is_some_and(|t| now.duration_since(*t) > window) {
                q.pop_front();
            }
            if (q.len() as u32) < count {
                return None;
            }
        }
        let in_cooldown = g.last_raised.get(&h.key).is_some_and(|t| now.duration_since(*t) < h.cooldown);
        let existing = g.alerts.iter().rposition(|a| a.dedupe_key == h.key && a.status != AlertStatus::Resolved);
        let (alert, is_new) = match existing {
            Some(i) if in_cooldown || g.alerts[i].status == AlertStatus::Open => {
                let a = &mut g.alerts[i];
                a.count += 1;
                a.last_seen = Utc::now();
                let raised = h.severity > a.severity;
                if raised {
                    a.severity = h.severity;
                    a.title = h.title.clone();
                }
                (a.clone(), raised)
            }
            _ => {
                let a = Alert {
                    id: format!("alr_{}_{}", Utc::now().format("%Y%m%d_%H%M%S"), &uuid::Uuid::new_v4().simple().to_string()[..6]),
                    ts: Utc::now(),
                    last_seen: Utc::now(),
                    count: h.threshold.map(|t| t.0).unwrap_or(1),
                    rule: h.rule.into(),
                    severity: h.severity,
                    status: AlertStatus::Open,
                    title: h.title.clone(),
                    cause: h.cause.clone(),
                    evidence: rules::evidence_json(&h),
                    proposal: h.proposal.clone(),
                    dedupe_key: h.key.clone(),
                    note: None,
                };
                g.alerts.push(a.clone());
                if g.alerts.len() > MAX_ALERTS {
                    let n = g.alerts.len() - MAX_ALERTS;
                    g.alerts.drain(..n);
                }
                g.last_raised.insert(h.key.clone(), now);
                (a, true)
            }
        };
        if let Some(q) = g.hits.get_mut(&h.key) {
            q.clear();
        }
        let snapshot = g.alerts.clone();
        drop(g);
        self.save(&snapshot);
        let v = serde_json::to_value(&alert).unwrap_or_default();
        if is_new {
            let _ = self.events.send(AgentEvent::AlertRaised { alert: v });
            if let Some(a) = &self.audit {
                let _ = a.append(kagent_core::audit::AuditKind::Alert, "watch", json!({"id": alert.id, "rule": alert.rule, "severity": alert.severity, "title": alert.title}));
            }
            self.desktop_notify(&alert);
            if let Some(push) = self.telegram.lock().unwrap().clone() {
                push.raised(&alert);
            }
            if let Some(hooks) = self.hooks.lock().unwrap().clone() {
                hooks.raised(&alert);
            }
        } else {
            let _ = self.events.send(AgentEvent::AlertUpdated { alert: v });
        }
        Some(alert)
    }

    fn desktop_notify(&self, a: &Alert) {
        if !self.notify || a.severity == Severity::Info || (a.severity == Severity::Review && self.quiet_now()) {
            return;
        }
        let urgency = if a.severity == Severity::High { "critical" } else { "normal" };
        let _ = std::process::Command::new("notify-send")
            .args(["-a", "KAgent", "-i", "dialog-warning", "-u", urgency, &a.title, &format!("{}\n{}", a.cause, a.proposal)])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

// ---------------------------------------------------------------- HTTP + tool

pub mod http {
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::extract::{Path, Query, State};
    use axum::response::{IntoResponse, Response};
    use axum::Json;
    use serde_json::{json, Value};

    use crate::AppState;

    pub async fn list(State(st): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Response {
        let open = q.get("status").map(|s| s == "open").unwrap_or(false);
        let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100);
        Json(st.alerts.list(open, limit)).into_response()
    }

    pub async fn ack(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: Option<Json<Value>>) -> Response {
        let note = body.and_then(|b| b.get("note").and_then(Value::as_str).map(String::from));
        match st.alerts.ack(&id, note) {
            Some(a) => Json(a).into_response(),
            None => (axum::http::StatusCode::NOT_FOUND, Json(json!({"error": {"code": "not_found", "message": "no such alert"}}))).into_response(),
        }
    }

    /// New session seeded with the alert, turn started: "look into this".
    pub async fn investigate(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
        let Some(a) = st.alerts.get(&id) else {
            return (axum::http::StatusCode::NOT_FOUND, Json(json!({"error": {"code": "not_found", "message": "no such alert"}}))).into_response();
        };
        let meta = match st.agent.store().create_with(&format!("Alert: {}", a.title), kagent_core::session::SessionOrigin::Alert, None).await {
            Ok(m) => m,
            Err(e) => return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"code": "internal", "message": e.to_string()}}))).into_response(),
        };
        let prompt = super::investigate::prompt(&a);
        let _ = st.agent.send(&meta.id, prompt, kagent_core::protocol::Via::Text).await;
        Json(json!({"sessionId": meta.id})).into_response()
    }
}

/// `list_alerts` tool: the agent can see what the watchers found.
pub struct ListAlerts {
    contract: kagent_core::tool::ToolContract,
    alerts: Arc<AlertManager>,
}

impl ListAlerts {
    pub fn new(alerts: Arc<AlertManager>) -> Self {
        Self {
            contract: kagent_core::tool::ToolContract::read_only(
                "list_alerts",
                "Alerts raised by KAgent's watchers (OOM, kernel warnings, failed units, disk, heat, SSH attacks, intrusion tripwires, network).",
                json!({"type":"object","properties":{"status":{"type":"string","enum":["open","all"]},"limit":{"type":"integer"}}}),
            ),
            alerts,
        }
    }
}

#[async_trait::async_trait]
impl kagent_core::tool::Tool for ListAlerts {
    fn contract(&self) -> &kagent_core::tool::ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, _ctx: kagent_core::tool::ToolContext) -> anyhow::Result<kagent_core::tool::ToolOutput> {
        let open = input.get("status").and_then(Value::as_str) != Some("all");
        let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(30) as usize;
        let rows: Vec<String> = self
            .alerts
            .list(open, limit)
            .iter()
            .map(|a| format!("{} [{:?}] {} x{} {:?}: {} | {}", a.id, a.severity, a.ts.format("%m-%d %H:%M"), a.count, a.status, a.title, a.proposal))
            .collect();
        Ok(kagent_core::tool::ToolOutput::ok(if rows.is_empty() { "no alerts".into() } else { rows.join("\n") }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn h(key: &str, sev: Severity, threshold: Option<(u32, Duration)>) -> Hit {
        Hit { rule: "r", key: key.into(), severity: sev, title: "t".into(), cause: "c".into(), evidence: "e".into(), proposal: "p".into(), threshold, cooldown: Duration::from_secs(3600) }
    }

    fn mgr() -> (Arc<AlertManager>, tempfile::TempDir) {
        let d = tempfile::tempdir().unwrap();
        let (tx, _) = broadcast::channel(16);
        (AlertManager::new(d.path().join("alerts.json"), tx, None, false), d)
    }

    #[test]
    fn threshold_dedupe_and_escalation() {
        let (m, _d) = mgr();
        let t0 = Instant::now();
        let th = Some((3, Duration::from_secs(60)));
        assert!(m.hit_at(h("k", Severity::Review, th), t0).is_none());
        assert!(m.hit_at(h("k", Severity::Review, th), t0 + Duration::from_secs(10)).is_none());
        let a = m.hit_at(h("k", Severity::Review, th), t0 + Duration::from_secs(20)).unwrap();
        assert_eq!(a.count, 3);
        // Further hits update the same open alert.
        for i in 0..3 {
            m.hit_at(h("k", Severity::Review, th), t0 + Duration::from_secs(30 + i));
        }
        assert_eq!(m.list(true, 10).len(), 1);
        let a = m.hit_at(h("k", Severity::High, None), t0 + Duration::from_secs(40)).unwrap();
        assert_eq!(a.severity, Severity::High);
        // Window expiry: two old hits do not count.
        assert!(m.hit_at(h("x", Severity::Info, th), t0).is_none());
        assert!(m.hit_at(h("x", Severity::Info, th), t0 + Duration::from_secs(61)).is_none());
        assert!(m.hit_at(h("x", Severity::Info, th), t0 + Duration::from_secs(122)).is_none());
    }

    #[test]
    fn ack_persists_and_cooldown_reopens_later() {
        let (m, d) = mgr();
        let t0 = Instant::now();
        let a = m.hit_at(h("k", Severity::Review, None), t0).unwrap();
        m.ack(&a.id, Some("fine".into())).unwrap();
        // Within cooldown: same (acked) alert is updated, not a new one.
        m.hit_at(h("k", Severity::Review, None), t0 + Duration::from_secs(5));
        assert_eq!(m.list(false, 10).len(), 1);
        // After cooldown: a fresh alert.
        m.hit_at(h("k", Severity::Review, None), t0 + Duration::from_secs(4000));
        assert_eq!(m.list(true, 10).len(), 1);
        assert_eq!(m.list(false, 10).len(), 2);
        let (tx, _) = broadcast::channel(4);
        assert_eq!(AlertManager::new(d.path().join("alerts.json"), tx, None, false).list(false, 10).len(), 2);
    }

    #[test]
    fn quiet_hours() {
        let q = parse_quiet("23:00-07:00");
        assert!(in_quiet(q, 23 * 60 + 30) && in_quiet(q, 60) && !in_quiet(q, 12 * 60));
        let d = parse_quiet("09:00-17:00");
        assert!(in_quiet(d, 10 * 60) && !in_quiet(d, 18 * 60));
        assert!(parse_quiet("nope").is_none());
    }
}
