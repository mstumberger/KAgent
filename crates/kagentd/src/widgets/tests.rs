//! Tests of widgets.md section 7: the store, the tool and the route, without a server.

use super::*;
use axum::body::to_bytes;
use axum::http::header::HeaderMap;
use kagent_core::protocol::Message;
use kagent_core::session::SessionOrigin;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const SESSION: &str = "0f1e2d3c-aaaa-4bbb-8ccc-1234567890ab";
const PANEL: &str = "<div class=\"gauge\"><span>85</span></div>";

/// The store over a temporary data directory, and the tool over it.
fn setup() -> (tempfile::TempDir, Arc<Widgets>, WidgetShow) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Widgets::new(dir.path()));
    let tool = WidgetShow::new(store.clone());
    (dir, store, tool)
}

/// A context of the session `session`.
fn ctx(session: &str) -> ToolContext {
    let mut ctx = ToolContext::standalone(Duration::from_secs(5), 10_000);
    ctx.session_id = session.into();
    ctx
}

async fn call(tool: &WidgetShow, session: &str, input: Value) -> kagent_core::tool::ToolOutput {
    tool.call(input, ctx(session)).await.unwrap()
}

/// The call had to succeed; its answer says why it did not.
async fn saved(tool: &WidgetShow, session: &str, input: Value) -> kagent_core::tool::ToolOutput {
    let out = call(tool, session, input).await;
    assert!(!out.is_error, "{}", out.content);
    out
}

/// The status of an answer, refused or not.
fn status_of(res: ApiResult<Response>) -> StatusCode {
    match res {
        Ok(r) => r.status(),
        Err(e) => e.0,
    }
}

/// The answer of a call that had to answer (`HttpError` is not `Debug`).
fn answered(res: ApiResult<Response>) -> Response {
    res.ok().unwrap_or_else(|| panic!("must answer"))
}

/// Status, headers and body of an answer.
async fn read(res: Response) -> (StatusCode, HeaderMap, String) {
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Scenario S33 item 1: a fragment at the cap is kept, one byte more is refused, and the refusal writes nothing.
#[tokio::test]
async fn the_size_cap() {
    let (dir, _, tool) = setup();
    let full = format!("<p>{}</p>", "a".repeat(MAX_BYTES - 7));
    assert_eq!(full.len(), MAX_BYTES, "the sample fills the cap");
    saved(&tool, SESSION, json!({ "title": "Disk", "html": full })).await;
    let kept = dir.path().join("widgets").join(SESSION).join("1.html");
    assert_eq!(std::fs::metadata(&kept).unwrap().len() as usize, MAX_BYTES);

    let big = format!("<p>{}</p>", "b".repeat(MAX_BYTES - 6));
    let out = call(&tool, SESSION, json!({ "title": "Disk", "html": big })).await;
    assert!(out.is_error, "{}", out.content);
    assert_eq!(out.content, format!("the widget has {} bytes; at most {MAX_BYTES} are kept: shorten it", big.len()));
    assert!(!dir.path().join("widgets").join(SESSION).join("2.html").exists(), "a refused call writes nothing");

    let empty = call(&tool, SESSION, json!({ "title": "Disk", "html": "  \n" })).await;
    assert!(empty.is_error && empty.content.starts_with("the fragment is empty"), "{}", empty.content);
}

/// Scenario S33 item 1: a name that is not a plain id never becomes a path, in the tool and on the route.
#[tokio::test]
async fn names_are_validated_before_they_reach_the_disk() {
    let (dir, store, tool) = setup();
    let too_long = "a".repeat(65);
    for id in [SESSION, "mcp", "a", "A-b_9"] {
        assert!(plain_id(id), "{id}");
    }
    for id in ["", "..", "../evil", "a/b", "a.b", "s x", "š", too_long.as_str()] {
        assert!(!plain_id(id), "{id:?}");
    }
    // The tool: the MCP session, a standalone call and a crafted id all answer the same.
    for id in ["mcp", "", "../evil", "a/b"] {
        let out = call(&tool, id, json!({ "title": "t", "html": PANEL })).await;
        assert!(out.is_error && out.content == NO_SESSION, "{id:?}: {}", out.content);
    }
    // The store itself refuses too, even when called around the tool.
    let err = store.save("../evil", "t", PANEL).await.err().expect("refused");
    assert_eq!(err.to_string(), NO_SESSION);
    assert!(!dir.path().join("widgets").exists(), "nothing was written anywhere");

    // The route: a session that is not a plain id and a number out of range are 400.
    for id in ["..", "../evil", "a/b", "a.b", "", "a b", "š", too_long.as_str()] {
        assert_eq!(status_of(store.fragment(id, "1").await), StatusCode::BAD_REQUEST, "session {id:?}");
    }
    for n in ["", "0", "01", "201", "999", "1000", "x", "1.html", "1 1", "-1", "١"] {
        assert_eq!(status_of(store.fragment(SESSION, n).await), StatusCode::BAD_REQUEST, "id {n:?}");
    }
}

/// Scenario S33 item 1: an unknown id is 404, before and after fragments exist; a link in place of a file is not followed.
#[tokio::test]
async fn an_unknown_id_is_404() {
    let (dir, store, tool) = setup();
    let refused = store.fragment(SESSION, "1").await.err().expect("refused");
    assert_eq!(refused.0, StatusCode::NOT_FOUND);
    let (_, _, body) = read(refused.into_response()).await;
    assert!(body.contains("no such widget"), "{body}");
    saved(&tool, SESSION, json!({ "title": "Disk", "html": PANEL })).await;
    assert_eq!(status_of(store.fragment(SESSION, "1").await), StatusCode::OK);
    assert_eq!(status_of(store.fragment(SESSION, "2").await), StatusCode::NOT_FOUND);
    assert_eq!(status_of(store.fragment(SESSION, "200").await), StatusCode::NOT_FOUND);
    assert_eq!(status_of(store.fragment("0f1e2d3c-aaaa-4bbb-8ccc-000000000000", "1").await), StatusCode::NOT_FOUND);

    let other = "0f1e2d3c-aaaa-4bbb-8ccc-000000000001";
    let link = dir.path().join("widgets").join(other);
    std::fs::create_dir(&link).unwrap();
    std::os::unix::fs::symlink(dir.path().join("widgets").join(SESSION).join("1.html"), link.join("1.html")).unwrap();
    assert_eq!(status_of(store.fragment(other, "1").await), StatusCode::NOT_FOUND, "a link is not a fragment");
}

/// The route answers the fragment as it was saved, as `text/html`, with the title beside it.
#[tokio::test]
async fn the_fragment_is_answered_as_html_with_its_title() {
    let (dir, store, tool) = setup();
    let html = "<style>.b{height:8px}</style>\n<div class=\"b\">č</div>\n";
    let lines = json!({ "title": "Zdravje: Šum in disk", "html": ["<style>.b{height:8px}</style>", "<div class=\"b\">č</div>", ""] });
    saved(&tool, SESSION, lines).await;
    let (status, headers, body) = read(answered(store.fragment(SESSION, "1").await)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(body, html, "byte for byte, nothing added");
    assert_eq!(headers["x-kagent-title"], "Zdravje%3A%20%C5%A0um%20in%20disk");

    // Without a title file there is no header.
    std::fs::remove_file(dir.path().join("widgets").join(SESSION).join("1.title")).unwrap();
    let (_, headers, _) = read(answered(store.fragment(SESSION, "1").await)).await;
    assert!(!headers.contains_key("x-kagent-title"));
}

/// At most 200 fragments per session: the 201st takes the number of the first.
#[tokio::test]
async fn the_numbers_wrap_at_200() {
    let (dir, _, tool) = setup();
    for i in 1..=MAX_PER_SESSION + 2 {
        let out = call(&tool, SESSION, json!({ "title": "t", "html": format!("<p>{i}</p>") })).await;
        assert!(!out.is_error, "{}", out.content);
        let want = if i <= MAX_PER_SESSION { i } else { i - MAX_PER_SESSION };
        assert!(out.content.starts_with(&marker(SESSION, want)), "call {i}: {}", out.content);
    }
    let session = dir.path().join("widgets").join(SESSION);
    let names: Vec<String> = std::fs::read_dir(&session).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    let kept: Vec<&String> = names.iter().filter(|n| n.ends_with(".html")).collect();
    assert_eq!(kept.len(), MAX_PER_SESSION, "no more than 200 fragments");
    assert!(kept.iter().all(|n| (1..=MAX_PER_SESSION).any(|i| n.as_str() == format!("{i}.html"))), "{kept:?}");
    assert_eq!(names.len(), MAX_PER_SESSION * 2 + 1, "a fragment and its title each, and the count: {names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "no temporary file is left");
    assert_eq!(std::fs::read_to_string(session.join(".count")).unwrap(), "2");
    assert_eq!(std::fs::read_to_string(session.join("1.html")).unwrap(), format!("<p>{}</p>", MAX_PER_SESSION + 1), "the oldest was overwritten");
    assert_eq!(std::fs::read_to_string(session.join("1.title")).unwrap(), "t");
    assert_eq!(std::fs::read_to_string(session.join("2.html")).unwrap(), format!("<p>{}</p>", MAX_PER_SESSION + 2));
    assert_eq!(std::fs::read_to_string(session.join("3.html")).unwrap(), "<p>3</p>", "the rest stayed");
}

/// Scenario S33 item 1: the files go with the session, and with the subagents deleted with it.
#[tokio::test]
async fn files_go_with_the_session() {
    let (dir, store, tool) = setup();
    let sessions = SessionStore::new(dir.path().join("sessions")).unwrap();
    let parent = sessions.create_with("parent", SessionOrigin::Chat, None).await.unwrap().id;
    let child = sessions.create_with("child", SessionOrigin::Subagent, Some(parent.clone())).await.unwrap().id;
    let other = sessions.create("other").await.unwrap().id;
    for (session, fragments) in [(&parent, 2), (&child, 1), (&other, 1)] {
        for _ in 0..fragments {
            saved(&tool, session, json!({ "title": "t", "html": PANEL })).await;
        }
    }
    let root = dir.path().join("widgets");
    assert_eq!(store.family(&sessions, &parent).await, vec![parent.clone(), child.clone()], "the session and the subagents hanging off it, as the agent deletes them");
    assert_eq!(store.family(&sessions, "no-such-session").await, vec!["no-such-session".to_string()]);
    store.remove_all(&store.family(&sessions, &parent).await).await;
    assert!(!root.join(&parent).exists() && !root.join(&child).exists(), "gone with the session");
    assert!(root.join(&other).join("1.html").is_file(), "another session keeps its fragments");

    // An id that is not plain names no folder: nothing outside the store is touched.
    let outside = dir.path().join("evil");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep"), b"x").unwrap();
    for id in ["../evil", "..", "", "a/../../evil"] {
        store.remove_session(id).await;
    }
    assert!(outside.join("keep").is_file());
}

/// Mode 0600 for every file, 0700 for the folders.
#[tokio::test]
async fn files_are_private() {
    let (dir, _, tool) = setup();
    saved(&tool, SESSION, json!({ "title": "t", "html": PANEL })).await;
    let root = dir.path().join("widgets");
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join(SESSION)), 0o700);
    assert_eq!(mode(&root.join(SESSION).join("1.html")), 0o600);
    assert_eq!(mode(&root.join(SESSION).join("1.title")), 0o600);
}

/// The S20 trap (tools.md section 22): `html` takes one text or an array of lines.
#[tokio::test]
async fn the_fragment_comes_as_one_text_or_as_lines() {
    let (_, _, tool) = setup();
    let schema = &tool.contract().input_schema;
    assert_eq!(schema["properties"]["html"]["anyOf"], json!([{"type": "string"}, {"type": "array", "items": {"type": "string"}}]), "either form");
    assert!(schema["properties"]["html"].get("type").is_none());
    assert_eq!(schema["required"], json!(["title", "html"]));

    let text = call(&tool, SESSION, json!({ "title": "t", "html": "<p>a</p>\n<p>b</p>" })).await;
    let lines = call(&tool, SESSION, json!({ "title": "t", "html": ["<p>a</p>", "<p>b</p>"] })).await;
    assert!(!text.is_error && !lines.is_error, "{} {}", text.content, lines.content);
    assert_eq!(marker(SESSION, 1), "::widget{id=\"0f1e2d3c-aaaa-4bbb-8ccc-1234567890ab/1\"}");
    assert!(lines.content.starts_with(&marker(SESSION, 2)) && lines.content.contains("own line"), "{}", lines.content);
    for bad in [json!({ "title": "t" }), json!({ "title": "t", "html": 7 }), json!({ "title": "t", "html": [] }), json!({ "title": "t", "html": ["<p>a</p>", 3] }), json!({ "title": "t", "html": { "a": 1 } })] {
        let out = call(&tool, SESSION, bad.clone()).await;
        assert!(out.is_error && out.content.starts_with("html is required"), "{bad}: {}", out.content);
    }
    let out = call(&tool, SESSION, json!({ "html": PANEL })).await;
    assert_eq!(out.content, "title is required: a few words naming the panel");
}

/// The title is one line of at most 80 characters, in the answer and on the card.
#[tokio::test]
async fn the_title_is_one_line_of_at_most_80_characters() {
    let (_, store, tool) = setup();
    let long = format!("  {}\n  second line  ", "x".repeat(90));
    saved(&tool, SESSION, json!({ "title": long, "html": PANEL })).await;
    let (_, headers, _) = read(answered(store.fragment(SESSION, "1").await)).await;
    assert_eq!(headers["x-kagent-title"], "x".repeat(80), "the first 80 characters, nothing of the second line");
    assert_eq!(tool.assess(&json!({ "title": "Disk use", "html": PANEL })).summary, "widget Disk use");
    assert_eq!(tool.assess(&json!({ "html": PANEL })).summary, "widget ");
    assert_eq!(clean_title("Zdravje:   Šum\nin disk\ttoday"), "Zdravje: Šum in disk today");
}

/// The marker line is stored with the answer and comes back unchanged.
#[tokio::test]
async fn the_marker_line_survives_the_store_round_trip() {
    let (dir, _, tool) = setup();
    let sessions = SessionStore::new(dir.path().join("sessions")).unwrap();
    let session = sessions.create("gauge").await.unwrap().id;
    let out = call(&tool, &session, json!({ "title": "Doctor score", "html": PANEL })).await;
    assert!(!out.is_error, "{}", out.content);
    let marker = marker(&session, 1);
    assert_eq!(out.content, format!("{marker}\nPut this line on its own line in your answer, exactly as it is, where the panel belongs."));
    let answer = format!("Your doctor score is 85.\n\n{marker}\n\nNothing else is wrong.");
    sessions.append(&session, &[Message::user("how is my machine?"), Message::assistant(answer.clone(), vec![])]).await.unwrap();
    let back = sessions.load_messages(&session).await.unwrap();
    assert_eq!(back[1].content, answer, "the marker line is text like any other");
    assert_eq!(back[1].content.lines().nth(2), Some(marker.as_str()), "on its own line");
}

/// widgets.md section 3: the bundled skill is there, short, and names the theme variables.
#[test]
fn the_bundled_skill_is_short_and_names_the_variables() {
    let raw = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/widget-design/SKILL.md")).unwrap();
    assert!(raw.starts_with("---\nname: widget-design\n"), "{}", &raw[..40.min(raw.len())]);
    assert!(raw.lines().count() <= 60, "{} lines", raw.lines().count());
    assert!(!raw.contains('\u{2014}'), "no em dashes");
    assert!(kagent_core::skills::author::RESERVED.contains(&"widget-design"), "a user skill cannot take its name");
    for said in ["--k-night", "--k-glass", "--k-ink", "--k-dim", "--k-ice", "--k-ok", "--k-bad", "--k-signal", "widget_show", "300 px", "120 lines", "No forms", "loops forever", "Layout first"] {
        assert!(raw.contains(said), "the skill says '{said}'");
    }
}

/// tools.md section 22: MUT, risk low, no approval; the five rules of widgets.md section 3 are in the description.
#[test]
fn the_contract_needs_no_approval() {
    let (_, _, tool) = setup();
    let c = tool.contract();
    assert_eq!(c.name, "widget_show");
    assert!(!c.read_only && !c.needs_approval && !c.always_ask && !c.privileged && !c.destructive);
    assert_eq!(c.risk, Risk::Low);
    assert_eq!(c.side_effect, SideEffect::Workspace);
    let a = tool.assess(&json!({ "title": "t", "html": PANEL }));
    assert!(!a.needs_approval && !a.read_only && a.rule_key == "widget_show");
    assert_eq!(percent_encode("Disk a-b_9.~ x%/š"), "Disk%20a-b_9.~%20x%25%2F%C5%A1");
    for said in ["CSS and JS inline", "--k-glass", "--k-signal", "no external resources", "one idea per widget", "once per answer", "final data"] {
        assert!(c.description.contains(said), "the description says '{said}'");
    }
}
