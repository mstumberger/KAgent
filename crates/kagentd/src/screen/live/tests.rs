use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};

use super::*;
use crate::live::brain::select_tools;
use crate::sections::VoiceBrain;

fn spec(name: &str) -> ToolSpec {
    ToolSpec { name: name.into(), description: "d".into(), input_schema: json!({"type": "object"}) }
}

fn names(v: &[ToolSpec]) -> Vec<&str> {
    v.iter().map(|t| t.name.as_str()).collect()
}

/// A picture the size of a monitor, made here: nothing of the owner's screen is ever taken in a test.
fn screenshot(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8]));
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

#[test]
fn screen_is_declared_in_live_and_auto_whatever_the_voice_groups_hold() {
    let advertised = || vec![spec("system_info"), spec("screen"), spec("settings"), spec("terminal")];
    for mode in [VoiceBrain::Live, VoiceBrain::Auto] {
        let tools = select_tools(advertised(), mode, &[]);
        let names = names(&tools);
        assert!(names.contains(&"screen") && names.contains(&"settings"), "{mode:?}: {names:?}");
        assert!(!names.contains(&"system_info") && !names.contains(&"terminal"), "{mode:?}: the groups still decide for the rest: {names:?}");
        let screen = tools.iter().find(|t| t.name == "screen").unwrap();
        assert_eq!(screen.input_schema["properties"].as_object().unwrap().keys().collect::<Vec<_>>(), ["target"], "the live model has no `question`: it looks itself");
        assert!(screen.description.contains("puts it in your view"), "{}", screen.description);
    }
    assert!(!names(&select_tools(advertised(), VoiceBrain::Routed, &["desktop".into()])).contains(&"screen"), "routed: the Brain looks and its answer is read aloud");
    assert!(names(&select_tools(advertised(), VoiceBrain::Auto, &[])).contains(&"think"));
}

#[test]
fn what_is_asked_and_what_is_refused() {
    assert_eq!(question("window"), "I want to look at your active window. Say yes to go ahead, or no.");
    assert_eq!(question("monitor"), "I want to look at your monitor. Say yes to go ahead, or no.");
    assert_eq!(question("desktop"), "I want to look at your whole desktop. Say yes to go ahead, or no.");
    assert_eq!((target_of(&json!({})), target_of(&json!({"target": "monitor"})), target_of(&json!({"target": "desktop"})), target_of(&json!({"target": "anything"}))), ("window", "monitor", "desktop", "window"), "the active window unless more is asked");
    assert_eq!(refusal(true, 0).as_deref(), Some("the screen is locked"));
    assert_eq!(refusal(true, 3).as_deref(), Some("the screen is locked"), "a locked screen first");
    let card = refusal(false, 1).unwrap();
    assert!(card.contains("approval card") && card.contains("never captures its own") && card.contains("Answer the card"), "{card}");
    assert_eq!(refusal(false, 0), None);
    // v0.0.3 A4: only the cards of the same session count.
    let waiting = ["s1", "s2", "s2"];
    assert_eq!(open_cards(waiting, "s1"), 1);
    assert_eq!(open_cards(waiting, "s2"), 2);
    assert_eq!(open_cards(waiting, "s3"), 0);
    assert_eq!(refusal(false, open_cards(waiting, "s3")), None, "another session's card does not refuse the look");
}

#[test]
fn a_frame_is_a_realtime_input_video_message() {
    let msg = video_frame(&[1, 2, 3, 4]);
    assert_eq!(msg, json!({"realtimeInput": {"video": {"mimeType": "image/jpeg", "data": "AQIDBA=="}}}));
}

fn frames(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
    std::iter::from_fn(|| rx.try_recv().ok()).collect()
}

/// One test: it sets the two variables that stand in for the screenshot program and the lock state (a file).
#[tokio::test]
async fn a_live_look_sends_one_jpeg_frame_and_answers_ok() {
    let dir = tempfile::tempdir().unwrap();
    let shot = dir.path().join("fake.png");
    std::fs::write(&shot, screenshot(3000, 1800)).unwrap();
    std::env::set_var("KAGENT_FAKE_SCREENSHOT", &shot);
    let lock = dir.path().join("locked");
    std::env::set_var("KAGENT_FAKE_LOCKED", &lock);
    let providers = Arc::new(kagent_providers::ProviderStore::load(dir.path().join("providers.json"), kagent_providers::ProviderStore::builtin_catalog().unwrap(), &[]).unwrap());
    let screen = crate::screen::Screen::new(providers, Arc::new(OnceLock::new()), dir.path());
    let ctx = |call: &str| {
        let mut ctx = ToolContext::standalone(Duration::from_secs(30), 24_000);
        ctx.call_id = call.into();
        ctx
    };

    // The live model's call: a frame to its session, and the answer of the spec.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let registered = register("live-1", tx);
    let out = screen.call(json!({"target": "monitor"}), ctx("live-1")).await.unwrap();
    assert_eq!((out.is_error, out.content.as_str()), (false, "the screenshot is in your view now"));
    let sent = frames(&mut rx);
    assert_eq!(sent.len(), 1, "one frame");
    let video = &sent[0]["realtimeInput"]["video"];
    assert_eq!(video["mimeType"], "image/jpeg");
    let jpeg = base64::engine::general_purpose::STANDARD.decode(video["data"].as_str().unwrap()).unwrap();
    assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
    let decoded = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (1024, 614), "the longest side is 1024");
    assert!(std::fs::read_dir(dir.path().join("screenshots")).unwrap().next().is_none(), "the screenshot file is not kept");

    // Locked: nothing is captured or sent.
    std::fs::write(&lock, b"").unwrap();
    let out = screen.call(json!({}), ctx("live-1")).await.unwrap();
    assert_eq!((out.is_error, out.content.as_str()), (true, "the screen is locked"));
    assert!(frames(&mut rx).is_empty(), "a locked screen sends no frame");
    std::fs::remove_file(&lock).unwrap();

    // A call nobody registered (the Brain's own `screen`) keeps the vision path: no frame goes anywhere.
    drop(registered);
    let out = screen.call(json!({}), ctx("live-1")).await.unwrap();
    assert!(out.is_error && out.content.contains("no vision model"), "{}", out.content);
    assert!(sink_of("live-1").is_none() && frames(&mut rx).is_empty());
    std::env::remove_var("KAGENT_FAKE_SCREENSHOT");
    std::env::remove_var("KAGENT_FAKE_LOCKED");
}
