//! The look's prompt against the real local model (`gemma3:4b` in Ollama on this machine). Gated: it
//! runs only with `KAGENT_IT=1` (local Ollama only, like the other gated tests), and reports what the model said.

use std::io::Cursor;

use super::look::{self, Looker, Prepared};
use super::vision::{ask_local, Api, LocalModel};

fn png(circle: bool) -> Vec<u8> {
    let mut img = image::RgbImage::from_pixel(800, 450, image::Rgb([245, 245, 245]));
    let (cx, cy, r) = (400i32, 225i32, 140i32);
    for y in 0..450i32 {
        for x in 0..800i32 {
            let inside = (x - cx).pow(2) + (y - cy).pow(2) <= r * r;
            if circle && inside {
                img.put_pixel(x as u32, y as u32, image::Rgb([20, 170, 40]));
            } else if !circle && (x - cx).abs() < r && (y - cy).abs() < r {
                img.put_pixel(x as u32, y as u32, image::Rgb([30, 60, 200]));
            }
        }
    }
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

#[tokio::test]
async fn the_prompt_against_the_real_local_model() {
    if std::env::var("KAGENT_IT").as_deref() != Ok("1") {
        eprintln!("skipped: KAGENT_IT=1 runs it against the local Ollama");
        return;
    }
    let base = std::env::var("KAGENT_OLLAMA").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let model = LocalModel { id: "gemma3:4b".into(), base, api: Api::Ollama };
    let goal = "a green circle is visible on the screen";
    let mut verdicts = Vec::new();
    for (name, circle) in [("green circle", true), ("blue square", false)] {
        let Prepared::Frame(jpeg) = Looker::default().prepare(&png(circle), false).unwrap() else { panic!("a frame") };
        // The first call loads the model; the second is the time a look takes.
        let mut took = Vec::new();
        let mut raw = String::new();
        for _ in 0..2 {
            let started = std::time::Instant::now();
            raw = ask_local(&model, &jpeg, goal).await.expect("the local model answers");
            took.push(started.elapsed());
        }
        let parsed = look::parse_reply(&raw);
        eprintln!("S48 gated: image {name:?}: raw reply {raw:?}, parsed {parsed:?}, looks took {took:?}");
        assert!(parsed.is_some(), "the reply must parse as the JSON shape: {raw:?}");
        verdicts.push(parsed.unwrap());
    }
    // Loosely: the model should see the circle and not the square. Reported, not enforced hard.
    if !(verdicts[0].met && !verdicts[1].met) {
        eprintln!("S48 gated: the verdicts were {:?} and {:?}: not what a person would say", verdicts[0], verdicts[1]);
    }
}
