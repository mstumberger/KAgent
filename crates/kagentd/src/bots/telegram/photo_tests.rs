//! A photo sent to the bot (vision.md section 2): an image of the message that starts the turn.

use std::sync::atomic::Ordering;

use serde_json::json;

use super::tests::{now, paired, CHAT};
use crate::bots::fakes::text;
use crate::bots::telegram_api::{Message, Update};

fn photo(update_id: i64, sizes: serde_json::Value, caption: Option<&str>) -> Update {
    let mut message = json!({"message_id": update_id + 500, "chat": {"id": CHAT, "type": "private"}, "photo": sizes});
    if let Some(caption) = caption {
        message["caption"] = json!(caption);
    }
    serde_json::from_value(json!({"update_id": update_id, "message": message})).unwrap()
}

fn sizes() -> serde_json::Value {
    json!([{"file_id": "small", "width": 90, "height": 60, "file_size": 1200}, {"file_id": "big", "width": 1280, "height": 853, "file_size": 90000}, {"file_id": "mid", "width": 320, "height": 213, "file_size": 9000}])
}

#[tokio::test]
async fn a_photo_is_an_image_of_the_message_with_its_caption_as_the_text() {
    let r = paired();
    *r.api.file.lock().unwrap() = ("photos/file_7.jpg".into(), vec![0xFF, 0xD8, 1, 2, 3]);
    r.bot.handle(&photo(1, sizes(), Some("what does this say?")), now(0)).await;
    assert_eq!(*r.api.downloads.lock().unwrap(), ["photos/file_7.jpg".to_string()], "the file was downloaded from Telegram");
    let sent = r.agent.image_sends.lock().unwrap().clone();
    assert_eq!(sent, vec![("s1".to_string(), "what does this say?".to_string(), vec![5])], "one picture of five bytes, the caption as the text");
    assert!(r.agent.sends().is_empty(), "the message went through the image path, not as text");
    assert_eq!(r.api.texts().first().map(String::as_str), Some("Working on it..."), "the turn starts like a text turn");
}

#[tokio::test]
async fn a_photo_without_a_caption_is_images_alone() {
    let r = paired();
    *r.api.file.lock().unwrap() = ("photos/file_8.jpg".into(), vec![9; 3]);
    r.bot.handle(&photo(1, sizes(), None), now(0)).await;
    assert_eq!(r.agent.image_sends.lock().unwrap().clone(), vec![("s1".to_string(), String::new(), vec![3])]);
}

#[tokio::test]
async fn a_photo_while_a_turn_runs_queues_as_steering_with_its_image() {
    let r = paired();
    r.agent.running.store(true, Ordering::SeqCst);
    *r.api.file.lock().unwrap() = ("photos/file_9.jpg".into(), vec![1; 4]);
    r.bot.handle(&text(1, CHAT, "look at this"), now(0)).await;
    r.bot.handle(&photo(2, sizes(), Some("here")), now(1)).await;
    assert_eq!(r.agent.sends(), vec![("s1".to_string(), "look at this".to_string())]);
    assert_eq!(r.agent.image_sends.lock().unwrap().clone(), vec![("s1".to_string(), "here".to_string(), vec![4])], "the photo reaches the same session");
    r.agent.running.store(false, Ordering::SeqCst);
}

#[test]
fn the_biggest_size_that_a_bot_can_download_is_taken() {
    let m = |sizes| serde_json::from_value::<Update>(json!({"update_id": 1, "message": {"message_id": 1, "chat": {"id": 1}, "photo": sizes}})).unwrap().message.unwrap();
    let message: Message = m(sizes());
    assert_eq!(message.best_photo().unwrap().file_id, "big");
    let huge = m(json!([{"file_id": "a", "width": 100, "height": 100, "file_size": 1000}, {"file_id": "b", "width": 9000, "height": 9000, "file_size": 30_000_000}]));
    assert_eq!(huge.best_photo().unwrap().file_id, "a", "a size over 20 MB is not downloadable");
    assert!(m(json!([{"file_id": "b", "width": 9000, "height": 9000, "file_size": 30_000_000}])).best_photo().is_none());
    assert!(m(json!([])).best_photo().is_none());
}
