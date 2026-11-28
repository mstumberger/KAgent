//! The owner's side of the reader (web.md section 9): the window for a human check, the
//! warmed sites, forgetting, clearing the profile.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::manage::ManageError;
use super::*;
use crate::web::fake::{Fake, Reply};
use crate::web::Web;

/// A "browser" that opens no window: it lives for `seconds`, as a window the owner closes.
fn script_browser(dir: &Path, seconds: u32) -> String {
    let path = dir.join("fake-browser.sh");
    std::fs::write(&path, format!("#!/bin/sh\nsleep {seconds}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let limit = tokio::time::Instant::now() + Duration::from_secs(15);
    while !done() {
        assert!(tokio::time::Instant::now() < limit, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn reader_with(data: &Path, bin: Option<String>, egress: Arc<dyn crate::web::guard::Egress>) -> Arc<Reader> {
    Reader::new(data, Arc::new(move || bin.clone()), IDLE, egress)
}

#[tokio::test]
async fn the_window_is_one_at_a_time_and_the_host_is_warmed_when_it_closes() {
    let data = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let reader = reader_with(data.path(), Some(script_browser(tools.path(), 2)), Arc::new(crate::web::guard::Unchecked));
    for bad in ["http://127.0.0.1:7878/", "http://localhost/", "ftp://example.com/", "https://u:p@example.com/", "not a url", ""] {
        assert!(matches!(reader.open_window(bad).await, Err(ManageError::Bad(_))), "{bad:?}");
    }
    assert_eq!(reader.open_window("https://news.example.com/article?x=1").await, Ok("news.example.com".to_string()));
    let status = reader.status().await;
    assert!(status.window_open && status.warmed.is_empty(), "not warmed until it closes");
    assert!(matches!(reader.open_window("https://other.example.org/").await, Err(ManageError::Busy(_))), "one window");
    assert_eq!(reader.render("https://news.example.com/").await, Err(ReaderError::WindowOpen), "the profile is in use");
    assert!(matches!(reader.forget("news.example.com").await, Err(ManageError::Busy(_))));
    assert!(matches!(reader.clear_profile().await, Err(ManageError::Busy(_))));
    until("the window closed", || !reader.window.lock().unwrap().is_some()).await;
    until("the host was saved", || !reader.sites.list().is_empty()).await;
    let status = reader.status().await;
    assert!(!status.window_open && status.warmed.len() == 1 && status.warmed[0].host == "news.example.com", "{status:?}");
    assert_eq!(status.profile, data.path().join("reader-profile").display().to_string());
}

#[tokio::test]
async fn the_window_is_refused_while_a_page_is_read_and_without_a_browser() {
    let data = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let reader = reader_with(data.path(), Some(script_browser(tools.path(), 1)), Arc::new(crate::web::guard::Unchecked));
    let reading = reader.state.lock().await;
    assert!(matches!(reader.open_window("https://news.example.com/").await, Err(ManageError::Busy(why)) if why.contains("being read")));
    assert!(matches!(reader.forget("news.example.com").await, Err(ManageError::Busy(_))));
    assert!(matches!(reader.clear_profile().await, Err(ManageError::Busy(_))));
    drop(reading);
    let none = reader_with(data.path(), None, Arc::new(crate::web::guard::Unchecked));
    assert_eq!(none.open_window("https://news.example.com/").await, Err(ManageError::NoBrowser));
    assert!(none.status().await.browser.is_none());
}

#[tokio::test]
async fn clearing_the_profile_deletes_it_and_the_warmed_list() {
    let data = tempfile::tempdir().unwrap();
    let reader = reader_with(data.path(), None, Arc::new(crate::web::guard::Unchecked));
    std::fs::create_dir_all(reader.profile().join("Default")).unwrap();
    std::fs::write(reader.profile().join("Default/Cookies"), "x").unwrap();
    reader.sites.add("a.example.com", 1).unwrap();
    reader.clear_profile().await.unwrap();
    assert!(!reader.profile().exists() && reader.sites.list().is_empty());
    reader.clear_profile().await.unwrap();
}

#[tokio::test]
async fn the_routes_answer_with_their_codes() {
    use crate::web::reader::http;
    let data = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let reader = reader_with(data.path(), Some(script_browser(tools.path(), 1)), Arc::new(crate::web::guard::Unchecked));
    let web = Web::with(Arc::new(std::sync::OnceLock::new()), Arc::new(crate::web::fake::MemoryKeys::default()), Arc::new(crate::web::guard::Unchecked), crate::web::Engines::default()).with_reader(reader.clone());
    let code = |r: crate::server::ApiResult<serde_json::Value>| r.err().map(|e| (e.0.as_u16(), e.1));
    assert_eq!(code(http::open_window(&web, json!({"url": "http://169.254.169.254/"})).await), Some((400, "invalid_input")));
    assert_eq!(code(http::open_window(&web, json!({})).await), Some((400, "invalid_input")));
    assert_eq!(http::open_window(&web, json!({"url": "https://news.example.com/"})).await.ok(), Some(json!({"host": "news.example.com"})));
    assert_eq!(code(http::open_window(&web, json!({"url": "https://news.example.com/"})).await), Some((409, "busy")));
    let status = http::status(&web).await.ok().unwrap();
    assert_eq!((status["enabled"].clone(), status["windowOpen"].clone()), (json!(true), json!(true)));
    until("closed", || reader.window.lock().unwrap().is_none()).await;
    let bare = Web::with(Arc::new(std::sync::OnceLock::new()), Arc::new(crate::web::fake::MemoryKeys::default()), Arc::new(crate::web::guard::Unchecked), crate::web::Engines::default());
    assert_eq!(code(http::status(&bare).await), Some((404, "not_found")));
}

/// Scenario R3: a site that asks for a check is passed once in the owner's window (here: a read that
/// sets the cookie); the clearance is in the profile for the next browser, and Forget takes it away.
#[tokio::test]
async fn a_warmed_site_is_read_after_the_clearance_and_forget_takes_it_away() {
    let challenge = "<html><head><title>Just a moment...</title></head><body><p>Checking if the site connection is secure</p></body></html>";
    let real = "<html><head><title>Members</title></head><body><main><p>The members page</p></main></body></html>";
    let warm = Reply::with(200, vec![("content-type", "text/html".into()), ("set-cookie", "clearance=1; Path=/; Max-Age=3600".into())], b"<html><body>warmed</body></html>");
    let guarded = Reply::ok("text/html; charset=utf-8", real.as_bytes()).when_cookie("clearance", Reply::with(403, vec![("content-type", "text/html".into())], challenge.as_bytes()));
    let fake = Fake::start(&[("/warm", warm), ("/guarded", guarded)]).await;
    let data = tempfile::tempdir().unwrap();
    if crate::browser::find_chromium().is_none() {
        eprintln!("skipped: no Chromium-family browser is installed on this machine");
        return;
    }
    let reader = reader_with(data.path(), crate::browser::find_chromium(), fake.egress());
    reader.set_challenge_wait(Duration::from_secs(2));
    assert_eq!(reader.render(&fake.url("/guarded")).await, Err(ReaderError::Challenge(fake.url("/guarded"))), "before the warm-up");
    reader.render(&fake.url("/warm")).await.unwrap();
    // The browser is closed (the owner closes his window): the cookie is in the profile.
    reader.shutdown().await;
    let page = reader.render(&fake.url("/guarded")).await.unwrap();
    assert!(page.html.contains("The members page"), "{}", page.html);
    reader.sites.add("127.0.0.1", 1).unwrap();
    reader.forget("127.0.0.1").await.unwrap();
    assert!(reader.sites.list().is_empty());
    reader.shutdown().await;
    assert_eq!(reader.render(&fake.url("/guarded")).await, Err(ReaderError::Challenge(fake.url("/guarded"))), "the cookie went with Forget");
    reader.shutdown().await;
}
