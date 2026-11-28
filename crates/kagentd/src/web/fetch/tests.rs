//! `web_fetch` against the loopback fake (`fake.rs`): the pages it reads, the redirects
//! it follows and refuses, the cap, the cache.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kagent_core::tool::{SideEffect, Tool, ToolContext, ToolOutput};
use serde_json::{json, Value};

use super::*;
use crate::web::fake::{Fake, MemoryKeys, Reply, ALIAS};
use crate::web::{Engines, Web};

const PAGE: &[u8] = b"<html><head><title>A page</title></head><body><nav>Menu</nav><main><h1>Hi</h1><p>Text with a <a href='next.html'>relative link</a> and <a href='https://other.example/'>an absolute one</a>.</p></main></body></html>";

async fn fake() -> Fake {
    let big = vec![b'x'; MAX_BODY + 1];
    let links: Vec<u8> = (0..45).flat_map(|n| format!("<a href='/l{n}'>link {n}</a><br>").into_bytes()).collect();
    let long = "word ".repeat(3000).into_bytes();
    Fake::start(&[
        ("/page", Reply::ok("text/html; charset=utf-8", PAGE)),
        ("/plain", Reply::ok("text/plain; charset=us-ascii", b"  just text\n\n")),
        ("/json", Reply::ok("application/json; charset=utf-8", br#"{"a": 1}"#)),
        ("/xml", Reply::ok("application/xml; charset=utf-8", b"<a>1</a>")),
        ("/yaml", Reply::ok("application/yaml", b"a: 1\n")),
        ("/bin", Reply::with(200, vec![("content-type", "image/png".into())], &[0u8; 14_600])),
        ("/chunked-bin", Reply::chunked("application/octet-stream", vec![0u8; 5000])),
        ("/empty", Reply::ok("text/html", b"")),
        ("/untyped", Reply::with(200, vec![("content-type", String::new())], b"<html><body><p>No type at all</p></body></html>")),
        ("/missing", Reply::with(404, vec![("content-type", "text/html".into())], b"<h1>gone</h1>")),
        ("/moved", Reply::redirect(301, "/page")),
        ("/to-ip", Reply::redirect(302, "http://127.0.0.1:9/json/list")),
        ("/to-name", Reply::redirect(307, "http://localhost/")),
        ("/to-scheme", Reply::redirect(302, "ftp://example.com/x")),
        ("/to-user", Reply::redirect(302, "http://u:p@8.8.8.8/")),
        ("/loop", Reply::redirect(302, "/loop")),
        ("/nowhere", Reply::with(302, vec![], b"")),
        ("/big", Reply::ok("text/plain", &big)),
        ("/big-chunked", Reply::chunked("text/plain", vec![b'x'; MAX_BODY + 100])),
        ("/slow", Reply::NEVER),
        ("/links", Reply::ok("text/html", &links)),
        ("/long", Reply::ok("text/plain", &long)),
    ])
    .await
}

async fn web(fake: &Fake) -> Arc<Web> {
    Arc::new(Web::with(Arc::new(OnceLock::new()), Arc::new(MemoryKeys::default()), fake.egress(), Engines::default()))
}

/// `web_fetch` as the agent calls it.
async fn call(tool: &Arc<dyn Tool>, input: Value) -> ToolOutput {
    tool.call(input, ToolContext::standalone(Duration::from_secs(20), 10_000)).await.unwrap()
}

/// A refusal of the first address, and nothing was sent for it.
#[tokio::test]
async fn a_public_page_comes_back_as_text_and_a_file_that_is_not_text_as_an_error() {
    let fake = fake().await;
    let tool = web(&fake).await.fetch_tool();
    let base = fake.url("");
    assert_eq!(tool.contract().name, "web_fetch");
    assert_eq!((tool.contract().read_only, tool.contract().side_effect, tool.contract().default_timeout_ms, tool.contract().max_output_bytes), (true, SideEffect::Network, 45_000, 24_000));
    assert_eq!(tool.assess(&json!({"url": "https://example.org/x"})).summary, "web_fetch https://example.org/x");

    let out = call(&tool, json!({"url": fake.url("/page")})).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        format!("{UNTRUSTED}\nURL: {base}/page\nTitle: A page\n\n## Text\nHi\nText with a relative link and an absolute one.\n\n## Links\n1. relative link - {base}/next.html\n2. an absolute one - https://other.example/\n"),
        "the menu outside <main> is not the page, the relative link is resolved against the address"
    );
    for (path, text) in [("/plain", "just text"), ("/json", "{\"a\": 1}"), ("/xml", "<a>1</a>"), ("/yaml", "a: 1")] {
        let out = call(&tool, json!({"url": fake.url(path)})).await;
        assert!(!out.is_error, "{path}: {}", out.content);
        assert_eq!(out.content, format!("{UNTRUSTED}\nURL: {base}{path}\n\n## Text\n{text}\n"), "{path}: the raw text");
    }
    let out = call(&tool, json!({"url": fake.url("/untyped")})).await;
    assert_eq!(out.content, format!("{UNTRUSTED}\nURL: {base}/untyped\n\n## Text\nNo type at all\n"), "a server that names no type served a page");
    let out = call(&tool, json!({"url": fake.url("/bin")})).await;
    assert!(out.is_error && out.content == "not a text page (image/png, 14.3 kB)", "{}", out.content);
    let out = call(&tool, json!({"url": fake.url("/chunked-bin")})).await;
    assert!(out.is_error && out.content == "not a text page (application/octet-stream, size unknown)", "{}", out.content);
    assert!(fake.seen("/bin").len() + fake.seen("/chunked-bin").len() == 2);

    let out = call(&tool, json!({"url": fake.url("/empty")})).await;
    assert!(out.is_error && out.content == "the page is empty");
    let out = call(&tool, json!({"url": fake.url("/missing")})).await;
    assert!(out.is_error && out.content == "the page answered 404 Not Found");
    let out = call(&tool, json!({"url": fake.url("/nowhere")})).await;
    assert!(out.is_error && out.content == "the page answered 302 Found", "a redirect that names no target");
    let out = call(&tool, json!({})).await;
    assert!(out.is_error && out.content == "url required: the http or https address of a public web page");
    let out = call(&tool, json!({"url": "   "})).await;
    assert!(out.is_error && out.content.starts_with("url required"));
}

/// Scenario S30 item 1 (the chain) and item 5: every target is checked again, and a
/// blocked one stops the fetch; the DevTools port is refused before anything is sent.
#[tokio::test]
async fn every_redirect_is_checked_again_and_a_blocked_target_stops() {
    let fake = fake().await;
    let base = fake.url("");

    // Five redirects are followed (a chain of five hops ends well), the sixth is one too many.
    let chain = Fake::start(&[("/r0", Reply::redirect(302, "/r1")), ("/r1", Reply::redirect(302, "/r2")), ("/r2", Reply::redirect(302, "/r3")), ("/r3", Reply::redirect(303, "/r4")), ("/r4", Reply::redirect(308, "/r5")), ("/r5", Reply::ok("text/plain", b"the end"))]).await;
    let chain_tool = web(&chain).await.fetch_tool();
    let out = call(&chain_tool, json!({"url": chain.url("/r0")})).await;
    assert_eq!(out.content, format!("{UNTRUSTED}\nURL: {}\n\n## Text\nthe end\n", chain.url("/r5")), "the address of the answer is the last one");
    call(&chain_tool, json!({"url": chain.url("/r5")})).await;
    assert_eq!(chain.seen("/r0").len(), 1, "asked for by its first address once, then served from the cache");

    let tool = web(&fake).await.fetch_tool();
    let out = call(&tool, json!({"url": fake.url("/to-ip")})).await;
    assert!(out.is_error);
    assert_eq!(out.content, "redirect to a blocked address stopped: \"127.0.0.1\" is a local or private address; the web tools reach public hosts only");
    let out = call(&tool, json!({"url": fake.url("/to-name")})).await;
    assert_eq!(out.content, "redirect to a blocked address stopped: \"localhost\" is a name of a local network; the web tools reach public hosts only");
    let out = call(&tool, json!({"url": fake.url("/to-scheme")})).await;
    assert_eq!(out.content, "redirect to a blocked address stopped: only http and https URLs are fetched, not \"ftp://example.com/x\"");
    let out = call(&tool, json!({"url": fake.url("/to-user")})).await;
    assert_eq!(out.content, "redirect to a blocked address stopped: a \"user:password@\" part is refused; the web tools reach public hosts only");
    let out = call(&tool, json!({"url": fake.url("/loop")})).await;
    assert_eq!(out.content, format!("stopped after 5 redirects (the last target: {base}/loop)"));
    assert_eq!(fake.seen("/loop").len(), 6, "the request, and five redirects");

    // The address the model names: refused before any request.
    for (url, refusal) in [
        ("http://127.0.0.1:9333/json/list", "\"127.0.0.1\" is a local or private address; the web tools reach public hosts only"),
        ("http://localhost:7878/api/state", "\"localhost\" is a name of a local network; the web tools reach public hosts only"),
        ("http://[::1]/", "\"::1\" is a local or private address; the web tools reach public hosts only"),
        ("file:///etc/passwd", "only http and https URLs are fetched, not \"file:///etc/passwd\""),
        ("http://user:pw@example.com/", "a \"user:password@\" part is refused; the web tools reach public hosts only"),
    ] {
        let out = call(&tool, json!({"url": url})).await;
        assert!(out.is_error && out.content == format!("refused: {refusal}"), "{url}: {}", out.content);
    }
    let asked = fake.admitted();
    assert!(asked.contains(&"http://127.0.0.1:9333/json/list".to_string()), "the guard was asked first");
    assert!(!fake.requests().iter().any(|seen| seen.target.contains("json/list")), "and nothing was sent to it");
}

/// Scenario S30 item 5, through the tool: the DevTools port, refused; nothing listens for it here.
#[tokio::test]
async fn the_address_the_egress_resolved_is_the_one_connected_to() {
    let fake = fake().await;
    let tool = web(&fake).await.fetch_tool();
    // `pinned.test` is no name of any DNS: the egress gives the fake's address, and only that is connected to.
    let url = format!("http://{ALIAS}:{}/plain", fake.addr.port());
    let out = call(&tool, json!({"url": url})).await;
    assert_eq!(out.content, format!("{UNTRUSTED}\nURL: {url}\n\n## Text\njust text\n"), "{}", out.content);
    assert_eq!(fake.seen("/plain")[0].header("host"), format!("{ALIAS}:{}", fake.addr.port()), "the page is asked for under its own name");
}

#[tokio::test]
async fn the_body_is_stopped_at_the_cap_and_a_page_that_never_answers_is_given_up() {
    let fake = fake().await;
    let web = web(&fake).await;
    let tool = web.fetch_tool();
    for path in ["/big", "/big-chunked"] {
        let out = call(&tool, json!({"url": fake.url(path)})).await;
        assert!(out.is_error && out.content == "the page is larger than 2 MB; the download was stopped at the cap", "{path}: {}", out.content);
    }
    assert!(matches!(fetch_within(&web.transport, &fake.url("/slow"), Duration::from_millis(300)).await, Err(Fail::Timeout)));
    assert_eq!(Fail::Timeout.to_string(), "the page did not answer within 15 s");
}

#[tokio::test]
async fn the_request_carries_the_user_agent_the_accept_header_and_nothing_else() {
    let fake = fake().await;
    let tool = web(&fake).await.fetch_tool();
    call(&tool, json!({"url": fake.url("/page")})).await;
    let seen = &fake.seen("/page")[0];
    assert_eq!(seen.method, "GET");
    assert_eq!(seen.header("user-agent"), format!("KAgent/{}", env!("CARGO_PKG_VERSION")));
    assert_eq!(seen.header("accept"), "text/html,application/xhtml+xml,application/json,application/xml,text/*;q=0.9");
    assert!(seen.header("cookie").is_empty() && seen.header("authorization").is_empty() && seen.header("proxy-authorization").is_empty(), "no cookie, no credential");
    assert!(seen.body.is_empty(), "a GET carries nothing");
}

#[tokio::test]
async fn max_chars_cuts_the_text_and_at_most_thirty_links_are_listed() {
    let fake = fake().await;
    let tool = web(&fake).await.fetch_tool();
    let out = call(&tool, json!({"url": fake.url("/long"), "max_chars": 2000})).await;
    assert!(out.content.ends_with(&format!("## Text\n{}\n\n[... 12999 more characters not shown; ask again with a larger max_chars]\n", "word ".repeat(2000 / 5))), "{}", html::cut(&out.content, 200));
    let out = call(&tool, json!({"url": fake.url("/long"), "max_chars": 10})).await;
    assert!(out.content.contains("[... 13999 more characters not shown"), "a number under 1000 is read as 1000");
    let out = call(&tool, json!({"url": fake.url("/long"), "max_chars": 999999})).await;
    assert!(out.content.contains(&format!("## Text\n{}word\n", "word ".repeat(2999))) && !out.content.contains("more characters not shown"), "over 20000 is read as 20000, and the page has 14999: len {} tail {:?} has-note {}", out.content.len(), &out.content[out.content.len().saturating_sub(160)..], out.content.contains("more characters not shown"));
    let out = call(&tool, json!({"url": fake.url("/long")})).await;
    assert!(out.content.contains("[... 8999 more characters not shown"), "6000 unless asked");

    let out = call(&tool, json!({"url": fake.url("/links")})).await;
    assert!(out.content.contains("\n## Links\n1. link 0 - ") && out.content.contains("\n30. link 29 - ") && !out.content.contains("31. "), "{}", &out.content[out.content.len().saturating_sub(200)..]);
    assert!(out.content.contains("link 44"), "the text still has every line");
}

/// Scenario: the cache of web.md section 2 (15 minutes, 50 entries).
#[test]
fn the_cache_keeps_fifty_pages_for_fifteen_minutes_and_the_used_one_first() {
    let cache = Cache::default();
    let t0 = Instant::now();
    assert_eq!(cache.get("none", t0), None);
    cache.put("k".into(), "one".into(), t0);
    assert_eq!(cache.get("k", t0 + CACHE_TTL), Some("one".into()), "fresh until the last moment");
    assert_eq!(cache.get("k", t0 + CACHE_TTL + Duration::from_secs(1)), None, "then gone");
    cache.put("k".into(), "two".into(), t0);
    assert_eq!(cache.get("k", t0), Some("two".into()), "the newest under a key");
    for n in 0..60 {
        cache.put(format!("k{n}"), format!("v{n}"), t0);
    }
    assert_eq!(cache.get("k9", t0), None, "the oldest ten went over the fifty");
    assert_eq!(cache.get("k10", t0), Some("v10".into()), "used here, so the first to stay");
    cache.put("new".into(), "new".into(), t0);
    assert_eq!((cache.get("k11", t0), cache.get("k12", t0), cache.get("k10", t0), cache.get("new", t0)), (None, Some("v12".into()), Some("v10".into()), Some("new".into())));
    assert_eq!(cache.get("k10", t0 + CACHE_TTL), Some("v10".into()));
    assert_eq!(cache.get("k10", t0 + CACHE_TTL + Duration::from_secs(1)), None, "fifteen minutes after it was fetched, not after it was last used");
}

#[tokio::test]
async fn a_page_is_fetched_once_and_served_from_the_cache() {
    let fake = fake().await;
    let tool = web(&fake).await.fetch_tool();
    let first = call(&tool, json!({"url": fake.url("/page")})).await;
    let again = call(&tool, json!({"url": fake.url("/page")})).await;
    assert_eq!(first.content, again.content);
    assert_eq!(fake.seen("/page").len(), 1, "the second answer came from the cache");

    // Asked for through a redirect: the first address keeps the answer too.
    let moved = call(&tool, json!({"url": fake.url("/moved")})).await;
    assert_eq!(moved.content, first.content);
    call(&tool, json!({"url": fake.url("/moved")})).await;
    assert_eq!((fake.seen("/moved").len(), fake.seen("/page").len()), (1, 2), "the redirect led to a second request for the page; neither address is asked again");

    // Another `max_chars` is another entry, and nothing that failed is kept.
    call(&tool, json!({"url": fake.url("/page"), "max_chars": 1000})).await;
    assert_eq!(fake.seen("/page").len(), 3);
    call(&tool, json!({"url": fake.url("/missing")})).await;
    call(&tool, json!({"url": fake.url("/missing")})).await;
    assert_eq!(fake.seen("/missing").len(), 2);
}

mod reader_tests;
