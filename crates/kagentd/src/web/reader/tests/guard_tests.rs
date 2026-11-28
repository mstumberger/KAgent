//! The egress guard inside the browser (web.md section 8): a public page that redirects
//! to a local address, or that asks for one, gets nothing. The fake site is on loopback,
//! so the test egress lets that one address through and nothing else; a second server on
//! loopback plays "the daemon's own API": if it sees a request, the guard failed.

use super::*;

const SUBRESOURCES: &str = "<html><head><title>Inside</title></head><body><main><p id=\"r\">asking</p></main>\
    <img src=\"SECRET/img\"><script src=\"SECRET/script.js\"></script><link rel=\"stylesheet\" href=\"SECRET/style.css\">\
    <script>fetch('SECRET/api').then(function () { document.getElementById('r').textContent = 'reached'; }, function () { document.getElementById('r').textContent = 'blocked'; });</script>\
    <iframe src=\"SECRET/frame\"></iframe></body></html>";

async fn secret() -> Fake {
    Fake::start(&[("/img", Reply::ok("image/png", b"x")), ("/script.js", Reply::ok("text/javascript", b"1")), ("/style.css", Reply::ok("text/css", b"a{}")), ("/api", Reply::ok("text/plain", b"s")), ("/frame", Reply::ok("text/html", b"<p>f</p>")), ("/secret", Reply::ok("text/html", b"<p>secret</p>"))]).await
}

#[tokio::test]
async fn a_public_page_that_redirects_to_a_local_address_is_refused_inside_the_browser() {
    let secret = secret().await;
    let site = Fake::start(&[
        ("/to-secret", Reply::redirect(302, &secret.url("/secret"))),
        ("/to-loopback", Reply::redirect(302, "http://127.0.0.1:1/")),
        ("/to-metadata", Reply::redirect(302, "http://169.254.169.254/latest/meta-data/")),
        ("/two-hops", Reply::redirect(302, "/to-metadata")),
        ("/meta-refresh", Reply::ok("text/html", format!("<html><head><meta http-equiv=\"refresh\" content=\"0; url={}\"></head><body>moving</body></html>", secret.url("/secret")).as_bytes())),
    ])
    .await;
    let data = tempfile::tempdir().unwrap();
    let Some(reader) = reader(&site, data.path(), IDLE) else { return };
    for (path, name) in [("/to-secret", "127.0.0.1"), ("/to-loopback", "127.0.0.1"), ("/to-metadata", "169.254.169.254"), ("/two-hops", "169.254.169.254")] {
        let Err(ReaderError::Refused { refusal, redirected }) = reader.render(&site.url(path)).await else { panic!("{path} must be refused") };
        assert!(redirected, "{path}: a redirect");
        let said = ReaderError::Refused { refusal, redirected }.to_string();
        assert!(said.starts_with("redirect to a blocked address stopped: ") && said.contains(name), "{path}: {said}");
    }
    let meta = reader.render(&site.url("/meta-refresh")).await;
    assert!(!matches!(&meta, Ok(page) if page.html.contains("secret")), "a refresh to a local address reads nothing of it: {meta:?}");
    assert!(secret.requests().is_empty(), "the local server never saw a request: {:?}", secret.requests());
    // A page of the owner's is not refused for being on loopback elsewhere: the first hop is the egress's decision too.
    assert!(matches!(reader.render(&secret.url("/secret")).await, Err(ReaderError::Refused { redirected: false, .. })), "the first address is refused as well");
    assert!(secret.requests().is_empty());
    reader.shutdown().await;
}

#[tokio::test]
async fn subresources_frames_and_scripts_cannot_reach_a_local_address() {
    let secret = secret().await;
    let page = SUBRESOURCES.replace("SECRET", &format!("http://{}", secret.addr));
    let site = Fake::start(&[("/page", Reply::ok("text/html; charset=utf-8", page.as_bytes()))]).await;
    let data = tempfile::tempdir().unwrap();
    let Some(reader) = reader(&site, data.path(), IDLE) else { return };
    let read = reader.render(&site.url("/page")).await.unwrap();
    assert!(read.html.contains("<p id=\"r\">blocked</p>"), "the page's own fetch failed: {}", read.html);
    assert!(secret.requests().is_empty(), "no image, script, style, frame or fetch got through: {:?}", secret.requests());
    // The same page without the guard reaches the server: the test would see it.
    let other_data = tempfile::tempdir().unwrap();
    let open = Reader::new(other_data.path(), Arc::new(crate::browser::find_chromium), IDLE, Arc::new(crate::web::guard::Unchecked));
    open.render(&site.url("/page")).await.unwrap();
    assert!(!secret.requests().is_empty(), "the control: without the guard the requests are made");
    open.shutdown().await;
    reader.shutdown().await;
}

#[test]
fn the_profile_is_not_a_browsers_own() {
    let data = Path::new("/home/owner/.local/share/kagent");
    let profile = browser::profile_dir(data);
    assert!(profile.starts_with(data) && profile.ends_with("reader-profile"));
    for other in ["browser-profile", "app-profile", "BraveSoftware", "google-chrome", "chromium"] {
        assert!(!profile.to_string_lossy().contains(other), "{other}");
    }
}
