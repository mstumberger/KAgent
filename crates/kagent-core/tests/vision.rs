//! Scenarios S46 (vision.md sections 1 and 2) with a scripted model: the images of a
//! message are stored, the Brain gets pixels or a description made once, a turn nobody can
//! read is refused before it starts, only the last 6 messages carry pixels, and compaction
//! turns images into descriptions.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use kagent_core::agent::vision::{Refused, VisionPlan, NO_VISION};
use kagent_core::agent::{SendOutcome, TurnOpts};
use kagent_core::model::ModelResponse;
use kagent_core::protocol::{AgentEvent, ImageRef, Role, TurnStatus, Via};

fn answer(text: &str) -> Result<ModelResponse, String> {
    Ok(ModelResponse { text: text.into(), ..Default::default() })
}

fn png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(40, 30, |x, _| if x % 5 < 2 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

fn plan(brain_reads: bool, describer: Option<&str>) -> kagent_core::agent::vision::VisionSource {
    let describer = describer.map(str::to_string);
    Arc::new(move |_| VisionPlan { brain_reads, describer: describer.clone() })
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


#[tokio::test]
async fn a_brain_that_reads_images_gets_pixels() {
    let r = rig_of(vec![answer("a dialog")]).await;
    r.agent.set_vision_source(plan(true, Some("vis::m")));
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    send(&r, "what is this", vec![image.clone()]).await.unwrap();
    let requests = r.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "no vision model call: the Brain reads it");
    let user = &requests[0][0];
    let pixels = user.images[0].data.as_ref().expect("pixels in the request");
    assert_eq!(pixels.0.len() as u64, image.bytes, "the stored file is what is sent");
    assert_eq!(user.content, "what is this", "no description in the text");
    assert!(r.store.load_messages(&r.session).await.unwrap()[0].images[0].description.is_none());
}

#[tokio::test]
async fn a_brain_that_cannot_read_gets_the_description_made_once() {
    let r = rig_of(vec![answer("Dialog: Disk full. OK button."), answer("it says the disk is full"), answer("yes")]).await;
    r.agent.set_vision_source(plan(false, Some("vis::eye-1")));
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    send(&r, "what is this", vec![image]).await.unwrap();
    {
        let refs = r.model.model_refs.lock().unwrap().clone();
        assert_eq!(refs.iter().map(|(m, _)| m.as_str()).collect::<Vec<_>>(), ["vis::eye-1", "fake::test"], "the vision model first, then the Brain");
        let requests = r.model.requests.lock().unwrap().clone();
        assert!(requests[0][0].images[0].data.is_some() && requests[0][0].content.starts_with("Describe this image for another model"), "the vision model got the pixels and the question");
        let brain = &requests[1][0];
        assert!(brain.images.is_empty(), "the Brain gets no pixels");
        assert!(brain.content.contains("[image 1, read by eye-1: Dialog: Disk full. OK button.]"), "{}", brain.content);
    }
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert_eq!(stored[0].images[0].description.as_deref(), Some("Dialog: Disk full. OK button."), "the description is stored with the message");
    assert_eq!(stored[0].content, "what is this", "the text of the message is as the owner wrote it");

    // The next turn describes nothing again.
    send(&r, "and now?", vec![]).await.unwrap();
    let refs = r.model.model_refs.lock().unwrap().clone();
    assert_eq!(refs.len(), 3, "one more request: the Brain's");
    assert_eq!(refs[2].0, "fake::test");
    let requests = r.model.requests.lock().unwrap().clone();
    assert!(requests[2][0].content.contains("Disk full"), "the description is in the history of the next turn");
}

#[tokio::test]
async fn no_vision_model_refuses_the_turn_before_it_starts() {
    let mut r = rig_of(vec![]).await;
    r.agent.set_vision_source(plan(false, None));
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    let err = send(&r, "what is this", vec![image]).await.unwrap_err();
    let refused = err.downcast_ref::<Refused>().expect("a refusal, not a failure");
    assert_eq!(refused.0, "The Brain cannot read images: pick a vision model in Settings -> Models.");
    assert_eq!(refused.0, NO_VISION);
    let mut notices = Vec::new();
    while let Ok(ev) = r.events.try_recv() {
        match ev {
            AgentEvent::Notice { text, session_id, .. } => notices.push((text, session_id)),
            AgentEvent::TurnStarted { .. } => panic!("the turn must not start"),
            _ => {}
        }
    }
    assert_eq!(notices, vec![(NO_VISION.to_string(), Some(r.session.clone()))], "the owner sees it as a notice of this session");
    assert!(r.store.load_messages(&r.session).await.unwrap().is_empty(), "nothing was stored");
    assert!(r.model.requests.lock().unwrap().is_empty(), "no model was asked");
    assert!(!r.agent.is_running(&r.session).await, "the session is idle");
    // Text alone still works.
    r.model.queue.lock().unwrap().push(answer("fine"));
    assert_eq!(send(&r, "hello", vec![]).await.unwrap().status, TurnStatus::Completed);
}

#[tokio::test]
async fn a_failing_vision_model_fails_the_turn_with_what_to_do() {
    let r = rig_of(vec![Err("503 unavailable".into())]).await;
    r.agent.set_vision_source(plan(false, Some("vis::eye")));
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    let result = send(&r, "what is this", vec![image]).await.unwrap();
    assert_eq!(result.status, TurnStatus::Failed);
    let error = result.error.unwrap();
    assert!(error.contains("vis::eye") && error.contains("could not read the image"), "{error}");
}

#[tokio::test]
async fn only_the_last_six_messages_carry_pixels() {
    let r = rig_of((0..6).map(|i| answer(&format!("a{i}"))).collect()).await;
    r.agent.set_vision_source(plan(true, None));
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    send(&r, "look", vec![image]).await.unwrap();
    for i in 0..5 {
        send(&r, &format!("more {i}"), vec![]).await.unwrap();
    }
    let requests = r.model.requests.lock().unwrap().clone();
    // Request 3 has 5 messages: the message with the image is the 5th from the end.
    assert_eq!(requests[2].len(), 5);
    assert!(requests[2][0].images.first().and_then(|i| i.data.as_ref()).is_some(), "within the last 6 messages: pixels");
    // Request 4 has 7: the image is the 7th from the end.
    assert_eq!(requests[3].len(), 7);
    let old = &requests[3][0];
    assert!(old.images.is_empty(), "older than the last 6 messages: no pixels");
    assert!(old.content.contains("[image 1: not shown to this model]"), "{}", old.content);
    assert!(requests[5].iter().all(|m| m.images.iter().all(|i| i.data.is_some())), "an image that is sent has its pixels");
}

#[tokio::test]
async fn compaction_turns_the_images_of_what_it_summarizes_into_descriptions() {
    // The Brain reads images, so the message has no description yet: compaction asks for it.
    let r = rig_of(vec![answer("first"), answer("second"), answer("Terminal: error 13"), answer("The user showed an error.")]).await;
    r.agent.set_vision_source(plan(true, Some("vis::eye")));
    let image = r.store.put_image(&r.session, png()).await.unwrap();
    send(&r, "look", vec![image]).await.unwrap();
    send(&r, "and?", vec![]).await.unwrap();
    // The history is long enough for a compaction by hand: four stored messages, then a last one.
    r.agent.append_external(&r.session, vec![kagent_core::protocol::Message::user("tail"), kagent_core::protocol::Message::assistant("ok", vec![]), kagent_core::protocol::Message::user("last")]).await.unwrap();
    let view = r.agent.compact(&r.session).await.unwrap();
    assert_eq!(view.summary, "The user showed an error.");
    let requests = r.model.requests.lock().unwrap().clone();
    let refs = r.model.model_refs.lock().unwrap().clone();
    assert_eq!(refs[2].0, "vis::eye", "the description was made then");
    let summary_request = requests.last().unwrap();
    assert!(summary_request.iter().all(|m| m.images.is_empty()), "no pixels go to the summarizer");
    let user = summary_request.iter().find(|m| m.role == Role::User && m.content.starts_with("look")).unwrap();
    assert!(user.content.contains("[image 1, read by eye: Terminal: error 13]"), "{}", user.content);
}
