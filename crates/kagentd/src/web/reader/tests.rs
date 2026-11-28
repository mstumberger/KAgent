//! The reader against a loopback fake site and a real headless browser. A machine without
//! a Chromium-family browser skips them, with the reason printed; the reader's temporary
//! profile is under a temporary data folder, never the owner's.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::web::html;
use crate::web::fake::{Fake, Reply};

const JS_PAGE: &str = "<html><head><title>JS page</title></head><body><main><div id=\"a\">waiting</div></main><script>document.getElementById('a').textContent = 'Rendered by a script';</script></body></html>";
const CLEARS: &str = "<html><head><title>Just a moment...</title></head><body><p>Checking if the site connection is secure</p>\
    <script>setTimeout(function () { document.title = 'The real page'; document.body.innerHTML = '<main><p>Behind the check</p></main>'; }, 1500);</script></body></html>";
const STAYS: &str = "<html><head><title>Just a moment...</title></head><body><p>Checking if the site connection is secure</p><div class=\"cf-challenge\"></div></body></html>";
const DIALOG: &str = "<html><head><title>Dialogs</title></head><body><script>alert('hello'); confirm('sure?'); prompt('name?');</script><main><p>After the dialogs</p></main></body></html>";

async fn site() -> Fake {
    Fake::start(&[
        ("/js", Reply::ok("text/html; charset=utf-8", JS_PAGE.as_bytes())),
        ("/clears", Reply::ok("text/html; charset=utf-8", CLEARS.as_bytes())),
        ("/stays", Reply::with(403, vec![("content-type", "text/html; charset=utf-8".into())], STAYS.as_bytes())),
        ("/dialog", Reply::ok("text/html; charset=utf-8", DIALOG.as_bytes())),
        ("/missing", Reply::with(404, vec![("content-type", "text/html".into())], b"<h1>gone</h1>")),
        ("/file", Reply::with(200, vec![("content-type", "application/octet-stream".into()), ("content-disposition", "attachment; filename=a.bin".into())], &[1u8; 2000])),
    ])
    .await
}

/// The reader over a temporary data folder and the browser of this machine, or `None` (printed) when there is none.
fn reader(fake: &Fake, data: &Path, idle: Duration) -> Option<Arc<Reader>> {
    if crate::browser::find_chromium().is_none() {
        eprintln!("skipped: no Chromium-family browser is installed on this machine");
        return None;
    }
    Some(Reader::new(data, Arc::new(crate::browser::find_chromium), idle, fake.egress()))
}

#[tokio::test]
async fn no_browser_on_the_machine_is_said_and_nothing_starts() {
    let fake = site().await;
    let data = tempfile::tempdir().unwrap();
    let reader = Reader::new(data.path(), Arc::new(|| None), IDLE, fake.egress());
    assert_eq!(reader.render(&fake.url("/js")).await, Err(ReaderError::NoBrowser));
    assert!(!reader.profile().exists() && !reader.running().await);
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn a_page_that_needs_javascript_is_read_after_the_script_ran() {
    let fake = site().await;
    let data = tempfile::tempdir().unwrap();
    let Some(reader) = reader(&fake, data.path(), IDLE) else { return };
    let page = reader.render(&fake.url("/js")).await.unwrap();
    assert!(page.html.contains("Rendered by a script") && !page.html.contains("waiting"), "{}", page.html);
    assert_eq!(page.url, fake.url("/js"));
    // The plain text of it, by the same extraction as plain HTTP.
    let text = html::to_text(&page.html, &reqwest::Url::parse(&page.url).unwrap());
    assert_eq!((text.title.as_str(), text.text.as_str()), ("JS page", "Rendered by a script"));
    // The reader's own profile, under the data folder, and a second read reuses the browser.
    assert_eq!(reader.profile(), data.path().join("reader-profile"));
    assert!(reader.profile().join("DevToolsActivePort").exists() || reader.profile().is_dir());
    let port = reader.state.lock().await.as_ref().map(|r| r.port);
    reader.render(&fake.url("/js")).await.unwrap();
    assert_eq!(reader.state.lock().await.as_ref().map(|r| r.port), port, "the browser is the same");
    reader.shutdown().await;
    assert!(!reader.running().await, "gone at shutdown");
}

#[tokio::test]
async fn the_browser_is_closed_after_the_idle_time() {
    let fake = site().await;
    let data = tempfile::tempdir().unwrap();
    let Some(reader) = reader(&fake, data.path(), Duration::from_millis(600)) else { return };
    reader.render(&fake.url("/js")).await.unwrap();
    assert!(reader.running().await, "right after a read it runs");
    let until = tokio::time::Instant::now() + Duration::from_secs(15);
    while reader.running().await && tokio::time::Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!reader.running().await, "the idle timer closed it");
    // And the next read starts it again.
    assert!(reader.render(&fake.url("/js")).await.is_ok());
    reader.shutdown().await;
}

#[tokio::test]
async fn a_status_a_download_and_dialogs_are_handled_without_acting() {
    let fake = site().await;
    let data = tempfile::tempdir().unwrap();
    let Some(reader) = reader(&fake, data.path(), IDLE) else { return };
    assert_eq!(reader.render(&fake.url("/missing")).await, Err(ReaderError::Status(404)));
    let download = reader.render(&fake.url("/file")).await;
    assert!(matches!(&download, Err(ReaderError::Failed(why)) if why.contains("download")), "{download:?}");
    let page = tokio::time::timeout(Duration::from_secs(30), reader.render(&fake.url("/dialog"))).await.expect("dialogs do not hang the read").unwrap();
    assert!(page.html.contains("After the dialogs"), "{}", page.html);
    let leftovers: Vec<_> = std::fs::read_dir(data.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers, ["reader-profile"], "nothing was downloaded next to the profile: {leftovers:?}");
    reader.shutdown().await;
}

#[tokio::test]
async fn a_challenge_that_clears_is_read_and_one_that_stays_is_a_challenge() {
    let fake = site().await;
    let data = tempfile::tempdir().unwrap();
    let Some(reader) = reader(&fake, data.path(), IDLE) else { return };
    reader.set_challenge_wait(Duration::from_secs(10));
    let page = reader.render(&fake.url("/clears")).await.unwrap();
    assert!(page.html.contains("Behind the check") && page.html.contains("The real page"), "{}", page.html);
    reader.set_challenge_wait(Duration::from_secs(2));
    assert_eq!(reader.render(&fake.url("/stays")).await, Err(ReaderError::Challenge(fake.url("/stays"))));
    reader.shutdown().await;
}
