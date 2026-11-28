//! The marker of a running watch, the stop phrases, the kill switch, and that no frame is ever stored
//! (screen-watch.md sections 1, 3 and 7).

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::*;

fn listing(dir: &std::path::Path) -> Vec<String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            out.push(p.display().to_string());
            if p.is_dir() {
                walk(&p, out);
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, &mut v);
    v.sort();
    v
}

fn rig_in(dir: &std::path::Path) -> Rig {
    let fake = Fake::new();
    let (tx, events) = broadcast::channel(1024);
    Rig { sw: ScreenWatch::new(fake.clone(), tx, Some(dir)), fake, events }
}

#[tokio::test(start_paused = true)]
async fn frames_never_reach_disk_the_home_is_the_same_before_and_after_a_run() {
    let home = tempfile::tempdir().unwrap();
    let shots = tempfile::tempdir().unwrap();
    // The frame the "screen" shows: a file outside the home, read through the daemon's own capture.
    let frame = shots.path().join("screen.png");
    std::fs::write(&frame, png(true)).unwrap();
    std::env::set_var("KAGENT_FAKE_SCREENSHOT", &frame);
    let before = listing(home.path());
    assert!(before.is_empty());
    let r = rig_in(home.path());
    r.fake.real_grab.store(true, Ordering::SeqCst);
    r.fake.say(&[&reply(false, "compiling", 0.9), &reply(true, "done", 0.9)]);
    r.sw.start(StartArgs { minutes: Some(1), ..args("g") }).await.unwrap();
    // While it runs the only file is the marker, and it holds the goal, not a picture.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let during = listing(home.path());
    assert_eq!(during.len(), 1, "{during:?}");
    let marker = std::fs::read_to_string(&during[0]).unwrap();
    assert!(during[0].ends_with("screen-watch.json") && marker.contains("\"goal\":\"g\"") && !marker.contains("base64") && !marker.contains("PNG"), "{marker}");
    settle(&r.sw).await;
    std::env::remove_var("KAGENT_FAKE_SCREENSHOT");
    assert_eq!(listing(home.path()), before, "after the run the home is as before: no frame, no marker");
    for told in r.fake.told() {
        assert!(told.is_ascii() && told.len() < 200 && !told.contains("data:image"), "a message holds words only: {told}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_marker_goes_with_every_way_a_watch_ends() {
    let home = tempfile::tempdir().unwrap();
    let r = rig_in(home.path());
    r.sw.start(args("a")).await.unwrap();
    assert_eq!(listing(home.path()).len(), 1);
    r.sw.stop();
    assert!(listing(home.path()).is_empty(), "stopped");
    r.fake.gone.store(true, Ordering::SeqCst);
    r.sw.start(args("b")).await.unwrap();
    settle(&r.sw).await;
    assert!(listing(home.path()).is_empty(), "failed");
}

#[tokio::test(start_paused = true)]
async fn a_restart_ends_the_watch_and_the_session_is_told_once_at_the_next_start() {
    let home = tempfile::tempdir().unwrap();
    {
        let r = rig_in(home.path());
        r.sw.start(StartArgs { session_id: "s7".into(), ..args("tell me when the build finishes") }).await.unwrap();
        // The daemon stops here: nothing ends the watch, the marker stays.
        assert_eq!(listing(home.path()).len(), 1);
    }
    let r = rig_in(home.path());
    assert!(r.sw.current().is_none(), "state is in memory only: nothing runs after a restart");
    r.sw.start_up().await;
    assert_eq!(r.fake.restarts.lock().unwrap().clone(), vec!["The screen watch ended with the restart. (goal: tell me when the build finishes)"]);
    let last = r.sw.last_ended().unwrap();
    assert_eq!((last.state, last.session_id.as_str(), last.reason.as_deref()), (State::Failed, "s7", Some("the daemon restarted")));
    assert!(listing(home.path()).is_empty());
    r.sw.start_up().await;
    assert_eq!(r.fake.restarts.lock().unwrap().len(), 1, "told once");
    assert!(r.fake.told().is_empty(), "no voice, no notification for it");
}

#[tokio::test(start_paused = true)]
async fn the_stop_phrase_ends_a_running_watch_even_when_it_comes_in_pieces() {
    let r = rig();
    assert!(!r.sw.hear("stop watching"), "nothing runs: nothing to stop");
    r.sw.start(args("g")).await.unwrap();
    assert!(!r.sw.hear("could you keep an eye on it"));
    assert!(!r.sw.hear("don't stop watching"));
    assert!(r.sw.current().is_some());
    assert!(!r.sw.hear("ok stop"));
    assert!(r.sw.hear("watching now"), "the two pieces make the phrase");
    assert_eq!(r.sw.last_ended().unwrap().state, State::Stopped);
    assert!(!r.sw.hear("stop watching"), "and once it is over there is nothing to stop");
}

#[tokio::test(start_paused = true)]
async fn the_kill_switch_ends_the_watch_and_a_quiet_hour_does_not() {
    let r = rig();
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(30)).await;
    // `POST /stop-all`, the tray and KRunner call `stop`.
    assert_eq!(r.sw.stop().unwrap().state, State::Stopped);
    assert!(r.sw.current().is_none());
    // Quiet hours (watchers.quietHours) are about alerts: nothing in the watch reads them.
    let src = include_str!("../run.rs").to_string() + include_str!("../mod.rs") + include_str!("../env.rs");
    assert!(!src.to_lowercase().contains("quiet"), "a watch ignores quiet hours");
}

#[test]
fn every_way_to_stop_everything_stops_the_watch() {
    for (file, src) in [("api.rs", include_str!("../../api.rs")), ("krunner.rs", include_str!("../../krunner.rs")), ("tray.rs", include_str!("../../tray.rs"))] {
        let calls = src.matches("agent.stop_all()").count();
        let stops = src.matches("screen_watch.stop()").count();
        assert!(calls > 0 && stops >= calls, "{file}: each stop_all is followed by the watch's stop ({calls} calls, {stops} stops)");
    }
}
