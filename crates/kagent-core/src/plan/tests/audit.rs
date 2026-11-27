//! v0.0.2 section 7: the audit record of an approval says how it was answered (`via`).

use super::*;

#[tokio::test]
async fn the_approval_record_names_how_the_card_was_answered() {
    let r = rig(PermissionMode::Ask, vec![]).await;
    r.agent.set_planning(&r.session, true).await.unwrap();
    answered(&r, json!({ "plan": "1. a" }), ApprovalDecision::AllowOnce).await;
    let path = r.dir.path().join("data/audit.jsonl");
    let raw = std::fs::read_to_string(&path).unwrap();
    let record: Value = raw.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).find(|v| v["kind"] == "approval").expect("an approval record");
    assert_eq!((record["data"]["decision"].as_str(), record["data"]["via"].as_str()), (Some("allowOnce"), Some("click")), "{record}");
    assert!(crate::audit::verify(&path).is_ok(), "the chain holds");
}
