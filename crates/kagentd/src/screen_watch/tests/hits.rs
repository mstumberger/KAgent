//! Hits, the end of a watch and the cloud check (screen-watch.md section 7).

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::*;

#[tokio::test(start_paused = true)]
async fn a_hit_on_the_third_look_ends_a_once_watch_with_one_message() {
    let mut r = rig();
    r.fake.say(&[&reply(false, "compiling 12/480", 0.9), &reply(false, "compiling 300/480", 0.9), &reply(true, "build finished, no errors", 0.95)]);
    let w = r.sw.start(args("tell me when the build in the terminal finishes")).await.unwrap();
    assert_eq!((w.state, w.mode, w.target, w.every_sec, w.minutes), (State::Watching, Mode::Once, Target::Window, 10, 20));
    assert_eq!(w.window_id.as_deref(), Some("0x0460000a"));
    let done = settle(&r.sw).await;
    assert_eq!((done.state, done.looks), (State::Met, 3));
    assert_eq!(done.last_note, "build finished, no errors");
    assert_eq!(r.fake.told(), vec!["Screen watch: build finished, no errors. (goal: tell me when the build in the terminal finishes)"]);
    assert_eq!(r.fake.local_calls.load(Ordering::SeqCst), 3);
    let seen = states(&mut r.events);
    assert_eq!(seen.first().unwrap().0, "watching");
    assert_eq!(seen.last().unwrap(), &("met".to_string(), 3), "the last event is the end, with its looks");
}

#[tokio::test(start_paused = true)]
async fn a_repeat_watch_goes_on_and_reports_at_most_every_30_seconds() {
    let r = rig();
    // Looks at 0, 10, 20, 30, 40 s: hits on all; reports at 0 and 30.
    r.fake.say(&[&reply(true, "error one", 0.9), &reply(true, "error one again", 0.9), &reply(false, "fine", 0.9), &reply(true, "error two", 0.9), &reply(false, "fine", 0.9)]);
    let w = r.sw.start(StartArgs { mode: Some(Mode::Repeat), minutes: Some(1), ..args("watch for errors") }).await.unwrap();
    assert_eq!(w.mode, Mode::Repeat);
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Expired, "a repeat watch ends with the time");
    let told = r.fake.told();
    assert_eq!(told.len(), 3, "two hits and the end: {told:?}");
    assert_eq!(told[0], "Screen watch: error one. (goal: watch for errors)");
    assert_eq!(told[1], "Screen watch: error two. (goal: watch for errors)");
    assert!(told[2].starts_with("Screen watch ended after 1 minute: "), "{told:?}");
}

#[tokio::test(start_paused = true)]
async fn two_hits_closer_than_30_seconds_are_one_report() {
    let r = rig();
    r.fake.say(&[&reply(true, "first", 0.9), &reply(true, "second", 0.9), &reply(true, "third", 0.9)]);
    r.sw.start(StartArgs { mode: Some(Mode::Repeat), minutes: Some(1), ..args("g") }).await.unwrap();
    settle(&r.sw).await;
    let hits: Vec<_> = r.fake.told().into_iter().filter(|t| !t.contains("ended after")).collect();
    assert_eq!(hits, vec!["Screen watch: first. (goal: g)"], "looks 2 and 3 are inside the 30 s");
}

#[tokio::test(start_paused = true)]
async fn the_time_up_says_the_last_note() {
    let r = rig();
    r.fake.say(&[&reply(false, "still compiling (312/480)", 0.9)]);
    r.sw.start(StartArgs { minutes: Some(1), ..args("g") }).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Expired);
    assert_eq!(r.fake.told().last().unwrap(), "Screen watch ended after 1 minute: still compiling");
}

#[tokio::test(start_paused = true)]
async fn the_time_up_after_20_minutes_names_the_minutes() {
    let r = rig();
    r.sw.start(args("g")).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Expired);
    assert_eq!(done.looks, 120, "a look every 10 s for 20 minutes");
    assert_eq!(r.fake.told(), vec!["Screen watch ended after 20 minutes: still compiling"]);
}

#[tokio::test(start_paused = true)]
async fn stop_ends_it_and_says_nothing() {
    let mut r = rig();
    r.sw.start(args("g")).await.unwrap();
    tokio::time::sleep(Duration::from_secs(25)).await;
    let w = r.sw.stop().unwrap();
    assert_eq!(w.state, State::Stopped);
    assert!(r.sw.current().is_none() && r.sw.stop().is_none(), "a second stop finds nothing");
    let looks = w.looks;
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(r.sw.last_ended().unwrap().looks, looks, "no look after the stop");
    assert!(r.fake.told().is_empty(), "the owner stopped it: nothing to tell");
    assert_eq!(states(&mut r.events).last().unwrap().0, "stopped");
}

#[tokio::test(start_paused = true)]
async fn a_hit_that_is_not_sure_needs_a_second_look_two_seconds_later() {
    let r = rig();
    r.fake.say(&[&reply(true, "maybe done", 0.4), &reply(false, "no, still running", 0.8), &reply(true, "maybe done", 0.5), &reply(true, "done for sure", 0.5)]);
    r.sw.start(args("g")).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Met);
    assert_eq!(r.fake.local_calls.load(Ordering::SeqCst), 4, "two hits that are not sure, two looks each");
    assert_eq!(r.fake.told(), vec!["Screen watch: done for sure. (goal: g)"], "one report, with the note of the second look");
}

#[tokio::test(start_paused = true)]
async fn the_second_look_is_never_skipped_even_when_the_screen_did_not_change() {
    let r = rig();
    r.fake.same_frame.store(true, Ordering::SeqCst);
    r.fake.say(&[&reply(true, "done", 0.3), &reply(true, "done", 0.9)]);
    r.sw.start(args("g")).await.unwrap();
    assert_eq!(settle(&r.sw).await.state, State::Met);
    assert_eq!(r.fake.local_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn without_cloud_confirm_no_frame_goes_to_the_cloud() {
    let r = rig();
    *r.fake.cloud.lock().unwrap() = Some(VecDeque::new());
    r.fake.say(&[&reply(true, "done", 0.9)]);
    r.sw.start(args("g")).await.unwrap();
    assert_eq!(settle(&r.sw).await.state, State::Met);
    assert_eq!(r.fake.cloud_frames.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn cloud_confirm_sends_exactly_one_frame_per_hit_and_a_no_is_not_a_hit() {
    let r = rig();
    r.fake.cfg.lock().unwrap().cloud_confirm = true;
    *r.fake.cloud.lock().unwrap() = Some(VecDeque::from([Ok(reply(false, "it is a test log", 0.9)), Ok(reply(true, "build finished", 0.9))]));
    // Looks that are not hits send nothing; the first hit is refused by the cloud model, the second is confirmed.
    r.fake.say(&[&reply(false, "compiling", 0.9), &reply(true, "done?", 0.9), &reply(true, "done", 0.9)]);
    r.sw.start(args("g")).await.unwrap();
    let done = settle(&r.sw).await;
    assert_eq!(done.state, State::Met);
    assert_eq!(r.fake.cloud_frames.load(Ordering::SeqCst), 2, "one frame for each hit, none for the looks that missed");
    assert_eq!(r.fake.told(), vec!["Screen watch: build finished. (goal: g)"]);
}

#[tokio::test(start_paused = true)]
async fn cloud_confirm_that_fails_is_not_asked_again_and_the_local_hit_stands() {
    let r = rig();
    r.fake.cfg.lock().unwrap().cloud_confirm = true;
    *r.fake.cloud.lock().unwrap() = Some(VecDeque::from([Err("429".to_string())]));
    r.fake.say(&[&reply(true, "done", 0.9)]);
    r.sw.start(args("g")).await.unwrap();
    assert_eq!(settle(&r.sw).await.state, State::Met);
    assert_eq!(r.fake.cloud_frames.load(Ordering::SeqCst), 1);
}

