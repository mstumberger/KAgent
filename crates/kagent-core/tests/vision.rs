//! Scenarios S46 (vision.md section 1) with a scripted model: the images of a message are
//! stored with it.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use kagent_core::agent::{SendOutcome, TurnOpts};
use kagent_core::model::ModelResponse;
use kagent_core::protocol::{ImageRef, TurnStatus, Via};

fn answer(text: &str) -> Result<ModelResponse, String> {
    Ok(ModelResponse { text: text.into(), ..Default::default() })
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(40, 30, |x, _| if x % 5 < 2 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

async fn rig_of(script: Vec<Result<ModelResponse, String>>) -> Rig {
    rig_with(ScriptedModel::scripted(script), Arc::new(Mutex::new(Spans::default())), common::config()).await
}

async fn send(r: &Rig, text: &str, images: Vec<ImageRef>) -> anyhow::Result<kagent_core::agent::TurnResult> {
    match r.agent.send_with(&r.session, text.into(), Via::Text, TurnOpts { images, ..Default::default() }).await? {
        SendOutcome::Started { handle, .. } => Ok(handle.wait().await),
        SendOutcome::Queued => panic!("nothing runs"),
    }
}

#[tokio::test]
async fn the_images_of_a_message_are_stored_with_it() {
    let r = rig_of(vec![answer("seen")]).await;
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    let result = send(&r, "what is this", vec![image.clone()]).await.unwrap();
    assert_eq!(result.status, TurnStatus::Completed);
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored[0].images, vec![image.clone()], "the reference is on the user message");
    assert_eq!(r.agent.messages(&r.session).await.unwrap()[0].images, vec![image.clone()]);
    let raw = std::fs::read_to_string(r._dir.path().join(format!("{}.jsonl", r.session))).unwrap();
    assert!(raw.lines().next().unwrap().contains(&image.id) && !raw.contains("iVBOR"), "the file has the id, not the pixels");
}

