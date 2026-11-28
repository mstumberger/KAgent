//! The one watch, the limits, the pauses and the load (screen-watch.md section 7).

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::*;

#[tokio::test(start_paused = true)]
async fn a_second_start_is_refused_with_the_running_goal() {
    let r = rig();
    r.sw.start(args("tell me when the build finishes")).await.unwrap();
    let e = r.sw.start(args("something else")).await.unwrap_err();
    assert!(e.contains("tell me when the build finishes") && e.contains("screen_watch stop"), "{e}");
    r.sw.stop();
    assert!(r.sw.start(args("something else")).await.is_ok(), "after the stop a new one starts");
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn two_starts_at_once_make_one_watch() {
    let r = rig();
    let (a, b) = tokio::join!(r.sw.start(args("a")), r.sw.start(args("b")));
    assert!(a.is_ok() != b.is_ok(), "exactly one: {a:?} {b:?}");
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn a_closed_window_fails_the_watch() {
    let r = rig();
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(15)).await;
    r.fake.gone.store(true, Ordering::SeqCst);
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Failed);
    assert_eq!(done.reason.as_deref(), Some("the window was closed"));
    assert_eq!(r.fake.told(), vec!["Screen watch ended: the window was closed. (goal: g)"]);
}

#[tokio::test(start_paused = true)]
async fn five_unreadable_answers_in_a_row_fail_it_and_one_good_answer_resets_the_count() {
    let r = rig();
    r.fake.say(&["no json", "{}", "yes", "[1]", &reply(false, "ok", 0.9), "no", "no", "no", "no", "no"]);
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert_eq!(r.sw.current().unwrap().last_note, "unreadable answer");
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Failed);
    assert_eq!(done.looks, 10, "four unreadable, a good one, then five");
    assert!(done.reason.unwrap().contains("5 unreadable answers in a row"));
}

#[tokio::test(start_paused = true)]
async fn a_model_that_does_not_answer_counts_like_an_unreadable_answer() {
    let r = rig();
    r.fake.replies.lock().unwrap().extend((0..5).map(|_| Err("the vision model did not answer: connection refused".to_string())));
    r.sw.start(args("g")).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Failed);
}

#[tokio::test(start_paused = true)]
async fn an_idle_screen_costs_a_model_look_in_seven() {
    let r = rig();
    r.fake.same_frame.store(true, Ordering::SeqCst);
    r.sw.start(StartArgs { minutes: Some(5), ..args("g") }).await.unwrap();
    let done = settle(&r.sw).await;
    // 30 looks at 10 s: the first and then one in seven.
    assert_eq!(done.looks, 30);
    assert_eq!(r.fake.local_calls.load(Ordering::SeqCst), 1 + (30 - 1) / 7);
}

#[tokio::test(start_paused = true)]
async fn a_locked_screen_pauses_the_watch_and_counts_no_looks() {
    let r = rig();
    r.fake.locked.store(true, Ordering::SeqCst);
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(95)).await;
    let w = r.sw.current().unwrap();
    assert!(w.paused, "the chip can say it is paused");
    assert_eq!((w.looks, r.fake.captures.load(Ordering::SeqCst)), (0, 0), "no frame was taken while it was locked");
    r.fake.locked.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(25)).await;
    let w = r.sw.current().unwrap();
    assert!(!w.paused && w.looks >= 2, "{w:?}");
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn the_clock_runs_while_it_is_locked() {
    let r = rig();
    r.fake.locked.store(true, Ordering::SeqCst);
    r.sw.start(StartArgs { minutes: Some(1), ..args("g") }).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!((done.state, done.looks), (State::Expired, 0));
}

#[tokio::test(start_paused = true)]
async fn an_approval_card_on_the_screen_stops_a_monitor_watch_from_looking() {
    let r = rig();
    r.fake.cards.store(1, Ordering::SeqCst);
    r.sw.start(StartArgs { target: Some(Target::Monitor), ..args("g") }).await.unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(r.fake.captures.load(Ordering::SeqCst), 0, "KAgent never captures its own cards");
    r.fake.cards.store(0, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(r.fake.captures.load(Ordering::SeqCst) > 0);
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn slow_looks_double_the_interval_and_say_so() {
    let r = rig();
    // Half of 10 s is 5 s: looks of 6 s are slow.
    *r.fake.look_time.lock().unwrap() = Duration::from_secs(6);
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(5 * 16 + 5)).await;
    let w = r.sw.current().unwrap();
    assert_eq!(w.every_sec, 20, "{w:?}");
    assert_eq!(w.last_note, "the looks are slow: now every 20 s");
    tokio::time::sleep(Duration::from_secs(5 * 26 + 60)).await;
    let w = r.sw.current().unwrap();
    assert!(w.every_sec <= 40 && w.every_sec >= 20, "it keeps doubling only while slow: {}", w.every_sec);
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn fast_looks_leave_the_interval_alone() {
    let r = rig();
    *r.fake.look_time.lock().unwrap() = Duration::from_secs(1);
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(r.sw.current().unwrap().every_sec, 10);
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn a_watch_waits_while_a_turn_thinks_on_its_model() {
    let r = rig();
    r.fake.busy.store(true, Ordering::SeqCst);
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(r.fake.captures.load(Ordering::SeqCst), 0, "no look while a turn thinks");
    r.fake.busy.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(15)).await;
    assert!(r.fake.captures.load(Ordering::SeqCst) >= 1);
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn the_limits_of_a_start_are_enforced_and_refused_with_what_to_do() {
    let r = rig();
    for (a, want) in [
        (StartArgs { minutes: Some(121), ..args("g") }, "between 1 and 120"),
        (StartArgs { minutes: Some(0), ..args("g") }, "between 1 and 120"),
        (StartArgs { every_sec: Some(4), ..args("g") }, "between 5 and 60"),
        (StartArgs { every_sec: Some(61), ..args("g") }, "between 5 and 60"),
        (args("   "), "needs a goal"),
        (args(&"x".repeat(201)), "over 200"),
    ] {
        let e = r.sw.start(a).await.unwrap_err();
        assert!(e.contains(want), "{e}");
    }
    assert!(r.sw.current().is_none(), "a refused start leaves nothing running");
    let w = r.sw.start(StartArgs { minutes: Some(120), every_sec: Some(5), ..args("g") }).await.unwrap();
    assert_eq!((w.minutes, w.every_sec), (120, 5));
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn the_two_hour_cap_ends_the_watch_at_two_hours() {
    let r = rig();
    r.sw.start(StartArgs { minutes: Some(120), every_sec: Some(60), ..args("g") }).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Expired);
    assert_eq!(done.looks, 120, "a look a minute for two hours, and then it is over");
    assert!(r.fake.told()[0].starts_with("Screen watch ended after 120 minutes"));
}

#[tokio::test(start_paused = true)]
async fn the_defaults_come_from_the_settings() {
    let r = rig();
    {
        let mut c = r.fake.cfg.lock().unwrap();
        c.default_minutes = 7;
        c.every_sec = 15;
    }
    let w = r.sw.start(args("g")).await.unwrap();
    assert_eq!((w.minutes, w.every_sec), (7, 15));
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn it_is_refused_when_switched_off_without_a_model_or_on_kagent_itself() {
    let r = rig();
    r.fake.cfg.lock().unwrap().enabled = false;
    assert!(r.sw.start(args("g")).await.unwrap_err().contains("Settings -> Desktop and apps"));
    r.fake.cfg.lock().unwrap().enabled = true;
    r.fake.no_model.store(true, Ordering::SeqCst);
    assert_eq!(r.sw.start(args("g")).await.unwrap_err(), "Screen watch needs a local vision model: Settings -> Local models recommends gemma3:4b for this machine.");
    r.fake.no_model.store(false, Ordering::SeqCst);
    r.fake.own_window.store(true, Ordering::SeqCst);
    assert!(r.sw.start(args("g")).await.unwrap_err().contains("KAgent itself"));
    assert!(r.sw.start(StartArgs { target: Some(Target::Monitor), ..args("g") }).await.is_ok(), "the monitor is fine");
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn the_watch_json_has_the_shape_of_the_spec() {
    let r = rig();
    let w = r.sw.start(args("g")).await.unwrap();
    let v = serde_json::to_value(&w).unwrap();
    for k in ["id", "sessionId", "goal", "target", "windowId", "windowTitle", "until", "everySec", "mode", "state", "looks", "lastNote"] {
        assert!(v.get(k).is_some(), "{k}: {v}");
    }
    assert!(v["id"].as_str().unwrap().starts_with("w_") && v["until"].as_str().unwrap().contains('T'));
    assert_eq!((v["target"].as_str(), v["state"].as_str(), v["mode"].as_str()), (Some("window"), Some("watching"), Some("once")));
    r.sw.stop();
}

#[tokio::test(start_paused = true)]
async fn the_events_between_states_are_at_most_every_five_seconds() {
    let mut r = rig();
    r.sw.start(StartArgs { every_sec: Some(5), minutes: Some(1), ..args("g") }).await.unwrap();
    settle(&r.sw).await;
    let n = states(&mut r.events).len();
    // The start, one every 5 s of 60 s at most, and the end.
    assert!((3..=15).contains(&n), "{n} events");
}
