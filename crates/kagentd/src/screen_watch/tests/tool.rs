//! The tool `screen_watch` and the permission it asks (screen-watch.md section 4).

use std::sync::Arc;

use kagent_core::permission::{Decision, Origin, PermissionEngine};
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};

use super::super::tool::{ScreenWatchTool, WatchCell};
use super::*;

fn tool(r: &Rig) -> ScreenWatchTool {
    let cell: WatchCell = Default::default();
    let _ = cell.set(r.sw.clone());
    ScreenWatchTool::new(cell)
}

async fn run(t: &ScreenWatchTool, input: Value) -> (bool, String) {
    let mut ctx = ToolContext::standalone(Duration::from_secs(5), 10_000);
    ctx.session_id = "s1".into();
    let out = t.call(input, ctx).await.unwrap();
    (out.is_error, out.content)
}

#[test]
fn the_summary_names_the_window_or_the_monitor_the_minutes_and_the_goal() {
    let r = rig();
    let t = tool(&r);
    std::env::set_var("KAGENT_FAKE_WINDOW", "0x0460000a konsole: cargo build");
    let a = t.assess(&json!({"action": "start", "goal": "tell me when the build finishes", "minutes": 20}));
    assert_eq!(a.summary, "watch \"konsole: cargo build\" for 20 minutes: tell me when the build finishes");
    let m = t.assess(&json!({"action": "start", "goal": "errors", "minutes": 5, "target": "monitor"}));
    assert_eq!(m.summary, "watch the monitor for 5 minutes: errors");
    std::env::remove_var("KAGENT_FAKE_WINDOW");
    assert!(a.always_ask && a.needs_approval && !a.read_only && !a.destructive, "a start asks every time");
    assert_eq!(a.rule_key, "screen_watch start");
}

#[test]
fn status_and_stop_ask_nothing() {
    let r = rig();
    let t = tool(&r);
    for action in ["status", "stop"] {
        let a = t.assess(&json!({"action": action}));
        assert!(a.read_only && !a.needs_approval && !a.always_ask, "{action}");
    }
    assert_eq!(kagent_core::tools::group_of("screen_watch"), "desktop");
    assert!(!t.contract().read_only && t.contract().always_ask);
}

#[test]
fn a_start_is_asked_from_voice_in_every_mode_and_a_stop_never() {
    let r = rig();
    let t = tool(&r);
    let start = t.assess(&json!({"action": "start", "goal": "g", "target": "monitor"}));
    let stop = t.assess(&json!({"action": "stop"}));
    for mode in [kagent_core::permission::PermissionMode::Ask, kagent_core::permission::PermissionMode::Full] {
        let engine = PermissionEngine::new(mode, Vec::new());
        assert!(matches!(engine.evaluate_from("s1", "screen_watch", &start, Origin::Voice), Decision::Ask { .. }), "{mode:?} from voice");
        assert!(matches!(engine.evaluate_from("s1", "screen_watch", &stop, Origin::Voice), Decision::Allow { .. }), "{mode:?} stop from voice");
    }
    let ask = PermissionEngine::new(kagent_core::permission::PermissionMode::Ask, Vec::new());
    assert!(matches!(ask.evaluate("s1", "screen_watch", &start), Decision::Ask { .. }), "from chat in the default mode");
}

#[tokio::test(start_paused = true)]
async fn start_status_and_stop_through_the_tool() {
    let r = rig();
    let t = tool(&r);
    assert_eq!(run(&t, json!({"action": "status"})).await, (false, "no screen watch".into()));
    let (err, text) = run(&t, json!({"action": "start", "goal": "tell me when the build finishes", "minutes": 10, "mode": "repeat"})).await;
    assert!(!err && text.contains("konsole: cargo build") && text.contains("10 minutes") && text.contains("tell me when the build finishes"), "{text}");
    let w = r.sw.current().unwrap();
    assert_eq!((w.session_id.as_str(), w.mode, w.minutes), ("s1", Mode::Repeat, 10), "the session of the call");
    let (_, status) = run(&t, json!({"action": "status"})).await;
    assert!(status.contains("tell me when the build finishes") && status.contains("repeat mode") && status.contains("0 looks"), "{status}");
    let (err, second) = run(&t, json!({"action": "start", "goal": "other"})).await;
    assert!(err && second.contains("tell me when the build finishes"), "{second}");
    let (err, stopped) = run(&t, json!({"action": "stop"})).await;
    assert!(!err && stopped.starts_with("Stopped the screen watch"), "{stopped}");
    let (_, status) = run(&t, json!({"action": "status"})).await;
    assert!(status.contains("No screen watch is running") && status.contains("ended as stopped"), "{status}");
    assert_eq!(run(&t, json!({"action": "stop"})).await, (false, "no screen watch is running".into()));
}

#[tokio::test]
async fn a_start_without_a_local_model_is_refused_and_says_what_to_do() {
    let r = rig();
    r.fake.no_model.store(true, std::sync::atomic::Ordering::SeqCst);
    let (err, text) = run(&tool(&r), json!({"action": "start", "goal": "g"})).await;
    assert!(err && text.contains("Settings -> Local models recommends gemma3:4b"), "{text}");
    assert!(r.sw.current().is_none());
}

#[tokio::test]
async fn bad_input_is_an_error_that_says_how_to_call() {
    let r = rig();
    let t = tool(&r);
    for bad in [json!({"action": "start", "goal": "g", "colour": "red"}), json!({"goal": "g"}), json!({"action": "start", "goal": "g", "minutes": "soon"})] {
        let (err, text) = run(&t, bad).await;
        assert!(err && text.contains("screen_watch"), "{text}");
    }
    let (err, text) = run(&t, json!({"action": "pause"})).await;
    assert!(err && text.contains("use start, status or stop"), "{text}");
    let _ = Arc::strong_count(&r.sw);
}
