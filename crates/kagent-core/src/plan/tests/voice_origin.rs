//! voice-control.md section 4: `always_ask` asks in `full` for a call of a voice turn, and
//! does not for a call of the chat.

use super::*;
use crate::permission::Origin;

/// `terminal send` (always_ask) run as a call of `origin` in `full` mode: whether a card
/// was raised. The card is denied, so nothing is typed.
async fn raises_a_card(origin: Origin) -> bool {
    let r = rig(PermissionMode::Full, vec![]).await;
    let calls = vec![ToolCall { id: "c1".into(), name: "terminal".into(), input: json!({"action": "send", "target": "nowhere", "text": "ls"}) }];
    let agent = r.agent.clone();
    let session = r.session.clone();
    let run = tokio::spawn(async move { agent.execute_calls_from(&session, "turn", &calls, &CancellationToken::new(), PrivilegeMethod::None, origin).await });
    let mut asked = false;
    for _ in 0..100 {
        if let Some(card) = r.agent.broker().pending().first() {
            assert!(card.reason.contains("always asks"), "{}", card.reason);
            r.agent.broker().resolve(&card.request_id, ApprovalDecision::Deny { reason: None }, crate::permission::ApprovalVia::Click);
            asked = true;
            break;
        }
        if run.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    run.await.unwrap();
    asked
}

#[tokio::test]
async fn a_voice_turn_asks_for_an_always_ask_call_in_full_and_chat_does_not() {
    assert!(raises_a_card(Origin::Voice).await, "from voice: asked");
    assert!(!raises_a_card(Origin::Chat).await, "from chat in full: unchanged");
}

#[test]
fn the_engine_asks_by_origin_and_a_via_voice_turn_is_voice() {
    let e = PermissionEngine::new(PermissionMode::Full, vec![]);
    let mut a = crate::tool::CallAssessment { read_only: false, needs_approval: true, always_ask: true, privileged: false, concurrency_safe: false, risk: crate::tool::Risk::Medium, summary: String::new(), rule_key: "x".into(), destructive: false };
    assert!(matches!(e.evaluate_from("s", "t", &a, Origin::Voice), Decision::Ask { .. }));
    assert!(matches!(e.evaluate_from("s", "t", &a, Origin::Chat), Decision::Allow { .. }));
    a.always_ask = false;
    assert!(matches!(e.evaluate_from("s", "t", &a, Origin::Voice), Decision::Allow { .. }), "an ordinary call is allowed in full");
    for (via, want) in [(crate::protocol::Via::Voice, Origin::Voice), (crate::protocol::Via::Text, Origin::Chat), (crate::protocol::Via::Task, Origin::Chat)] {
        assert_eq!(Origin::of_via(via), want);
    }
}
