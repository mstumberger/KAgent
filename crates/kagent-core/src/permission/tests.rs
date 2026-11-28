//! The unit tests of the permission engine and the broker.

use super::*;

fn call(read_only: bool, needs_approval: bool, privileged: bool) -> CallAssessment {
    CallAssessment {
        read_only,
        needs_approval,
        always_ask: privileged,
        privileged,
        concurrency_safe: read_only,
        risk: Risk::Medium,
        summary: String::new(),
        rule_key: "systemctl restart nginx".into(),
        destructive: false,
    }
}

#[test]
fn destructive_always_asks_even_with_full_control_and_rules() {
    let e = PermissionEngine::new(PermissionMode::Full, vec![AllowRule { tool: "system_power".into(), prefix: "".into() }]);
    let mut ovr = std::collections::HashMap::new();
    ovr.insert("system_power".to_string(), ToolOverride::Allow);
    e.set_overrides(ovr);
    let reboot = call(false, true, false).destructive();
    assert!(matches!(e.evaluate("s", "system_power", &reboot), Decision::Ask { .. }), "full control + allow rule + allow override still ask");
    e.set_mode(PermissionMode::ReadOnly);
    assert!(matches!(e.evaluate("s", "system_power", &reboot), Decision::Deny { .. }));
}

#[test]
fn modes_and_rules() {
    let e = PermissionEngine::new(PermissionMode::Ask, vec![]);
    assert!(matches!(e.evaluate("s", "systemd", &call(true, false, false)), Decision::Allow { .. }));
    assert!(matches!(e.evaluate("s", "systemd", &call(false, true, false)), Decision::Ask { .. }));
    e.add_rule(AllowRule { tool: "systemd".into(), prefix: "systemctl restart".into() });
    assert!(matches!(e.evaluate("s", "systemd", &call(false, true, false)), Decision::Allow { .. }));
    // Privileged calls ignore rules and trusted mode.
    e.set_mode(PermissionMode::Trusted);
    assert!(matches!(e.evaluate("s", "systemd", &call(false, true, true)), Decision::Ask { .. }));
    e.set_mode(PermissionMode::ReadOnly);
    assert!(matches!(e.evaluate("s", "systemd", &call(false, true, false)), Decision::Deny { .. }));
}

#[test]
fn session_rules_are_scoped_to_their_session() {
    // Scenario S17 item 3: "this session" allowed in A does not allow the call in B.
    let e = PermissionEngine::new(PermissionMode::Ask, vec![]);
    let mut shell_ls = call(false, true, false);
    shell_ls.rule_key = "shell ls".into();
    assert!(matches!(e.evaluate("A", "shell", &shell_ls), Decision::Ask { .. }));
    e.add_session_rule("A", AllowRule { tool: "shell".into(), prefix: "shell ls".into() });
    assert!(matches!(e.evaluate("A", "shell", &shell_ls), Decision::Allow { .. }), "still allowed in A");
    assert!(matches!(e.evaluate("B", "shell", &shell_ls), Decision::Ask { .. }), "session B still shows a card");
    assert!(matches!(e.evaluate("", "shell", &shell_ls), Decision::Ask { .. }), "no session id matches nothing");
    // The rule covers only its key prefix, as an "always allow" rule does.
    let mut shell_rm = call(false, true, false);
    shell_rm.rule_key = "shell rm -rf x".into();
    assert!(matches!(e.evaluate("A", "shell", &shell_rm), Decision::Ask { .. }));
}

#[test]
fn overrides() {
    let e = PermissionEngine::new(PermissionMode::Ask, vec![]);
    let mut o = std::collections::HashMap::new();
    o.insert("systemd".to_string(), ToolOverride::Deny);
    o.insert("journal".to_string(), ToolOverride::Ask);
    o.insert("shell".to_string(), ToolOverride::Allow);
    e.set_overrides(o);
    assert!(matches!(e.evaluate("s", "systemd", &call(true, false, false)), Decision::Deny { .. }));
    assert!(matches!(e.evaluate("s", "journal", &call(true, false, false)), Decision::Ask { .. }));
    assert!(matches!(e.evaluate("s", "shell", &call(false, true, false)), Decision::Allow { .. }));
    // allow never relaxes privileged calls, and read-only mode still wins.
    assert!(matches!(e.evaluate("s", "shell", &call(false, true, true)), Decision::Ask { .. }));
    e.set_mode(PermissionMode::ReadOnly);
    assert!(matches!(e.evaluate("s", "shell", &call(false, true, false)), Decision::Deny { .. }));
}

#[test]
fn a_call_of_an_outside_agent_asks_in_every_mode_and_no_rule_satisfies_it() {
    let ask = |e: &PermissionEngine, c: &CallAssessment| matches!(e.evaluate_from("mcp", "systemd", c, Origin::Agent), Decision::Ask { .. });
    let mutating = call(false, true, false);
    for mode in [PermissionMode::Ask, PermissionMode::Trusted, PermissionMode::Full] {
        let e = PermissionEngine::new(mode, vec![]);
        assert!(ask(&e, &mutating), "{mode:?} without a rule");
        e.add_rule(AllowRule { tool: "systemd".into(), prefix: "systemctl restart".into() });
        e.add_session_rule("mcp", AllowRule { tool: "systemd".into(), prefix: "systemctl restart".into() });
        assert!(ask(&e, &mutating), "{mode:?} with a matching rule and a session allowance");
        e.set_overrides([("systemd".to_string(), ToolOverride::Allow)].into());
        assert!(ask(&e, &mutating), "{mode:?} with an override allow");
        // The chat of the same engine is as before.
        let chat = e.evaluate("mcp", "systemd", &mutating);
        assert_eq!(matches!(chat, Decision::Allow { .. }), true, "{mode:?}: the chat is allowed by the rule");
        // A call that needs no approval is not read-only either: it asks too.
        assert!(ask(&e, &call(false, false, false)), "{mode:?}: not read-only, no approval needed");
        // Read-only calls run; a deny override and read-only mode still refuse.
        assert!(matches!(e.evaluate_from("mcp", "systemd", &call(true, false, false), Origin::Agent), Decision::Allow { .. }));
        e.set_overrides([("systemd".to_string(), ToolOverride::Deny)].into());
        assert!(matches!(e.evaluate_from("mcp", "systemd", &mutating, Origin::Agent), Decision::Deny { .. }));
    }
    let e = PermissionEngine::new(PermissionMode::ReadOnly, vec![]);
    assert!(matches!(e.evaluate_from("mcp", "systemd", &mutating, Origin::Agent), Decision::Deny { .. }));
}

#[tokio::test]
async fn broker_resolves_and_cancels() {
    let (tx, _rx) = broadcast::channel(16);
    let broker = PermissionBroker::new(tx, Duration::from_secs(5));
    let req = ApprovalRequest {
        request_id: "r1".into(),
        session_id: "s".into(),
        call_id: "c".into(),
        tool: "t".into(),
        kind: ApprovalKind::Tool,
        input: Value::Null,
        summary: String::new(),
        reason: String::new(),
        risk: Risk::Low,
        privileged: false,
        destructive: false,
        can_remember: true,
        rule_key: String::new(),
        preview: None,
    };
    let b2 = broker.clone();
    let waiter = tokio::spawn({
        let req = req.clone();
        async move { b2.request(req, &CancellationToken::new()).await }
    });
    while broker.pending().is_empty() {
        tokio::task::yield_now().await;
    }
    assert!(broker.resolve("r1", ApprovalDecision::AllowOnce, crate::permission::ApprovalVia::Click));
    assert_eq!(waiter.await.unwrap(), ApprovalDecision::AllowOnce);

    let cancel = CancellationToken::new();
    cancel.cancel();
    let d = broker.request(ApprovalRequest { request_id: "r2".into(), ..req.clone() }, &cancel).await;
    assert!(matches!(d, ApprovalDecision::Deny { .. }));

    // How each card was answered (config.md section 6, `via`).
    assert_eq!(broker.request_via(ApprovalRequest { request_id: "r3".into(), ..req.clone() }, &cancel).await.1, crate::permission::ApprovalVia::Cancel);
    let quick = PermissionBroker::new(broadcast::channel(8).0, Duration::from_millis(10));
    let (d, via) = quick.request_via(ApprovalRequest { request_id: "r4".into(), ..req.clone() }, &CancellationToken::new()).await;
    assert_eq!((d.label(), via.label()), ("deny", "timeout"));
    for via in [crate::permission::ApprovalVia::Voice, crate::permission::ApprovalVia::Telegram, crate::permission::ApprovalVia::Rule] {
        let b2 = broker.clone();
        let r = ApprovalRequest { request_id: format!("v-{}", via.label()), ..req.clone() };
        let waiter = tokio::spawn(async move { b2.request_via(r, &CancellationToken::new()).await });
        while broker.pending().is_empty() {
            tokio::task::yield_now().await;
        }
        assert!(broker.resolve(&format!("v-{}", via.label()), ApprovalDecision::AllowOnce, via));
        assert_eq!(waiter.await.unwrap(), (ApprovalDecision::AllowOnce, via));
    }
}

#[test]
fn full_control_allows_everything_but_root() {
    let e = PermissionEngine::new(PermissionMode::Full, vec![]);
    let mut always = call(false, true, false);
    always.always_ask = true;
    assert!(matches!(e.evaluate("s", "terminal", &always), Decision::Allow { .. }));
    assert!(matches!(e.evaluate("s", "package", &call(false, true, true)), Decision::Ask { .. }), "root still asks");
    let mut ovr = std::collections::HashMap::new();
    ovr.insert("shell".to_string(), ToolOverride::Deny);
    e.set_overrides(ovr);
    assert!(matches!(e.evaluate("s", "shell", &call(false, true, false)), Decision::Deny { .. }), "deny overrides win");
}
