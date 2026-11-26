//! The runner tests of hooks.md section 8: the payload, each way a run can end, the
//! deadline, the environment and the output limit. The programs under test are `/bin/sh`
//! lines and `sleep`, in a temporary folder.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::*;
use crate::hooks::config::Event;

fn sh(event: Event, line: &str) -> Hook {
    Hook { id: "hook_00000001".into(), shell: "/bin/sh".into(), ..Hook::new(event, line) }
}

async fn run_line(line: &str) -> Ran {
    run(&sh(Event::PreToolUse, line), &payload(), "s1", None).await
}

fn payload() -> Value {
    json!({ "event": "PreToolUse", "ts": "2026-11-26T10:15:30.123Z", "sessionId": "sess_1a2b3c4d", "via": "text", "tool": "shell", "input": { "command": "ls" } })
}

/// Whether a process of group `pgid` still runs, read from `/proc` (as `background/shutdown.rs` does).
fn group_alive(pgid: u32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else { return false };
    entries.flatten().any(|entry| {
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { return false };
        let mut fields = stat.rsplit_once(')').map(|(_, rest)| rest).unwrap_or_default().split_whitespace();
        let state = fields.next().unwrap_or_default();
        fields.nth(1) == Some(pgid.to_string().as_str()) && state != "Z"
    })
}

/// The group the test hook wrote down (`$$`: it leads a process group of its own).
async fn group_of(dir: &tempfile::TempDir) -> u32 {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(dir.path().join("pid")) {
            if let Ok(pgid) = text.trim().parse() {
                return pgid;
            }
        }
        assert!(Instant::now() < end, "the hook never wrote its pid");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_payload_is_on_stdin_and_every_run_leaves_an_audit_record() {
    let dir = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::open(&dir.path().join("audit.jsonl")).unwrap());
    let hook = Hook { id: "hook_1a2b3c4d".into(), ..Hook::new(Event::PreToolUse, "cat") };
    let ran = run(&hook, &payload(), "sess_1a2b3c4d", Some(&audit)).await;
    assert_eq!((ran.exit_code, ran.timed_out, ran.stdout.as_str()), (Some(0), false, payload().to_string().as_str()), "`cat` hands the payload back");
    assert_eq!(ran.decision, Ok(Decision::default()), "the payload holds no decision");
    let records: Vec<Value> = std::fs::read_to_string(dir.path().join("audit.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(records.len(), 1, "every run writes one record");
    assert_eq!((records[0]["kind"].as_str(), records[0]["sessionId"].as_str()), (Some("hook"), Some("sess_1a2b3c4d")));
    let data = &records[0]["data"];
    assert_eq!(
        (data["event"].as_str(), data["hookId"].as_str(), data["command"].as_str(), data["exitCode"].as_u64(), data["decision"].as_str(), data["timedOut"].as_bool()),
        (Some("PreToolUse"), Some("hook_1a2b3c4d"), Some("cat"), Some(0), Some("none"), Some(false))
    );
    assert!(data["durationMs"].is_u64(), "{}", data);
}

#[tokio::test]
async fn exit_0_with_each_decision_and_whitespace_tolerated() {
    let ran = run_line(r#"echo '{"decision":"ask","reason":"why not","additionalContext":"mind the printer","updatedInput":{"command":"ls -la"}}'"#).await;
    assert_eq!(
        ran.decision,
        Ok(Decision { verdict: Some(Verdict::Ask), reason: "why not".into(), additional_context: Some("mind the printer".into()), updated_input: Some(json!({"command": "ls -la"})) })
    );
    for (name, verdict) in [("allow", Verdict::Allow), ("deny", Verdict::Deny)] {
        let ran = run_line(&format!(r#"echo '{{"decision":"{name}"}}'"#)).await;
        assert_eq!(ran.decision, Ok(Decision { verdict: Some(verdict), ..Default::default() }), "{name}");
        assert_eq!(ran.label(), name);
    }
    let ran = run_line(r#"printf ' \n { "decision" : "deny" , "reason" : "not here" } \n ' "#).await;
    assert_eq!(ran.decision, Ok(Decision { verdict: Some(Verdict::Deny), reason: "not here".into(), ..Default::default() }), "whitespace is tolerated");
    // What is not one JSON object, or decides nothing the agent knows, fails the hook.
    for line in [r#"echo ok"#, r#"echo [1,2]"#, r#"echo '{"decision":"maybe"}'"#, r#"echo '{"reason":5}'"#, r#"echo '{"updatedInput":"ls"}'"#] {
        let ran = run_line(line).await;
        assert!(ran.decision.is_err(), "{line}: {:?}", ran.decision);
    }
    // A field of its own is the hook's business.
    let ran = run_line(r#"echo '{"decision":"allow","hookSpecificOutput":{"x":1}}'"#).await;
    assert_eq!(ran.decision, Ok(Decision { verdict: Some(Verdict::Allow), ..Default::default() }));
}

#[tokio::test]
async fn exit_2_denies_and_stderr_is_the_reason() {
    let ran = run_line(r#"echo 'no curl on this machine' >&2; exit 2"#).await;
    assert_eq!(ran.decision, Ok(Decision { verdict: Some(Verdict::Deny), reason: "no curl on this machine".into(), ..Default::default() }));
    let ran = run_line("exit 2").await;
    assert_eq!(ran.decision, Ok(Decision { verdict: Some(Verdict::Deny), reason: "denied by the hook".into(), ..Default::default() }), "a reason when stderr is empty");
    let ran = run_line(r#"echo '{"decision":"allow"}'; exit 2"#).await;
    assert_eq!(ran.decision.unwrap().verdict, Some(Verdict::Deny), "the code wins over stdout");
}

#[tokio::test]
async fn another_exit_code_or_a_signal_is_a_failure() {
    let ran = run_line(r#"echo 'boom' >&2; exit 3"#).await;
    assert_eq!((ran.exit_code, ran.decision.clone()), (Some(3), Err("exit code 3: boom".into())));
    assert_eq!(ran.label(), "failed");
    assert_eq!(ran.or_allow(), Decision::default(), "what a failed hook counts as");
    let ran = run_line("kill -9 $$").await;
    assert_eq!(ran.exit_code, None);
    assert!(ran.decision.unwrap_err().starts_with("ended without an exit code (a signal)"), "killed by a signal");
}

#[tokio::test]
async fn a_hook_past_its_deadline_is_killed_with_its_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let mut hook = sh(Event::Stop, &format!("echo $$ > '{}'; sleep 30", dir.path().join("pid").display()));
    hook.timeout_ms = 200;
    let started = Instant::now();
    let ran = run(&hook, &json!({}), "s1", None).await;
    assert!(started.elapsed() < Duration::from_secs(5), "nothing waits for `sleep 30`: {:?}", started.elapsed());
    assert!(ran.timed_out && ran.exit_code.is_none(), "{ran:?}");
    assert_eq!(ran.decision, Err("timed out after 200 ms".into()));
    assert_eq!(ran.label(), "failed");
    let pgid = group_of(&dir).await;
    let end = Instant::now() + Duration::from_secs(5);
    while group_alive(pgid) && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!group_alive(pgid), "`sleep 30` went with the process group of the hook");
}

#[tokio::test]
async fn the_environment_holds_the_four_variables_only() {
    let hook = Hook { id: "hook_00000001".into(), ..Hook::new(Event::SessionStart, "env") };
    let ran = run(&hook, &json!({}), "s1", None).await;
    assert_eq!(ran.exit_code, Some(0), "{}", ran.stderr);
    let vars: Vec<(String, String)> = ran.stdout.lines().map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())).unwrap()).collect();
    let mut names: Vec<&str> = vars.iter().map(|(k, _)| k.as_str()).collect();
    names.sort();
    assert_eq!(names, ["HOME", "LANG", "PATH", "USER"], "nothing else of the daemon's environment reaches the hook");
    let value = |name: &str| vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    assert_eq!((value("HOME").as_deref(), value("PATH").as_deref()), (std::env::var("HOME").ok().as_deref(), std::env::var("PATH").ok().as_deref()));
}

#[tokio::test]
async fn output_over_64_kb_fails_the_hook_and_the_run_ends() {
    let ran = run_line(r#"head -c 70000 /dev/zero | tr '\0' x; echo; echo '{"decision":"deny"}'"#).await;
    assert_eq!(ran.exit_code, Some(0));
    assert_eq!(ran.decision, Err("stdout is longer than 64 KB".into()), "a decision after the limit is not read");
    assert!(ran.stdout.len() <= STDOUT_LIMIT, "kept: {}", ran.stdout.len());
    // The limit itself is fine.
    let hook = sh(Event::Stop, r#"printf '{"decision":"deny","reason":"%s"}' "$(head -c 65000 /dev/zero | tr '\0' x)""#);
    let ran = run(&hook, &json!({}), "s1", None).await;
    assert_eq!(ran.decision.as_ref().unwrap().verdict, Some(Verdict::Deny));
    assert!(ran.stdout.len() > 64_000);
}

#[tokio::test]
async fn a_hook_that_cannot_start_is_a_failure() {
    let hook = Hook { id: "hook_00000001".into(), ..Hook::new(Event::Stop, "/nonexistent/kagent-s32-hook") };
    let ran = run(&hook, &json!({}), "s1", None).await;
    assert_eq!(ran.label(), "failed");
    assert!(ran.decision.unwrap_err().starts_with("cannot start the hook: "), "a program that is not there");
}

#[tokio::test]
async fn a_hook_that_leaves_something_running_does_not_hold_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let hook = sh(Event::Stop, &format!(r#"echo $$ > '{}'; sleep 30 & echo '{{"decision":"allow"}}'"#, dir.path().join("pid").display()));
    let started = Instant::now();
    let ran = run(&hook, &json!({}), "s1", None).await;
    assert_eq!(ran.decision, Ok(Decision { verdict: Some(Verdict::Allow), ..Default::default() }));
    assert!(!ran.timed_out);
    assert!(PIPE_GRACE <= started.elapsed() && started.elapsed() < Duration::from_secs(3), "the pipes are let go after a moment, not at the deadline: {:?}", started.elapsed());
    // What the hook left behind is not waited for. This test stops what it started.
    let pgid = group_of(&dir).await;
    kill_group(pgid, "KILL");
    let end = Instant::now() + Duration::from_secs(5);
    while group_alive(pgid) && Instant::now() < end {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!group_alive(pgid));
}
