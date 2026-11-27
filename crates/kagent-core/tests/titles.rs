//! Session titles set in the background (`agent/title.rs`): they land in the session's meta
//! in memory, under its lock, so no later save brings back the placeholder and no title
//! save drops a change made meanwhile (v0.0.2 section 4).

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use kagent_core::model::ModelResponse;
use kagent_core::protocol::{AgentEvent, Message};
use kagent_core::todo::{TodoItemInput, TodoStatus};

fn item(content: &str) -> TodoItemInput {
    TodoItemInput { content: content.into(), status: Some(TodoStatus::Pending), priority: None }
}

#[tokio::test]
async fn a_background_title_and_a_todo_change_both_stay() {
    let mut r = rig(vec![ModelResponse { text: "Disk usage check".into(), ..Default::default() }], Arc::new(Mutex::new(Spans::default()))).await;
    // A session with a placeholder title and one message, loaded in memory.
    let mut meta = r.store.create("New session").await.unwrap();
    r.store.append(&meta.id, &[Message::user("why is my disk so full today")]).await.unwrap();
    meta.message_count = 1;
    r.store.save_meta(&meta).await.unwrap();
    let sid = meta.id.clone();
    r.agent.set_todo(&sid, vec![item("look at the disk")]).await.unwrap();

    r.agent.backfill_titles().await;
    let quick = r.store.meta(&sid).await.unwrap();
    assert_eq!(quick.title, "Why is my disk so full today", "the quick title at once");
    assert_eq!((quick.todo.len(), quick.message_count), (1, 1), "the todo list and the count are kept");
    wait_event(&mut r.events, 5, |e| matches!(e, AgentEvent::SessionUpdated { title, .. } if title == "Disk usage check")).await;

    // A save of the meta in memory after the title came: the title stays.
    r.agent.set_todo(&sid, vec![item("look at the disk"), item("clean the cache")]).await.unwrap();
    let saved = r.store.meta(&sid).await.unwrap();
    assert_eq!(saved.title, "Disk usage check", "the meta in memory has the refined title");
    assert_eq!((saved.todo.len(), saved.message_count), (2, 1));
}
