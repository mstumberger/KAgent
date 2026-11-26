//! The cards the bot sends (bots.md sections 4 and 5): which approval cards may be
//! forwarded, what a card says, and what its buttons answer. Pure.

use kagent_core::permission::{ApprovalKind, ApprovalRequest};
use kagent_core::util::truncate_str;

use crate::watch::{Alert, Severity};

/// Whether an approval card goes to the paired chat (bots.md sections 4 and 8). A plan
/// and a destructive call never do; a root action only with `allow_privileged`.
pub fn forward_rule(card: &ApprovalRequest, allow_privileged: bool) -> Result<(), &'static str> {
    if card.kind == ApprovalKind::Plan {
        return Err("a plan is approved on the desktop only");
    }
    if card.destructive {
        return Err("a destructive call never comes from Telegram");
    }
    if card.privileged && !allow_privileged {
        return Err("a root action is not forwarded while allowPrivileged is off");
    }
    Ok(())
}

/// What the message of an approval card says: the tool, the summary and the risk.
pub fn approval_text(card: &ApprovalRequest) -> String {
    let mut text = format!("KAgent needs your approval\n\nTool: {}\n{}\nRisk: {}", card.tool, card.summary, word(format!("{:?}", card.risk)));
    if !card.reason.trim().is_empty() {
        text.push_str("\nWhy: ");
        text.push_str(card.reason.trim());
    }
    if let Some(preview) = card.preview.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        // What the desktop card shows of the call, cut to leave room for the buttons' message.
        text.push_str("\n\n");
        text.push_str(truncate_str(preview, 800));
    }
    if card.privileged {
        text.push_str("\n\nThis is a root action.");
    }
    text
}

/// A raised alert goes to the chat (bots.md section 5): `high` always, `review` outside
/// the quiet hours, `info` never, and all only while `notify_alerts` is on.
pub fn alert_goes(severity: Severity, notify_alerts: bool, quiet: bool) -> bool {
    notify_alerts
        && match severity {
            Severity::High => true,
            Severity::Review => !quiet,
            Severity::Info => false,
        }
}

/// What the message of an alert says.
pub fn alert_text(alert: &Alert) -> String {
    format!("KAgent alert ({}): {}\n\n{}\nSuggested: {}", word(format!("{:?}", alert.severity)), alert.title, alert.cause, alert.proposal)
}

fn word(name: String) -> String {
    name.to_lowercase()
}

/// What a button of the bot answers. Telegram allows 64 bytes for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    /// A card of `request`: Allow once (`allow`) or Deny.
    Approval { request: String, allow: bool },
    /// Acknowledge the alert.
    Ack { alert: String },
}

/// `callback_data` of the Allow once and Deny buttons of a forwarded card.
pub fn approval_data(request: &str, allow: bool) -> String {
    format!("a:{request}:{}", if allow { "allow" } else { "deny" })
}

/// `callback_data` of the Ack button of a pushed alert.
pub fn ack_data(alert: &str) -> String {
    format!("ack:{alert}")
}

/// The press a button's `callback_data` names; `None` for anything the bot did not send.
pub fn press(data: &str) -> Option<Press> {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    match parts.as_slice() {
        ["a", request, "allow"] if !request.is_empty() => Some(Press::Approval { request: request.to_string(), allow: true }),
        ["a", request, "deny"] if !request.is_empty() => Some(Press::Approval { request: request.to_string(), allow: false }),
        ["ack", alert] if !alert.is_empty() => Some(Press::Ack { alert: alert.to_string() }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::tool::Risk;
    use serde_json::json;

    const REQUEST: &str = "6f1c2a0e-9b7d-4c3e-8a2f-0d1e2f3a4b5c";

    fn card(kind: ApprovalKind, destructive: bool, privileged: bool) -> ApprovalRequest {
        ApprovalRequest {
            request_id: REQUEST.into(),
            session_id: "s1".into(),
            call_id: "c1".into(),
            tool: "shell".into(),
            kind,
            input: json!({"command": "systemctl restart kagent"}),
            summary: "restart the kagent service".into(),
            reason: "changes system state".into(),
            risk: Risk::Medium,
            privileged,
            destructive,
            can_remember: true,
            rule_key: "shell".into(),
            preview: None,
        }
    }

    #[test]
    fn forwarding_rules() {
        // Scenario S29 item 1: destructive never; privileged per setting; plan never.
        let never = Err("a destructive call never comes from Telegram");
        assert_eq!(forward_rule(&card(ApprovalKind::Tool, false, false), false), Ok(()), "a plain tool card is forwarded");
        assert_eq!(forward_rule(&card(ApprovalKind::Tool, false, false), true), Ok(()));
        assert_eq!(forward_rule(&card(ApprovalKind::Tool, true, false), false), never);
        assert_eq!(forward_rule(&card(ApprovalKind::Tool, true, false), true), never, "not with allowPrivileged either");
        assert_eq!(forward_rule(&card(ApprovalKind::Tool, false, true), false), Err("a root action is not forwarded while allowPrivileged is off"));
        assert_eq!(forward_rule(&card(ApprovalKind::Tool, false, true), true), Ok(()), "a root action only with allowPrivileged");
        let plan = Err("a plan is approved on the desktop only");
        assert_eq!(forward_rule(&card(ApprovalKind::Plan, false, false), true), plan);
        assert_eq!(forward_rule(&card(ApprovalKind::Plan, false, true), true), plan);
    }

    #[test]
    fn a_card_shows_the_tool_the_summary_and_the_risk() {
        let mut plain = card(ApprovalKind::Tool, false, false);
        assert_eq!(approval_text(&plain), "KAgent needs your approval\n\nTool: shell\nrestart the kagent service\nRisk: medium\nWhy: changes system state");
        plain.preview = Some(format!("simulated\n{}", "x".repeat(1000)));
        assert!(approval_text(&plain).ends_with(&format!("\n\nsimulated\n{}", "x".repeat(790))), "a preview is cut to 800 characters");
        plain.privileged = true;
        assert!(approval_text(&plain).ends_with("\n\nThis is a root action."));
        plain.reason = "  ".into();
        plain.preview = Some("  ".into());
        assert!(!approval_text(&plain).contains("Why:"), "an empty reason says nothing");
    }

    #[test]
    fn alert_rules() {
        // Scenario S29 item 5.
        assert!(!alert_goes(Severity::High, false, false), "only while notifyAlerts is on");
        assert!(alert_goes(Severity::High, true, true), "high always goes, in the quiet hours too");
        assert!(alert_goes(Severity::Review, true, false));
        assert!(!alert_goes(Severity::Review, true, true), "a review alert in the quiet hours is not pushed");
        assert!(!alert_goes(Severity::Info, true, false), "info is for the UI only");
    }

    #[test]
    fn an_alert_message_names_it_and_its_severity() {
        let alert: Alert = serde_json::from_value(json!({
            "id": "alr_20261123_100000_ab12", "ts": "2026-11-23T10:00:00Z", "lastSeen": "2026-11-23T10:00:00Z", "count": 3,
            "rule": "ssh-bruteforce", "severity": "review", "status": "open", "title": "12 failed SSH logins", "cause": "repeated password failures",
            "evidence": [], "proposal": "block the address", "dedupeKey": "k"
        })).unwrap();
        assert_eq!(alert_text(&alert), "KAgent alert (review): 12 failed SSH logins\n\nrepeated password failures\nSuggested: block the address");
    }

    #[test]
    fn a_button_answers_only_what_the_bot_sent() {
        for allow in [true, false] {
            let data = approval_data(REQUEST, allow);
            assert!(data.len() <= 64, "Telegram takes 64 bytes: {}", data.len());
            assert_eq!(press(&data), Some(Press::Approval { request: REQUEST.into(), allow }));
        }
        assert!(ack_data("alr_20261123_100000_ab12").len() <= 64);
        assert_eq!(press("ack:alr_20261123_100000_ab12"), Some(Press::Ack { alert: "alr_20261123_100000_ab12".into() }));
        for other in ["", "allow", "a:1:always", "a:1", "ack", "a::allow", "allowOnce", "a:1:Allow"] {
            assert_eq!(press(other), None, "{other:?} is no button of this bot: Always is never offered");
        }
    }
}
