//! The diff routes (diff.md sections 3 and 4): `GET /api/changes/:id/diff` for a journal change of a file edit, and
//! `POST /api/diff` for a git repository. Read-only: no route here writes anything.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::journal::{Change, ChangeJournal};
use serde::Deserialize;
use serde_json::{json, Value};

use super::git::{self, GitError};
use super::lines::diff_texts;
use super::{Diff, Status, MAX_FILE_BYTES};
use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

fn conflict(msg: impl Into<String>) -> HttpError {
    HttpError(StatusCode::CONFLICT, "conflict", msg.into())
}

impl From<GitError> for HttpError {
    fn from(e: GitError) -> Self {
        match e {
            GitError::Invalid(m) => bad_request(m),
            GitError::NotARepo(m) => conflict(m),
            GitError::Failed(m) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", m),
        }
    }
}

const NO_COPY: &str = "this change has no stored copy of the file: only an edit_file or write_file change keeps one";

fn path_in(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// A stored text, or `None` when it is gone or not UTF-8 text.
fn stored(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The answer for a journal change (diff.md section 3). `read_now` reads the file as it is now.
pub fn for_change(change: &Change, read_now: impl Fn(&str) -> Option<String>) -> Result<Value, HttpError> {
    let op_action = change.op.get("action").and_then(Value::as_str).unwrap_or_default();
    let restores = change.undo.as_ref().and_then(|u| u.get("action")).and_then(Value::as_str) == Some("restore_file");
    if !(matches!(op_action, "write_file" | "edit_file") && restores) {
        return Err(conflict(NO_COPY));
    }
    let path = path_in(&change.op, "path").ok_or_else(|| conflict(NO_COPY))?;
    let undo = change.undo.as_ref().expect("checked");
    // A file that did not exist has no backup: its text before is empty.
    let (before, was_new) = match path_in(undo, "backup") {
        Some(b) => (stored(&b).ok_or_else(|| conflict(NO_COPY))?, false),
        None => (String::new(), true),
    };
    let (after, mode) = match path_in(&change.op, "afterBackup").and_then(|p| stored(&p)) {
        Some(text) => (text, "stored"),
        // From before this stage: the file as it is now, when the change still stands.
        None if change.undone_at.is_none() => (read_now(&path).ok_or_else(|| conflict(NO_COPY))?, "now"),
        None => return Err(conflict(NO_COPY)),
    };
    let diff = Diff::of(vec![diff_texts(&path, &before, &after, if was_new { Status::Added } else { Status::Modified })], false);
    // The texts go with the diff so the view can expand the unchanged lines (each at most 1 MB).
    let keep = |t: String| (t.len() <= MAX_FILE_BYTES).then_some(t);
    Ok(json!({"id": change.id, "path": path, "after": mode, "before": keep(before), "afterText": keep(after), "diff": diff}))
}

pub async fn change_diff(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let journal: Arc<ChangeJournal> = st.services.journal.clone();
    let change = journal.get(&id).map_err(|_| not_found(format!("no change {id}")))?;
    let body = tokio::task::spawn_blocking(move || for_change(&change, |p| std::fs::read_to_string(p).ok())).await.map_err(|e| HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()))??;
    Ok(Json(body).into_response())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffBody {
    pub project: String,
    pub from: String,
    #[serde(default)]
    pub to: Option<String>,
}

/// The canonical folder of `project` when it is one of `known` (the projects list, canonical paths), else 400.
pub fn known_project(project: &str, known: &[PathBuf]) -> Result<PathBuf, HttpError> {
    let not_known = || bad_request(format!("{project} is not a known project: only a folder of the projects list (GET /api/projects) can be diffed"));
    if !project.starts_with('/') {
        return Err(not_known());
    }
    let canonical = std::fs::canonicalize(project).map_err(|_| not_known())?;
    known.iter().any(|k| std::fs::canonicalize(k).is_ok_and(|k| k == canonical)).then_some(canonical).ok_or_else(not_known)
}

/// `POST /api/diff` over a project and refs.
pub async fn repo_diff(body: &Value, known: &[PathBuf]) -> Result<Value, HttpError> {
    let req: DiffBody = serde_json::from_value(body.clone()).map_err(|e| bad_request(format!("expected {{project, from, to?}}: {e}")))?;
    let project = known_project(&req.project, known)?;
    let to = req.to.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let diff = git::diff(&project, req.from.trim(), to).await?;
    Ok(json!({"project": req.project, "from": req.from.trim(), "to": to, "diff": diff}))
}

pub async fn post(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let roots = st.settings.read().unwrap().desktop_prefs().project_roots;
    let known = tokio::task::spawn_blocking(move || kagent_core::tools::xdg::projects(&roots)).await.unwrap_or_default();
    Ok(Json(repo_diff(&body, &known).await?).into_response())
}

