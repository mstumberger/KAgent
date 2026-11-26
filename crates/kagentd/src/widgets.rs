//! Inline widgets (docs/specs/widgets.md): a small HTML panel the agent puts inside its
//! answer. The `widget_show` tool saves the fragment under `<data>/widgets/<session>/<n>.html`
//! and answers the marker line the UI turns into a sandboxed iframe; `GET
//! /api/widgets/:session/:id` serves one fragment back. A fragment is model output: it is
//! stored and served here, never parsed and never run with any privilege.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path as WebPath, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use kagent_core::session::SessionStore;
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

/// At most this many bytes in a fragment (widgets.md section 5).
pub const MAX_BYTES: usize = 24 * 1024;
/// Fragments kept per session: the 201st takes the number of the first.
pub const MAX_PER_SESSION: usize = 200;
/// At most this many characters of the title shown above the panel.
pub const MAX_TITLE_CHARS: usize = 80;
/// The session of an MCP caller has no chat window (tools.md section 22).
pub const NO_SESSION: &str = "widgets need a chat session";
/// Notes in a session's folder the number of its last fragment.
const COUNT: &str = ".count";

/// A plain session id: 1 to 64 ASCII letters, digits, `-` or `_`. Only such an id ever
/// becomes a folder name here, in the tool and on the route.
pub fn plain_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A widget number, as the route's `:id` and the marker's `<n>`: 1 to 200, no leading zero.
fn number(id: &str) -> Option<usize> {
    if id.is_empty() || id.len() > 3 || id.starts_with('0') || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    id.parse::<usize>().ok().filter(|n| (1..=MAX_PER_SESSION).contains(n))
}

/// The line the tool answers and the UI renders (widgets.md sections 1 and 2).
pub fn marker(session: &str, n: usize) -> String {
    format!("::widget{{id=\"{session}/{n}\"}}")
}

/// The title as it is kept and shown: one line of at most 80 characters.
fn clean_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_TITLE_CHARS).collect()
}

/// The fragment out of the `html` argument: one text, or its lines. `validate_schema` does
/// not look into `anyOf`, so the form is checked here, as `skill_save.body` and
/// `plan_exit.plan` do (the S20 trap: the local server drops a call whose long text sits
/// in a plain `string` it cannot parse).
fn html_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(lines)) if !lines.is_empty() && lines.iter().all(|l| l.is_string()) => {
            Some(lines.iter().filter_map(|l| l.as_str()).collect::<Vec<_>>().join("\n"))
        }
        _ => None,
    }
}

/// The fragments of every session, under `<data>/widgets`.
#[derive(Clone, Debug)]
pub struct Widgets {
    root: PathBuf,
}

impl Widgets {
    /// The store under `<data>/widgets`.
    pub fn new(data: impl Into<PathBuf>) -> Self {
        Self { root: data.into().join("widgets") }
    }

    /// The folder of a session's fragments; `None` for an id that is not plain.
    fn dir_of(&self, session: &str) -> Option<PathBuf> {
        plain_id(session).then(|| self.root.join(session))
    }

    /// The number of the next fragment: one past the last one, which `COUNT` notes, from
    /// 200 back to 1 (so the oldest is overwritten). A folder without a count continues
    /// past its highest fragment.
    async fn next_number(&self, dir: &Path) -> anyhow::Result<usize> {
        let last = match tokio::fs::read_to_string(dir.join(COUNT)).await {
            Ok(text) => text.trim().parse::<usize>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.highest(dir).await?,
            Err(e) => return Err(e.into()),
        };
        Ok(last % MAX_PER_SESSION + 1)
    }

    /// The highest fragment number of the folder; 0 for none.
    async fn highest(&self, dir: &Path) -> anyhow::Result<usize> {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e.into()),
        };
        let mut highest = 0usize;
        while let Some(entry) = entries.next_entry().await? {
            let stem = entry.file_name();
            let stem = stem.to_string_lossy();
            if let Some(n) = stem.strip_suffix(".html").and_then(|s| s.parse::<usize>().ok()) {
                highest = highest.max(n);
            }
        }
        Ok(highest)
    }

    /// Saves a fragment of the session under its next number, with its title, and answers
    /// that number. The id is checked again here, so nothing reaches the disk unchecked.
    pub async fn save(&self, session: &str, title: &str, html: &str) -> anyhow::Result<usize> {
        let dir = self.dir_of(session).ok_or_else(|| anyhow::anyhow!(NO_SESSION))?;
        let n = self.next_number(&dir).await?;
        tokio::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).await?;
        // The count goes first: a fragment that fails to write leaves a gap, never a
        // number used twice.
        write_private(&dir.join(COUNT), n.to_string().as_bytes()).await?;
        write_private(&dir.join(format!("{n}.title")), clean_title(title).as_bytes()).await?;
        write_private(&dir.join(format!("{n}.html")), html.as_bytes()).await?;
        Ok(n)
    }

    /// The fragments of a session go with it (widgets.md section 5). An id that is not
    /// plain names no folder here, so nothing outside the store is touched.
    pub async fn remove_session(&self, session: &str) {
        if let Some(dir) = self.dir_of(session) {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }

    pub async fn remove_all(&self, sessions: &[String]) {
        for session in sessions {
            self.remove_session(session).await;
        }
    }

    /// A session and the subagents hanging off it: they are deleted together (agents.md
    /// section 2), so their fragments go together.
    pub async fn family(&self, sessions: &SessionStore, id: &str) -> Vec<String> {
        let mut ids = vec![id.to_string()];
        if let Ok(metas) = sessions.list().await {
            ids.extend(metas.iter().filter(|m| m.parent.as_deref() == Some(id)).map(|m| m.id.clone()));
        }
        ids
    }

    /// The answer of `GET /api/widgets/:session/:id` (api.md section 6): the fragment as
    /// `text/html`, its title in `X-KAgent-Title` (percent-encoded). 400 for a session id
    /// that is not plain or a number out of range, 404 when there is no such fragment.
    pub async fn fragment(&self, session: &str, id: &str) -> ApiResult<Response> {
        let dir = self.dir_of(session).ok_or_else(|| bad_request("session id must be a plain id"))?;
        let n = number(id).ok_or_else(|| bad_request(format!("widget id must be a number from 1 to {MAX_PER_SESSION}")))?;
        let path = dir.join(format!("{n}.html"));
        // A regular file only: a link put in its place is not followed.
        match tokio::fs::symlink_metadata(&path).await {
            Ok(meta) if meta.is_file() => {}
            _ => return Err(not_found("no such widget")),
        }
        let html = tokio::fs::read(&path).await.map_err(internal)?;
        let mut res = (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")],
            html,
        )
            .into_response();
        let title = tokio::fs::read_to_string(dir.join(format!("{n}.title"))).await.unwrap_or_default();
        if !title.is_empty() {
            if let Ok(value) = percent_encode(&title).parse() {
                res.headers_mut().insert("x-kagent-title", value);
            }
        }
        Ok(res)
    }
}

/// Writes `bytes` to `path` as 0600, through a temporary file and a rename: a reader never
/// sees half a fragment, and a file that existed with other rights is replaced whole.
async fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).await?;
    let written = async { file.write_all(bytes).await?; file.flush().await }.await;
    drop(file);
    if let Err(e) = written {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Every byte but the unreserved ASCII characters as `%XX`: the title is a valid header
/// value whatever its letters (the UI reads it with `decodeURIComponent`).
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b => format!("%{b:02X}"),
        })
        .collect()
}

fn internal(e: std::io::Error) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e}"))
}

/// `GET /api/widgets/{session}/{id}`: behind the bearer token like every `/api` route.
pub async fn get_widget(State(st): State<Arc<AppState>>, WebPath((session, id)): WebPath<(String, String)>) -> Response {
    match st.widgets.fragment(&session, &id).await {
        Ok(res) => res,
        Err(e) => e.into_response(),
    }
}

const DESCRIPTION: &str = "Show a small interactive HTML panel (a gauge, bars, a table, a sparkline) inside your answer, for numbers you already have. It saves the fragment and returns one marker line: put that line alone, on its own line, where the panel belongs, and do not repeat the panel's data in the text. Rules: static HTML with its CSS and JS inline; colours only through the theme variables --k-night, --k-glass, --k-ink, --k-dim, --k-ice, --k-ok, --k-bad, --k-signal; no external resources (no web font, script, image or link); one idea per widget; call it once per answer, with the final data (read the data first, build the panel after). Readable at 300 px wide, under 120 lines, nothing that loops forever.";

/// `widget_show` (tools.md section 22): MUT, risk low, no approval.
pub struct WidgetShow {
    contract: ToolContract,
    store: Arc<Widgets>,
}

impl WidgetShow {
    pub fn new(store: Arc<Widgets>) -> Self {
        let mut contract = ToolContract::read_only(
            "widget_show",
            DESCRIPTION,
            json!({"type":"object","required":["title","html"],"properties":{
                "title":{"type":"string","description":"a few words naming the panel; shown above it; at most 80 characters"},
                "html":{"description":"the panel's content: HTML for the body only (no html, head or body tags) with its inline CSS and JS; under 24 KB. One text, or one line per item","anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}
            }}),
        )
        .mutating(SideEffect::Workspace, Risk::Low);
        contract.needs_approval = false; // only what the user's own chat shows
        Self { contract, store }
    }
}

#[async_trait]
impl Tool for WidgetShow {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let title = input.get("title").and_then(Value::as_str).unwrap_or("");
        CallAssessment::from_contract(&self.contract, format!("widget {}", clean_title(title)), "widget_show".into())
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let title = match input.get("title").and_then(Value::as_str) {
            Some(t) => t.to_string(),
            None => return Ok(ToolOutput::error("title is required: a few words naming the panel")),
        };
        let Some(html) = html_of(input.get("html")) else {
            return Ok(ToolOutput::error("html is required: the panel as an HTML fragment, one text or one line per item"));
        };
        if ctx.session_id == "mcp" || !plain_id(&ctx.session_id) {
            return Ok(ToolOutput::error(NO_SESSION));
        }
        if html.trim().is_empty() {
            return Ok(ToolOutput::error("the fragment is empty: write the panel as HTML and call widget_show again"));
        }
        if html.len() > MAX_BYTES {
            return Ok(ToolOutput::error(format!("the widget has {} bytes; at most {MAX_BYTES} are kept: shorten it", html.len())));
        }
        let n = match self.store.save(&ctx.session_id, &title, &html).await {
            Ok(n) => n,
            Err(e) => return Ok(ToolOutput::error(format!("the widget could not be saved: {e:#}"))),
        };
        Ok(ToolOutput::ok(format!(
            "{}\nPut this line on its own line in your answer, exactly as it is, where the panel belongs.",
            marker(&ctx.session_id, n)
        )))
    }
}

#[cfg(test)]
#[path = "widgets/tests.rs"]
mod tests;
