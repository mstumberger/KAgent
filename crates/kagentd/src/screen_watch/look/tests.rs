use super::*;
use std::io::Cursor;

fn png(w: u32, h: u32, rgb: [u8; 3], patch: Option<(u32, u32, u32, u32, [u8; 3])>) -> Vec<u8> {
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    if let Some((x0, y0, pw, ph, c)) = patch {
        for y in y0..y0 + ph {
            for x in x0..x0 + pw {
                img.put_pixel(x, y, image::Rgb(c));
            }
        }
    }
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

#[test]
fn the_skip_is_under_one_and_a_half_percent_and_has_a_limit_of_six() {
    let base = png(640, 360, [20, 20, 20], None);
    let mut l = Looker::default();
    assert!(matches!(l.prepare(&base, false).unwrap(), Prepared::Frame(_)), "the first look always goes to the model");
    // The same frame: skipped six times in a row, the seventh goes to the model.
    for i in 0..6 {
        assert!(matches!(l.prepare(&base, false).unwrap(), Prepared::Skip), "skip {}", i + 1);
    }
    assert!(matches!(l.prepare(&base, false).unwrap(), Prepared::Frame(_)), "no look is skipped more than 6 times in a row");
    assert!(matches!(l.prepare(&base, false).unwrap(), Prepared::Skip), "and the count starts over");
    // A bar over about 4 % of the frame is a change.
    let changed = png(640, 360, [20, 20, 20], Some((0, 0, 640, 15, [240, 240, 240])));
    assert!(matches!(l.prepare(&changed, false).unwrap(), Prepared::Frame(_)));
}

#[test]
fn a_change_just_under_the_share_is_skipped_and_just_over_is_not() {
    let thumb = |a: &[u8], n: usize| {
        let mut b = a.to_vec();
        for p in b.iter_mut().take(n) {
            *p = 255;
        }
        b
    };
    let a = vec![0u8; 2304];
    // 1.5 % of 2304 is 34.56 pixels.
    assert!(should_skip(Some(&a), &thumb(&a, 34), 0));
    assert!(!should_skip(Some(&a), &thumb(&a, 35), 0));
    assert!(!should_skip(None, &a, 0), "nothing looked at yet");
    assert!(!should_skip(Some(&a), &a, MAX_SKIPS), "the limit");
    assert!(!should_skip(Some(&a), &a[..100], 0), "another size is a change");
    // Grey noise below the step is no change.
    let noisy: Vec<u8> = a.iter().map(|p| p + 10).collect();
    assert!(should_skip(Some(&a), &noisy, 0));
}

#[test]
fn a_forced_look_is_never_skipped() {
    let base = png(64, 36, [9, 9, 9], None);
    let mut l = Looker::default();
    l.prepare(&base, false).unwrap();
    assert!(matches!(l.prepare(&base, true).unwrap(), Prepared::Frame(_)));
}

#[test]
fn the_frame_for_the_model_is_a_jpeg_of_at_most_1024() {
    let big = png(2000, 1000, [30, 60, 90], None);
    let Prepared::Frame(jpeg) = Looker::default().prepare(&big, false).unwrap() else { panic!("a frame") };
    assert_eq!(&jpeg[..2], &[0xff, 0xd8], "a JPEG");
    let img = image::load_from_memory(&jpeg).unwrap();
    assert_eq!((img.width(), img.height()), (1024, 512));
    assert!(Looker::default().prepare(b"not an image", false).is_err());
}

#[test]
fn the_prompt_holds_the_goal_and_asks_for_json_only() {
    let p = prompt("tell me when the build finishes");
    assert!(p.contains("Goal: tell me when the build finishes") && p.contains("Answer JSON only") && p.contains("\"met\": true|false") && p.contains("at most 12 words") && p.contains("\"confidence\": 0..1"));
}

#[test]
fn a_clean_reply_is_a_verdict() {
    let v = parse_reply(r#"{"met": true, "note": "build finished, 0 errors", "confidence": 0.9}"#).unwrap();
    assert_eq!(v, Verdict { met: true, note: "build finished, 0 errors".into(), confidence: 0.9 });
    assert!(!parse_reply(r#"{"met": false, "note": "", "confidence": 0}"#).unwrap().met);
}

#[test]
fn what_small_models_wrap_around_the_json_is_tolerated() {
    let fenced = "```json\n{\"met\": false, \"note\": \"still compiling\", \"confidence\": 0.8}\n```";
    assert_eq!(parse_reply(fenced).unwrap().note, "still compiling");
    let chatty = "Sure! Here is my answer: {\"met\": true, \"note\": \"done\", \"confidence\": 0.7} Hope that helps.";
    assert!(parse_reply(chatty).unwrap().met);
    let strings = r#"{"met": "true", "note": "done", "confidence": "85%"}"#;
    let v = parse_reply(strings).unwrap();
    assert!(v.met && (v.confidence - 0.85).abs() < 1e-6);
    let percent = r#"{"met": true, "note": "x", "confidence": 90}"#;
    assert!((parse_reply(percent).unwrap().confidence - 0.9).abs() < 1e-6);
    let nested_note = r#"{"met": true, "note": "a {brace} inside", "confidence": 1}"#;
    assert_eq!(parse_reply(nested_note).unwrap().note, "a {brace} inside");
}

#[test]
fn every_malformed_reply_is_unreadable() {
    for bad in ["", "   ", "yes, the build is done", "[true]", "{}", r#"{"note": "x", "confidence": 1}"#, r#"{"met": 1, "note": "x"}"#, r#"{"met": "maybe"}"#, r#"{"met": null}"#, "{\"met\": true", "}{"] {
        assert_eq!(parse_reply(bad), None, "{bad:?}");
    }
}

#[test]
fn a_missing_or_odd_confidence_needs_the_second_look() {
    let none = parse_reply(r#"{"met": true, "note": "x"}"#).unwrap();
    assert!(none.confidence < LOW_CONFIDENCE);
    let silly = parse_reply(r#"{"met": true, "note": "x", "confidence": "high"}"#).unwrap();
    assert!(silly.confidence < LOW_CONFIDENCE);
    assert_eq!(parse_reply(r#"{"met": true, "confidence": 7000}"#).unwrap().confidence, 1.0);
    assert_eq!(parse_reply(r#"{"met": true, "confidence": -3}"#).unwrap().confidence, 0.0);
}

#[test]
fn a_note_is_twelve_words_on_one_line() {
    let long = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen";
    assert_eq!(short_note(long), "one two three four five six seven eight nine ten eleven twelve");
    assert_eq!(short_note("line\none\n\ntwo"), "line one two");
    assert!(short_note(&"x".repeat(500)).len() <= 140);
}
