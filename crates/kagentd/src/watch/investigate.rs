//! The first message of an alert session (alerts.md section 1, `/alerts/:id/investigate`).
//! It is marked as the starting task: a later request of the owner replaces it, so the
//! session does not keep going back to the alert (v0.0.2 section 7).

use super::Alert;

/// The marker the message starts with; the system prompt's rule about the newest request
/// names "the starting task" too (`prompt.rs`).
pub const STARTING_TASK: &str = "[Starting task of this alert session]";

pub fn prompt(a: &Alert) -> String {
    format!(
        "{STARTING_TASK} KAgent raised an alert. Investigate it with read-only tools first, explain the cause in plain words, then propose a fix (do not change anything without asking).\n\nAlert: {}\nRule: {} (severity {:?}, seen {} times)\nCause: {}\nEvidence: {}\nSuggested: {}\n\nThis is only the starting task. When the owner later asks for something else, the newest request wins: do that, and come back to this alert only when the owner asks.",
        a.title, a.rule, a.severity, a.count, a.cause, a.evidence, a.proposal
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alert_prompt_is_the_starting_task_and_yields_to_a_later_request() {
        let alert: Alert = serde_json::from_value(serde_json::json!({
            "id": "a1", "ts": "2026-11-27T10:00:00Z", "lastSeen": "2026-11-27T10:00:00Z", "count": 2,
            "rule": "disk-full", "severity": "review", "status": "open", "title": "Disk / is 95% full",
            "cause": "the root filesystem is almost full", "evidence": [], "proposal": "clean the package cache", "dedupeKey": "disk:/"
        }))
        .unwrap();
        let text = prompt(&alert);
        assert!(text.starts_with(STARTING_TASK), "{text}");
        assert!(text.contains("Alert: Disk / is 95% full\nRule: disk-full (severity Review, seen 2 times)"), "{text}");
        assert!(text.ends_with("the newest request wins: do that, and come back to this alert only when the owner asks."), "{text}");
    }
}
