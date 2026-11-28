//! The routes of the background shell tasks (api.md section 8, work-windows.md section 3):
//! the tasks of a session, the tail of one task's output and its stop. The work is a function
//! over the registry, so it is tested without a server (a second daemon cannot run next to
//! the owner's).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use kagent_core::session::SessionStore;
use kagent_core::tools::background::{Background, TaskStatus};

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

/// What `tail` is when absent, and its range.
pub const DEFAULT_TAIL: usize = 4_000;
pub const TAIL_RANGE: std::ops::RangeInclusive<usize> = 100..=20_000;

/// The redactor of the daemon (token-guard.md), as a tool output goes through it.
pub type Redactor = Arc<dyn Fn(&str) -> String + Send + Sync>;

fn no_task(id: &str) -> HttpError {
    not_found(format!("no background task '{id}'"))
}

/// `?tail=`: absent, or a whole number in `TAIL_RANGE`.
pub fn parse_tail(raw: Option<&str>) -> Result<Option<usize>, HttpError> {
    let Some(raw) = raw else { return Ok(None) };
    match raw.trim().parse::<usize>() {
        Ok(n) if TAIL_RANGE.contains(&n) => Ok(Some(n)),
        _ => Err(bad_request("tail must be a whole number from 100 to 20000")),
    }
}

/// `{tasks}`: those of session `id` (404 for a session that does not exist).
pub async fn list(sessions: &SessionStore, tasks: &Background, id: &str) -> ApiResult<Value> {
    sessions.meta(id).await.map_err(|_| not_found(format!("session {id} not found")))?;
    Ok(json!({"tasks": tasks.list(id)}))
}

/// `{task, output}`: the last `tail` characters of the output, through the redactor.
pub async fn output(tasks: &Background, redact: Option<Redactor>, id: &str, tail: Option<usize>) -> ApiResult<Value> {
    let tail = parse_tail(tail.map(|n| n.to_string()).as_deref())?.unwrap_or(DEFAULT_TAIL);
    let (task, text) = tasks.output_view(id, tail).await.ok_or_else(|| no_task(id))?;
    let text = redact.map_or_else(|| text.clone(), |redact| redact(&text));
    Ok(json!({"task": task, "output": text}))
}

/// Stop task `id`: the answer is the task as it is then (409 when it had ended).
pub async fn stop(tasks: &Background, id: &str) -> ApiResult<Value> {
    let task = tasks.view(id).ok_or_else(|| no_task(id))?;
    if task.status != TaskStatus::Running || !tasks.stop(id) {
        let status = tasks.view(id).map_or(task.status, |t| t.status);
        return Err(HttpError(StatusCode::CONFLICT, "conflict", format!("the task already ended ({})", status.label())));
    }
    Ok(json!(tasks.view(id).ok_or_else(|| no_task(id))?))
}

pub async fn get_list(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    Ok(Json(list(st.agent.store(), &Background::shared(), &id).await?).into_response())
}

pub async fn get_output(State(st): State<Arc<AppState>>, Path(id): Path<String>, Query(query): Query<HashMap<String, String>>) -> ApiResult<Response> {
    let tail = parse_tail(query.get("tail").map(String::as_str))?;
    let redact = st.services.redact.read().unwrap().clone();
    Ok(Json(output(&Background::shared(), redact, &id, tail).await?).into_response())
}

pub async fn post_stop(Path(id): Path<String>) -> ApiResult<Response> {
    Ok(Json(stop(&Background::shared(), &id).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    async fn ended(bg: &Background, id: &str) {
        let end = Instant::now() + Duration::from_secs(10);
        while bg.view(id).is_some_and(|t| t.status == TaskStatus::Running) {
            assert!(Instant::now() < end, "the task ends");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    #[tokio::test]
    async fn the_list_is_of_one_session_and_a_missing_session_is_404() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionStore::new(dir.path().join("sessions")).unwrap();
        let meta = sessions.create("t").await.unwrap();
        let bg = Background::new(dir.path().join("out"));
        let mine = bg.start(&meta.id, "echo mine", None).await.unwrap();
        let other = bg.start("another", "echo other", None).await.unwrap();
        ended(&bg, &mine.id).await;
        ended(&bg, &other.id).await;
        let body = list(&sessions, &bg, &meta.id).await.unwrap();
        let tasks = body["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!((tasks[0]["id"].as_str(), tasks[0]["command"].as_str(), tasks[0]["status"].as_str()), (Some(mine.id.as_str()), Some("echo mine"), Some("done")));
        assert_eq!(tasks[0]["sessionId"], json!(meta.id));
        let err = list(&sessions, &bg, "nope").await.unwrap_err();
        assert_eq!((err.0, err.1, err.2.as_str()), (StatusCode::NOT_FOUND, "not_found", "session nope not found"));
    }

    #[test]
    fn tail_is_absent_or_a_whole_number_from_100_to_20000() {
        assert_eq!(parse_tail(None).unwrap(), None);
        assert_eq!(parse_tail(Some("100")).unwrap(), Some(100));
        assert_eq!(parse_tail(Some("20000")).unwrap(), Some(20_000));
        for bad in ["99", "20001", "abc", "", "-5", "1.5", "4e3"] {
            let err = parse_tail(Some(bad)).unwrap_err();
            assert_eq!((err.0, err.1, err.2.as_str()), (StatusCode::BAD_REQUEST, "invalid_input", "tail must be a whole number from 100 to 20000"), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_output_is_the_tail_and_nothing_of_a_planted_token() {
        let dir = tempfile::tempdir().unwrap();
        let bg = Background::new(dir.path());
        let line = "0123456789".repeat(20);
        let t = bg.start("s", &format!("for i in 1 2 3 4 5 6 7 8 9 10; do echo \"{line}$i\"; done; echo SECRET-TOKEN-1234"), None).await.unwrap();
        ended(&bg, &t.id).await;
        let redact: Redactor = Arc::new(|text: &str| text.replace("SECRET-TOKEN-1234", "<redacted: KAgent API token>"));
        let body = output(&bg, Some(redact.clone()), &t.id, Some(300)).await.unwrap();
        let text = body["output"].as_str().unwrap();
        // The tail is cut before the redactor runs: its replacement may be longer than the token.
        assert!(text.chars().count() <= 300 + "<redacted: KAgent API token>".len() && text.ends_with("<redacted: KAgent API token>\n"), "{text:?}");
        assert!(!text.contains("SECRET-TOKEN"), "the token never shows");
        assert_eq!(body["task"]["id"], json!(t.id));
        assert_eq!(body["task"]["status"], json!("done"));
        // The default tail is the 4000 characters the tool reads: here the whole file.
        let all = output(&bg, Some(redact), &t.id, None).await.unwrap();
        assert!(all["output"].as_str().unwrap().starts_with(&line));
        // No redactor: the text as it is.
        assert!(output(&bg, None, &t.id, Some(100)).await.unwrap()["output"].as_str().unwrap().contains("SECRET-TOKEN-1234"));
        // A tail out of range is refused here as in the route.
        assert_eq!(output(&bg, None, &t.id, Some(99)).await.unwrap_err().0, StatusCode::BAD_REQUEST);
        let err = output(&bg, None, "bg_00000000", None).await.unwrap_err();
        assert_eq!((err.0, err.1, err.2.as_str()), (StatusCode::NOT_FOUND, "not_found", "no background task 'bg_00000000'"));
    }

    #[tokio::test]
    async fn stop_ends_a_running_task_and_a_task_that_ended_is_a_409() {
        let dir = tempfile::tempdir().unwrap();
        let bg = Background::new(dir.path());
        let running = bg.start("s", "sleep 42", None).await.unwrap();
        let body = stop(&bg, &running.id).await.unwrap();
        assert_eq!((body["id"].as_str(), body["status"].as_str()), (Some(running.id.as_str()), Some("killed")));
        let err = stop(&bg, &running.id).await.unwrap_err();
        assert_eq!((err.0, err.1, err.2.as_str()), (StatusCode::CONFLICT, "conflict", "the task already ended (killed)"));
        let quick = bg.start("s", "true", None).await.unwrap();
        ended(&bg, &quick.id).await;
        assert_eq!(stop(&bg, &quick.id).await.unwrap_err().2, "the task already ended (done)");
        assert_eq!(stop(&bg, "bg_00000000").await.unwrap_err().2, "no background task 'bg_00000000'");
    }
}
