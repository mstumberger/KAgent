//! `web_fetch` and the reader (web.md section 2): when the reader is asked, and what is said when it cannot run.

use serde_json::json;

use super::*;
use crate::settings::Settings;
use crate::web::fake::FakePageReader;
use crate::web::page_reader::ReaderError;

const RENDERED: &str = "<html><head><title>Rendered</title></head><body><main><p>Text that only a browser sees.</p></main></body></html>";
const SHELL: &[u8] = b"<html><head><title>App</title></head><body><div id=\"root\"></div><script src=\"/app.js\"></script></body></html>";
const CHALLENGE: &[u8] = b"<html><head><title>Just a moment...</title></head><body><p>Checking if the site connection is secure</p></body></html>";

async fn site() -> Fake {
    Fake::start(&[
        ("/page", Reply::ok("text/html; charset=utf-8", PAGE)),
        ("/shell", Reply::ok("text/html; charset=utf-8", SHELL)),
        ("/challenge", Reply::ok("text/html; charset=utf-8", CHALLENGE)),
        ("/blocked", Reply::with(403, vec![("content-type", "text/html".into())], b"<h1>no</h1>")),
        ("/missing", Reply::with(404, vec![("content-type", "text/html".into())], b"<h1>gone</h1>")),
    ])
    .await
}

fn with_reader(fake: &Fake, settings: Value, reader: Option<Arc<FakePageReader>>) -> Arc<Web> {
    let mut s = Settings::defaults();
    s.patch(settings).unwrap();
    let cell: crate::web::SettingsCell = Arc::new(OnceLock::new());
    cell.set(Arc::new(std::sync::RwLock::new(s))).unwrap();
    let web = Web::with(cell, Arc::new(MemoryKeys::default()), fake.egress(), Engines::default());
    Arc::new(match reader {
        Some(reader) => web.with_page_reader(reader),
        None => web,
    })
}

#[tokio::test]
async fn render_true_reads_through_the_reader_and_a_plain_page_does_not() {
    let fake = site().await;
    let reader = FakePageReader::html(RENDERED);
    let tool = with_reader(&fake, json!({}), Some(reader.clone())).fetch_tool();
    let plain = call(&tool, json!({"url": fake.url("/page")})).await;
    assert!(plain.content.contains("Hi") && reader.asked.lock().unwrap().is_empty(), "an ordinary page is read by plain HTTP: {}", plain.content);
    let out = call(&tool, json!({"url": fake.url("/page"), "render": true, "max_chars": 2000})).await;
    assert!(!out.is_error && out.content.contains("Title: Rendered") && out.content.contains("Text that only a browser sees."), "{}", out.content);
    assert_eq!(*reader.asked.lock().unwrap(), vec![fake.url("/page")]);
    assert!(fake.seen("/page").len() == 1, "render does not fetch plain first");
}

#[tokio::test]
async fn a_shell_a_challenge_and_a_refusal_fall_back_to_the_reader() {
    let fake = site().await;
    for path in ["/shell", "/challenge", "/blocked"] {
        let reader = FakePageReader::html(RENDERED);
        let tool = with_reader(&fake, json!({}), Some(reader.clone())).fetch_tool();
        let out = call(&tool, json!({"url": fake.url(path)})).await;
        assert!(!out.is_error && out.content.contains("Text that only a browser sees."), "{path}: {}", out.content);
        assert_eq!(reader.asked.lock().unwrap().len(), 1, "{path}");
    }
    // A 404 is a 404: the reader is not asked.
    let reader = FakePageReader::html(RENDERED);
    let tool = with_reader(&fake, json!({}), Some(reader.clone())).fetch_tool();
    let out = call(&tool, json!({"url": fake.url("/missing")})).await;
    assert!(out.is_error && out.content == "the page answered 404 Not Found" && reader.asked.lock().unwrap().is_empty(), "{}", out.content);
}

#[tokio::test]
async fn a_site_of_the_owners_list_is_always_read_by_the_reader() {
    let fake = site().await;
    let reader = FakePageReader::html(RENDERED);
    let tool = with_reader(&fake, json!({"web": {"readerSites": ["127.0.0.1"]}}), Some(reader.clone())).fetch_tool();
    let out = call(&tool, json!({"url": fake.url("/page")})).await;
    assert!(out.content.contains("Text that only a browser sees."), "{}", out.content);
    assert!(fake.requests().is_empty(), "the plain request was not made");
}

#[tokio::test]
async fn a_challenge_that_does_not_clear_is_not_done() {
    let fake = site().await;
    let reader = FakePageReader::failing(ReaderError::Challenge(fake.url("/challenge")));
    let tool = with_reader(&fake, json!({}), Some(reader)).fetch_tool();
    let out = call(&tool, json!({"url": fake.url("/challenge")})).await;
    assert!(out.is_error && out.content.starts_with("not done: this site asks for a human check. Next: tell the owner"), "{}", out.content);
}

#[tokio::test]
async fn without_a_reader_the_plain_answer_stands_and_the_note_comes_once() {
    let fake = site().await;
    for (reader, why) in [(None, "no Chromium-family browser was found"), (Some(FakePageReader::failing(ReaderError::NoBrowser)), "no Chromium-family browser was found")] {
        let tool = with_reader(&fake, json!({}), reader).fetch_tool();
        let out = call(&tool, json!({"url": fake.url("/page"), "render": true})).await;
        assert!(!out.is_error && out.content.contains("Hi"), "plain HTTP still works: {}", out.content);
        assert_eq!(out.content.matches("the reader could not run").count(), 1, "said once: {}", out.content);
        assert!(out.content.contains(why), "{}", out.content);
        let out = call(&tool, json!({"url": fake.url("/blocked")})).await;
        assert!(out.is_error && out.content.starts_with("the page answered 403 Forbidden\n[the reader could not run: "), "{}", out.content);
        // An ordinary page needs no reader and gets no note.
        assert!(!call(&tool, json!({"url": fake.url("/page")})).await.content.contains("reader"), "no note when it was not needed");
    }
    let tool = with_reader(&fake, json!({"web": {"reader": false}}), Some(FakePageReader::html(RENDERED))).fetch_tool();
    let out = call(&tool, json!({"url": fake.url("/shell"), "render": true})).await;
    assert!(out.content.contains("the reader is off (Settings -> Web)"), "{}", out.content);
}
