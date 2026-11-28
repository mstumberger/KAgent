//! The hub on real hook commands (small `/bin/sh` lines, files in temporary folders):
//! what reaches a hook, what its answer does to a call, the events of the loop, an
//! alert, and the work of the routes.

use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use kagent_core::audit::AuditLog;
use kagent_core::protocol::{AgentEvent, ToolCall, TurnStatus, Via};
use kagent_core::tool::{CallAssessment, Tool};
use kagent_core::tools::shell::Shell;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::*;
use crate::settings::Settings;
use crate::watch::AlertPush;

fn sh(id: &str, event: Event, line: impl Into<String>) -> Hook {
    Hook { id: id.into(), shell: "/bin/sh".into(), ..Hook::new(event, line.into()) }
}

struct Rig {
    hub: HookHub,
    events: broadcast::Receiver<AgentEvent>,
    dir: tempfile::TempDir,
}

/// A daemon's settings with `hooks`, an audit log and an event stream.
fn rig(hooks: Vec<Hook>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config").join("settings.json");
    let mut settings = Settings::defaults();
    let mut section = HooksSection::default();
    for hook in hooks {
        section.upsert(hook);
    }
    settings.extra.insert("hooks".into(), serde_json::to_value(&section).unwrap());
    settings.save(&path).unwrap();
    let (etx, erx) = broadcast::channel(64);
    let audit = Arc::new(AuditLog::open(&dir.path().join("audit.jsonl")).unwrap());
    Rig { hub: HookHub::new(Arc::new(RwLock::new(settings)), path, audit, etx), events: erx, dir }
}

impl Rig {
    /// The audit records of the hook runs.
    fn audit(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("audit.jsonl")).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()).filter(|v| v["kind"] == "hook").collect()
    }
}

fn call(name: &str, input: Value) -> ToolCall {
    ToolCall { id: "call_1".into(), name: name.into(), input }
}

fn assessment(command: &str) -> CallAssessment {
    Shell::new().assess(&json!({ "command": command }))
}

fn start(session: &str, via: Via) -> HookEvent<'_> {
    HookEvent::SessionStart { session_id: session, via }
}

fn prompt<'a>(session: &'a str, via: Via, text: &'a str) -> HookEvent<'a> {
    HookEvent::UserPromptSubmit { session_id: session, via, prompt: text }
}

fn stop<'a>(session: &'a str, via: Via, status: TurnStatus, error: Option<&'a str>) -> HookEvent<'a> {
    HookEvent::Stop { session_id: session, via, turn_id: "turn_1", status, error }
}

/// Wait for `cond`, a little while.
async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The next notice, a warn: its text.
async fn notice(r: &mut Rig, what: &str) -> String {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let left = end.saturating_duration_since(Instant::now());
        let ev = tokio::time::timeout(left, r.events.recv()).await.unwrap_or_else(|_| panic!("no notice for {what}")).expect("event channel open");
        if let AgentEvent::Notice { level, text, .. } = ev {
            assert_eq!(level, "warn", "{what}");
            return text;
        }
    }
}

fn read(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn where_no_hook_matches_nothing_is_spawned() {
    let mut r = rig(vec![sh("hook_stop0000", Event::Stop, "true")]);
    let look = call("shell", json!({ "command": "ls" }));
    assert_eq!(r.hub.pre("s1", &look, &assessment("ls")).await, HookInput::ALLOW);
    assert_eq!(r.hub.post("s1", &look, "ok", false, 3).await, None);
    assert_eq!(r.hub.event(start("s1", Via::Text)).await, None);
    assert_eq!(r.hub.event(prompt("s1", Via::Text, "hi")).await, None);
    assert!(r.audit().is_empty(), "no hook matched: no run, so no record");
    assert!(r.events.try_recv().is_err(), "and nothing is told");
    assert_eq!(r.hub.event(stop("s1", Via::Text, TurnStatus::Completed, None)).await, None);
    assert_eq!(r.audit().len(), 1, "the hook of another event runs for its own");
}

#[tokio::test]
async fn a_pretooluse_hook_sees_the_call_and_decides() {
    // A guard on shell: it asks for every shell call and denies those holding `curl`.
    let r = rig(vec![Hook {
        matcher: vec!["shell".into()],
        ..sh("hook_guard001", Event::PreToolUse, r#"grep -q curl && { echo 'curl is not allowed' >&2; exit 2; }; echo '{"decision":"ask","reason":"the owner wants a look at shell calls","additionalContext":"shell calls are watched"}'"#)
    }]);
    let look = call("shell", json!({ "command": "echo hi" }));
    let asked = r.hub.pre("s1", &look, &assessment("echo hi")).await;
    assert_eq!(
        asked,
        HookInput { verdict: HookVerdict::Ask, reason: "the owner wants a look at shell calls".into(), input: None, context: Some("shell calls are watched".into()) }
    );
    let curl = call("shell", json!({ "command": "curl https://example.com" }));
    let denied = r.hub.pre("s1", &curl, &assessment("curl https://example.com")).await;
    assert_eq!(denied, HookInput { verdict: HookVerdict::Deny, reason: "curl is not allowed".into(), ..HookInput::ALLOW });
    assert_eq!(r.hub.pre("s1", &call("write_file", json!({ "path": "~/x" })), &assessment("echo hi")).await, HookInput::ALLOW, "the matcher leaves other tools alone");
    let records = r.audit();
    assert_eq!(records.iter().map(|d| d["data"]["decision"].as_str()).collect::<Vec<_>>(), [Some("ask"), Some("deny")]);
    assert!(records.iter().all(|d| d["sessionId"] == "s1" && d["data"]["hookId"] == "hook_guard001" && d["data"]["event"] == "PreToolUse"));

    // What the hook is given: the event, the session, the tool, the input as the audit keeps it, the assessment.
    let out = tempfile::tempdir().unwrap();
    let r = rig(vec![sh("hook_see0002", Event::PreToolUse, format!("cat > '{}'", out.path().join("pre.json").display()))]);
    let secret = call("shell", json!({ "command": "ls", "password": "hunter2" }));
    r.hub.pre("s1", &secret, &assessment("ls")).await;
    let seen = read(&out.path().join("pre.json"));
    assert_eq!((seen["event"].as_str(), seen["sessionId"].as_str(), seen["via"].as_str(), seen["tool"].as_str()), (Some("PreToolUse"), Some("s1"), Some("text"), Some("shell")));
    assert!(seen["input"]["password"].as_str().unwrap().starts_with("<redacted:"), "redacted as in the audit: {}", seen["input"]["password"]);
    assert_eq!(seen["input"]["command"], "ls");
    assert_eq!((seen["assessment"]["readOnly"].as_bool(), seen["summary"].as_str()), (Some(true), Some("ls")), "the assessment the engine was shown, and its one line");
}

#[tokio::test]
async fn a_replaced_input_is_what_the_next_hook_sees() {
    let out = tempfile::tempdir().unwrap();
    let r = rig(vec![
        sh("hook_first01", Event::PreToolUse, r#"echo '{"decision":"allow","updatedInput":{"command":"echo one"}}'"#),
        sh("hook_see0002", Event::PreToolUse, format!("cat > '{}'", out.path().join("pre.json").display())),
    ]);
    let out_input = r.hub.pre("s1", &call("shell", json!({ "command": "echo zero" })), &assessment("echo zero")).await;
    assert_eq!(out_input, HookInput { input: Some(json!({ "command": "echo one" })), ..HookInput::ALLOW });
    assert_eq!(read(&out.path().join("pre.json"))["input"], json!({ "command": "echo one" }));
    // A denial ends the run of the hooks: the one after it is not started.
    let out = tempfile::tempdir().unwrap();
    let r = rig(vec![
        sh("hook_deny001", Event::PreToolUse, "echo 'no' >&2; exit 2"),
        sh("hook_after02", Event::PreToolUse, format!("cat > '{}'", out.path().join("never.json").display())),
    ]);
    assert_eq!(r.hub.pre("s1", &call("shell", json!({})), &assessment("ls")).await.verdict, HookVerdict::Deny);
    assert!(!out.path().join("never.json").exists());
    assert_eq!(r.audit().len(), 1);
}

#[tokio::test]
async fn a_failing_hook_is_non_blocking_and_is_told() {
    let mut r = rig(vec![sh("hook_fail001", Event::PreToolUse, "echo 'python is missing' >&2; exit 3")]);
    let look = call("shell", json!({ "command": "ls" }));
    assert_eq!(r.hub.pre("s1", &look, &assessment("ls")).await, HookInput::ALLOW, "allow, the input as it was");
    assert_eq!(
        notice(&mut r, "a failure").await,
        "Hook 'hook_fail001' (PreToolUse) failed: exit code 3: python is missing. The agent goes on without it."
    );
    assert_eq!(r.audit()[0]["data"]["decision"], "failed");

    // Past its deadline: killed, and told the same way.
    let mut slow = sh("hook_slow002", Event::UserPromptSubmit, "sleep 30");
    slow.timeout_ms = 200;
    let mut r = rig(vec![slow]);
    assert_eq!(r.hub.event(prompt("s1", Via::Text, "hi")).await, None);
    assert_eq!(notice(&mut r, "a timeout").await, "Hook 'hook_slow002' (UserPromptSubmit) failed: timed out after 200 ms. The agent goes on without it.");
    assert_eq!(json!([r.audit()[0]["data"]["timedOut"], r.audit()[0]["data"]["decision"]]), json!([true, "failed"]));
}

#[tokio::test]
async fn post_gives_the_output_and_takes_the_context() {
    let out = tempfile::tempdir().unwrap();
    let mut off = sh("hook_off0003", Event::PostToolUse, "exit 3");
    off.enabled = false;
    let r = rig(vec![
        sh("hook_log0002", Event::PostToolUse, format!("cat > '{}'; echo '{{\"additionalContext\":\"logged the call\"}}'", out.path().join("post.json").display())),
        off,
    ]);
    let mut r = r;
    let done = call("shell", json!({ "command": "echo hi" }));
    assert_eq!(r.hub.post("s1", &done, "hi\n", false, 12).await, Some("logged the call".into()));
    let seen = read(&out.path().join("post.json"));
    assert_eq!(json!([seen["event"], seen["tool"], seen["output"], seen["isError"], seen["durationMs"]]), json!(["PostToolUse", "shell", "hi\n", false, 12]));
    assert_eq!(seen["input"], json!({ "command": "echo hi" }), "what ran");
    assert_eq!(r.audit().len(), 1, "a hook that is off does not run");
    assert!(r.events.try_recv().is_err(), "so it is not told either");
}

#[tokio::test]
async fn an_async_hook_returns_at_once_and_its_context_is_for_the_next_turn() {
    let mut late = sh("hook_late004", Event::UserPromptSubmit, r#"sleep 1; echo '{"additionalContext":"a late note"}'"#);
    late.r#async = true;
    let r = rig(vec![late]);
    let started = Instant::now();
    assert_eq!(r.hub.event(prompt("s1", Via::Text, "hello")).await, None, "nothing waits for it");
    assert!(started.elapsed() < Duration::from_millis(600), "{:?}", started.elapsed());
    assert!(r.audit().is_empty(), "it is still running");
    until("the async hook to finish", || r.audit().len() == 1).await;
    assert_eq!(r.audit()[0]["data"]["decision"], "none");
    let next = r.hub.event(prompt("s1", Via::Text, "again")).await;
    assert_eq!(next.as_deref(), Some("(from the UserPromptSubmit hook 'hook_late004', run in the background) a late note"), "with the next turn");
    assert_eq!(r.hub.event(prompt("s1", Via::Text, "once more")).await, None, "said once");
}

#[tokio::test]
async fn the_events_carry_what_happened_and_the_tool_calls_know_the_via() {
    let out = tempfile::tempdir().unwrap();
    let file = |name: &str| format!("cat > '{}'", out.path().join(name).display());
    let r = rig(vec![
        sh("hook_start01", Event::SessionStart, file("start.json")),
        sh("hook_prompt2", Event::UserPromptSubmit, file("prompt.json")),
        sh("hook_pre0003", Event::PreToolUse, file("pre.json")),
        sh("hook_stop004", Event::Stop, file("stop.json")),
    ]);
    r.hub.event(start("s1", Via::Voice)).await;
    assert_eq!(read(&out.path().join("start.json"))["via"], "voice");
    r.hub.event(prompt("s1", Via::Voice, "is it raining?")).await;
    assert_eq!(read(&out.path().join("prompt.json"))["prompt"], "is it raining?");
    // A tool call of that turn is told how the turn was reached.
    r.hub.pre("s1", &call("shell", json!({ "command": "ls" })), &assessment("ls")).await;
    assert_eq!(read(&out.path().join("pre.json"))["via"], "voice");
    r.hub.event(stop("s1", Via::Voice, TurnStatus::Failed, Some("boom"))).await;
    let seen = read(&out.path().join("stop.json"));
    assert_eq!((seen["turnId"].as_str(), seen["status"].as_str(), seen["error"].as_str()), (Some("turn_1"), Some("failed"), Some("boom")));
    // The turn is over: the next call is not said to be voice.
    r.hub.pre("s1", &call("shell", json!({ "command": "ls" })), &assessment("ls")).await;
    assert_eq!(read(&out.path().join("pre.json"))["via"], "text");
}

#[tokio::test]
async fn an_alert_reaches_the_hooks_of_its_event() {
    use crate::watch::{Alert, AlertStatus, Severity};
    let now = chrono::Utc::now();
    let alert = Alert {
        id: "alr_20261126_101530_1a2b3c".into(),
        ts: now,
        last_seen: now,
        count: 1,
        rule: "disk-space".into(),
        severity: Severity::Review,
        status: AlertStatus::Open,
        title: "Disk nearly full".into(),
        cause: "/ is 92% full".into(),
        evidence: json!({ "mount": "/" }),
        proposal: "Free disk space".into(),
        dedupe_key: "disk:/".into(),
        note: None,
    };
    let out = tempfile::tempdir().unwrap();
    let r = rig(vec![sh("hook_alert05", Event::AlertRaised, format!("cat > '{}'", out.path().join("alert.json").display()))]);
    r.hub.raised(&alert);
    let path = out.path().join("alert.json");
    until("the hook of the alert to run", || path.exists()).await;
    let seen = read(&path);
    assert_eq!((seen["event"].as_str(), seen["sessionId"].as_str(), seen["alert"]["id"].as_str(), seen["alert"]["title"].as_str()), (Some("AlertRaised"), Some("watch"), Some("alr_20261126_101530_1a2b3c"), Some("Disk nearly full")));
    // With no hook for the event nothing is started.
    let r = rig(vec![sh("hook_stop006", Event::Stop, "true")]);
    r.hub.raised(&alert);
    assert!(r.audit().is_empty());
}

#[tokio::test]
async fn the_routes_add_change_toggle_delete_and_test() {
    let mut r = rig(vec![]);
    let created = r.hub.create(json!({ "event": "PreToolUse", "matcher": ["shell"], "command": "/bin/echo", "args": ["hi"], "timeoutMs": 1000 })).unwrap();
    assert!(created.id.starts_with("hook_") && created.id.len() == 13 && created.enabled, "the daemon assigns the id");
    // Saved in settings.json, and told to every UI.
    let saved = crate::settings::Settings::load(&r.dir.path().join("config").join("settings.json")).unwrap();
    assert_eq!(saved.hooks_config().find(&created.id), Some(&created));
    assert_eq!(r.hub.views(), vec![created.clone()]);
    assert!(matches!(r.events.try_recv(), Ok(AgentEvent::SettingsChanged { .. })));

    // What is refused, with 400.
    for (body, wants) in [
        (json!({ "id": "hook_mine001", "event": "Stop", "command": "/bin/true" }), "leave id out"),
        (json!({ "event": "Stop", "command": "/bin/true", "matcher": ["shell"] }), "matcher applies"),
        (json!({ "event": "Stop", "comand": "/bin/true" }), "unknown field `comand`"),
        (json!({ "event": "Stop", "command": "/bin/true", "timeoutMs": 99 }), "between 100 and 60000"),
        (json!({ "command": "/bin/true" }), "missing field `event`"),
        (json!({ "event": "PreToolUse" }), "missing field `command`"),
    ] {
        let err = r.hub.create(body.clone()).unwrap_err();
        assert_eq!((err.0, err.1), (StatusCode::BAD_REQUEST, "invalid_input"), "{body}");
        assert!(err.2.contains(wants), "{body}: {}", err.2);
    }
    assert_eq!(r.hub.views().len(), 1, "nothing of a refused request is kept");

    // A change replaces the record; its id is the one in the path.
    let changed = r.hub.update(&created.id, json!({ "id": created.id, "event": "PreToolUse", "command": "/bin/echo", "enabled": false })).unwrap();
    assert_eq!((changed.id.as_str(), changed.enabled, changed.matcher.len(), changed.args.len(), changed.timeout_ms), (created.id.as_str(), false, 0, 0, 5000));
    assert_eq!(r.hub.update("hook_nobody01", json!({ "event": "Stop", "command": "/bin/true" })).unwrap_err().0, StatusCode::NOT_FOUND);
    assert_eq!(r.hub.update(&created.id, json!({ "id": "hook_other02", "event": "PreToolUse", "command": "/bin/echo" })).unwrap_err().0, StatusCode::BAD_REQUEST);
    assert_eq!(r.hub.delete("hook_nobody01").unwrap_err().0, StatusCode::NOT_FOUND);
    let mut on = changed.clone();
    on.enabled = true;
    assert!(r.hub.update(&created.id, serde_json::to_value(&on).unwrap()).unwrap().enabled, "the toggle of the list");

    // Test: the hook as stored, with the sample of its event; the result goes to the caller only.
    let result = r.hub.test(&created.id, None, None).await.unwrap();
    assert_eq!((result.exit_code, result.decision, result.timed_out, result.stdout.as_str()), (Some(0), None, false, "\n"), "`/bin/echo` with the sample on stdin and no argument");
    let records = r.audit();
    assert_eq!((records.len(), records[0]["sessionId"].as_str(), records[0]["data"]["decision"].as_str()), (1, Some("hook-test"), Some("none")));
    assert!(r.hub.later.lock().unwrap().is_empty(), "nothing of a test waits for a turn");
    let result = r.hub.test(&created.id, Some(Event::Stop), Some(json!({ "event": "Stop", "my": "sample" }))).await.unwrap();
    assert_eq!((result.exit_code, result.stdout.as_str()), (Some(0), "\n"));
    assert_eq!(r.hub.test("hook_nobody01", None, None).await.unwrap_err().0, StatusCode::NOT_FOUND);

    r.hub.delete(&created.id).unwrap();
    assert!(r.hub.views().is_empty());
    assert_eq!(r.hub.delete(&created.id).unwrap_err().0, StatusCode::NOT_FOUND);
    assert!(crate::settings::Settings::load(&r.dir.path().join("config").join("settings.json")).unwrap().hooks_config().events.is_empty());
}
