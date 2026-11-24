//! KRunner integration (KDE Plasma Alt+Space): KAgent serves `org.kde.krunner1` on the
//! session bus. Queries start with "ka " (or "kagent "):
//!   ka <question>   -> asks the agent; the answer arrives as a notification
//!   ka doctor       -> instant health score (no model)
//!   ka stop         -> stop everything
//!   ka open         -> open the KAgent UI
//! Registration file: scripts/kde-integrate.sh (~/.local/share/krunner/dbusplugins).

use std::collections::HashMap;
use std::sync::Arc;

use zbus::OwnedVariant;

use kagent_core::protocol::{AgentEvent, Role, TurnStatus, Via};

use crate::AppState;

pub const SERVICE: &str = "org.kagent.KRunner";
pub const PATH: &str = "/runner";

type Match = (String, String, String, i32, f64, HashMap<String, OwnedVariant>);

/// Parse a runner query: `Some(rest)` when it is addressed to KAgent.
pub fn addressed(query: &str) -> Option<&str> {
    let q = query.trim_start();
    for p in ["kagent ", "ka "] {
        if q.len() > p.len() && q[..p.len()].eq_ignore_ascii_case(p) {
            return Some(q[p.len()..].trim()).filter(|r| !r.is_empty());
        }
    }
    None
}

struct Runner {
    st: Arc<AppState>,
}

fn notify(title: &str, body: &str, urgency: &str) {
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "KAgent", "-i", "audio-input-microphone", "-u", urgency, title, body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn open_ui(st: &AppState, session: Option<&str>) {
    let url = format!("http://127.0.0.1:7878/#token={}{}", st.token, session.map(|s| format!("&session={s}")).unwrap_or_default());
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[zbus::interface(name = "org.kde.krunner1")]
impl Runner {
    async fn actions(&self) -> Vec<(String, String, String)> {
        vec![("open".into(), "Open in KAgent".into(), "internet-web-browser".into())]
    }

    #[zbus(name = "Match")]
    async fn match_(&self, query: String) -> Vec<Match> {
        let Some(rest) = addressed(&query) else { return vec![] };
        let (text, icon) = match rest.to_lowercase().as_str() {
            "doctor" | "health" => ("KAgent: health check".to_string(), "checkmark"),
            "stop" => ("KAgent: stop everything".to_string(), "process-stop"),
            "open" => ("KAgent: open".to_string(), "internet-web-browser"),
            _ => (format!("Ask KAgent: {rest}"), "audio-input-microphone"),
        };
        let mut props = HashMap::new();
        if let Ok(v) = OwnedVariant::try_from(zbus::Variant::from("answer arrives as a notification")) {
            props.insert("subtext".to_string(), v);
        }
        // type 100 = ExactMatch: shown at the top.
        vec![(rest.to_string(), text, icon.to_string(), 100, 1.0, props)]
    }

    async fn run(&self, match_id: String, action_id: String) {
        let st = self.st.clone();
        let cmd = match_id.to_lowercase();
        match cmd.as_str() {
            "stop" => {
                let n = st.agent.stop_all().await;
                notify("KAgent", &format!("Stopped {n} running task(s)."), "normal");
            }
            "open" => open_ui(&st, None),
            "doctor" | "health" => {
                let ctx = kagent_core::tool::ToolContext::standalone(std::time::Duration::from_secs(30), 24_000);
                let r = kagent_core::tools::timeline::checkup(&ctx).await;
                let issues: Vec<String> = r["checks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["status"] != "ok")
                    .map(|c| format!("{}: {}", c["name"].as_str().unwrap_or(""), c["detail"].as_str().unwrap_or("")))
                    .collect();
                notify(&format!("Health {}/100", r["score"]), &if issues.is_empty() { "All checks OK.".into() } else { issues.join("\n") }, "normal");
            }
            _ => {
                tokio::spawn(ask(st, match_id, action_id == "open"));
            }
        }
    }
}

/// New session, send the question, notify with the answer when the turn ends.
async fn ask(st: Arc<AppState>, question: String, open: bool) {
    let meta = match st.agent.store().create(&question).await {
        Ok(m) => m,
        Err(e) => return notify("KAgent", &format!("could not start: {e}"), "critical"),
    };
    let mut rx = st.events.subscribe();
    if let Err(e) = st.agent.send(&meta.id, question.clone(), Via::Text).await {
        return notify("KAgent", &format!("could not start: {e}"), "critical");
    }
    if open {
        open_ui(&st, Some(&meta.id));
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        let ev = tokio::select! {
            ev = rx.recv() => ev,
            _ = tokio::time::sleep_until(deadline) => return,
        };
        match ev {
            Ok(AgentEvent::ApprovalRequested(req)) if req.session_id == meta.id => {
                notify("KAgent needs your approval", &format!("{}\nOpen KAgent to allow or deny.", req.summary), "critical");
            }
            Ok(AgentEvent::TurnFinished { session_id, status, error, .. }) if session_id == meta.id => {
                let answer = st
                    .agent
                    .messages(&meta.id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant && !m.content.trim().is_empty())
                    .map(|m| m.content)
                    .unwrap_or_default();
                let body = match status {
                    TurnStatus::Completed => kagent_core::util::truncate_str(&answer, 600).to_string(),
                    _ => error.unwrap_or_else(|| "stopped".into()),
                };
                notify(&format!("KAgent: {}", kagent_core::util::truncate_str(&question, 60)), &body, "normal");
                return;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            _ => {}
        }
    }
}

/// Claim the bus name and serve. Failure (no session bus) is logged, not fatal.
pub async fn serve(st: Arc<AppState>) -> Option<zbus::Connection> {
    let conn = zbus::connection::Builder::session().name(SERVICE).serve_at(PATH, Runner { st }).build().await;
    match conn {
        Ok(c) => {
            tracing::info!("KRunner plugin on D-Bus {SERVICE}{PATH} (\"ka <question>\" in Alt+Space)");
            Some(c)
        }
        Err(e) => {
            tracing::warn!("KRunner D-Bus service unavailable: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::addressed;

    #[test]
    fn query_prefixes() {
        assert_eq!(addressed("ka why is my fan loud"), Some("why is my fan loud"));
        assert_eq!(addressed("KAgent doctor"), Some("doctor"));
        assert_eq!(addressed("kate"), None);
        assert_eq!(addressed("ka "), None);
        assert_eq!(addressed("firefox"), None);
    }
}
