//! The contracts and the arguments of the subagent tools (tools.md section 21): what a call
//! may do, how long it may wait, and how the job and the profile are read.

use super::*;

#[test]
fn the_job_takes_one_text_or_its_lines() {
    let run = |v: Value| serde_json::from_value::<RunArgs>(v).map(|a| (a.description, a.prompt, a.profile, a.model, a.background));
    assert_eq!(run(json!({"description": "d", "prompt": "one text"})).unwrap(), ("d".into(), "one text".into(), None, None, None));
    let lines = run(json!({"description": "d", "prompt": ["line 1", "line 2"], "profile": "explore", "background": false})).unwrap();
    assert_eq!((lines.1.as_str(), lines.2.as_deref(), lines.4), ("line 1\nline 2", Some("explore"), Some(false)), "lines are joined with line breaks");
    assert!(run(json!({"description": "d", "prompt": 7})).is_err(), "the job must be text");
    assert!(run(json!({"description": "d", "prompt": ["ok", 7]})).is_err(), "and every line of it");
    assert!(run(json!({"prompt": "p"})).is_err(), "the description is required");
    assert!(run(json!({"description": "d", "prompt": "p", "background": true})).unwrap().4 == Some(true), "the keys are camelCase");
    assert!(serde_json::from_value::<WaitArgs>(json!({"ids": ["a"], "timeout_sec": 5})).is_ok() && serde_json::from_value::<WaitArgs>(json!({"timeoutSec": 5})).is_err());
    assert!(serde_json::from_value::<WaitArgs>(json!({"id": "a"})).is_err(), "a key that is not a field is refused");
}

#[test]
fn the_tools_are_what_the_section_says() {
    let access = AgentAccess::default();
    let run = AgentRun::new(access.clone());
    let stop = AgentStop::new(access.clone());
    let wait = AgentWait::new(access.clone());
    let list = AgentList::new(access);
    // `agent_run` and `agent_stop`: MUT, risk low, no approval.
    for contract in [run.contract().clone(), stop.contract().clone()] {
        assert!(!contract.read_only && !contract.needs_approval && !contract.always_ask && contract.concurrency_safe, "{}", contract.name);
        assert_eq!(contract.risk, Risk::Low, "{}", contract.name);
    }
    // `agent_wait` and `agent_list`: RO.
    for contract in [wait.contract().clone(), list.contract().clone()] {
        assert!(contract.read_only && !contract.needs_approval, "{}", contract.name);
    }
    // Longer than a read-only call's 15 s: `agent_run` waits up to 120 s, `agent_wait` as long.
    for contract in [run.contract().clone(), wait.contract().clone()] {
        assert!(contract.default_timeout_ms >= children::RUN_WAIT.as_millis() as u64, "{} waits up to 120 s: the call must outlive it", contract.name);
    }
    let mut long = run.contract().clone();
    long.name = "x".into();
    assert_eq!(run.assess(&json!({"description": "Check the disk"})).summary, "subagent: Check the disk");
    assert_eq!(run.assess(&json!({})).rule_key, "agent_run", "not rememberable: it never asks");
    let cut = run.assess(&json!({"description": "x".repeat(100)})).summary;
    assert_eq!(cut.chars().count(), "subagent: ".len() + MAX_DESCRIPTION_CHARS, "{cut}");
    assert_eq!(stop.assess(&json!({"id": "c1"})).summary, "stop subagent c1");
    // The schemas name their keys as the tools read them.
    for (contract, keys) in [
        (run.contract(), vec!["description", "prompt", "profile", "model", "background"]),
        (wait.contract(), vec!["ids", "timeout_sec"]),
        (stop.contract(), vec!["id"]),
    ] {
        let mut named: Vec<&str> = contract.input_schema["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        named.sort_unstable();
        let mut keys = keys;
        keys.sort_unstable();
        assert_eq!(named, keys, "{}", contract.name);
    }
    assert_eq!(run.contract().input_schema["properties"]["prompt"]["anyOf"][0]["type"], "string");
}

#[test]
fn the_profile_named_is_found_or_the_others_are_listed() {
    let access = AgentAccess::default();
    assert_eq!(access.profile("general").unwrap(), Profile::general(), "the built-ins work without a daemon");
    assert_eq!(access.profile("explore").unwrap(), Profile::explore());
    assert_eq!(access.profile("disk-audit").unwrap_err(), "unknown profile 'disk-audit': general, explore");
    access.set_profiles(vec![Profile { name: "disk-audit".into(), ..Profile::general() }, Profile::general(), Profile { name: "zeta".into(), ..Profile::explore() }]);
    assert_eq!(access.profile("disk-audit").unwrap().name, "disk-audit");
    assert_eq!(access.profile("nope").unwrap_err(), "unknown profile 'nope': general, explore, disk-audit, zeta", "the profiles of the owner, in order, and no built-in twice");
    assert!(access.agent().is_err(), "nothing is attached: the tool says so");
    // The wait of `agent_run` is 120 s unless a test shortens it.
    assert_eq!((access.run_wait(), access.set_run_wait(Duration::from_secs(2)), access.run_wait()), (children::RUN_WAIT, (), Duration::from_secs(2)));
}
