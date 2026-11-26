//! The tasks routes (api.md section 6, automations.md section 5). The work is a function
//! over the store, so it is tested without a server; the handlers parse and answer. Every
//! route that changes the list answers the whole list.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Local};
use serde_json::{json, Value};

use super::schedule::{self, Schedule};
use super::store::{TaskError, TaskInput, TaskStore};
use super::templates;
use crate::server::{ApiResult, HttpError};
use crate::AppState;

/// `{tasks: [...]}`, each task with its `scheduleText`.
pub fn views(store: &TaskStore) -> Value {
    json!({ "tasks": store.list().iter().map(TaskStore::view).collect::<Vec<_>>() })
}

fn http_error(error: TaskError) -> HttpError {
    match error {
        TaskError::NotFound(id) => HttpError(StatusCode::NOT_FOUND, "not_found", format!("no task {id}")),
        TaskError::Busy(why) => HttpError(StatusCode::CONFLICT, "conflict", why),
        TaskError::Invalid(why) => HttpError(StatusCode::BAD_REQUEST, "invalid_input", why),
    }
}

pub fn create_task(store: &TaskStore, body: &Value, now: DateTime<Local>) -> Result<Value, TaskError> {
    store.add(TaskInput::parse(body.clone())?, now).map(|_| views(store))
}

/// A template of the Tasks tab: its task, paused.
pub fn add_a_template(store: &TaskStore, id: &str, now: DateTime<Local>) -> Result<Value, TaskError> {
    store.add_template(id, now).map(|_| views(store))
}

pub fn change_task(store: &TaskStore, id: &str, body: &Value, now: DateTime<Local>) -> Result<Value, TaskError> {
    store.update(id, body.clone(), now).map(|_| views(store))
}

pub fn remove_task(store: &TaskStore, id: &str) -> Result<Value, TaskError> {
    store.delete(id).map(|_| views(store))
}

pub fn request_run(store: &TaskStore, id: &str) -> Result<Value, TaskError> {
    store.request_run(id).map(|_| views(store))
}

pub fn runs_of(store: &TaskStore, id: &str) -> Result<Value, TaskError> {
    Ok(json!({ "runs": store.runs(id)? }))
}

pub fn cancel_run(store: &TaskStore, id: &str) -> Result<Value, TaskError> {
    Ok(json!({ "cancelled": store.cancel(id)? }))
}

/// What a schedule in words or cron comes to: the schedule, its text and its next run.
pub fn preview_of(text: &str, now: DateTime<Local>) -> Result<Value, TaskError> {
    let schedule = schedule::parse(text, now)?;
    let text = schedule.as_ref().map(Schedule::describe).unwrap_or_else(|| schedule::ON_DEMAND.to_string());
    Ok(json!({ "schedule": schedule, "scheduleText": text, "nextRun": schedule.as_ref().and_then(|s| s.next_run(now)) }))
}

pub async fn list(State(st): State<Arc<AppState>>) -> Json<Value> {
    Json(views(&st.tasks))
}

pub async fn add(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let body = create_task(&st.tasks, &body, Local::now()).map_err(http_error)?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

pub async fn add_template(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let body = add_a_template(&st.tasks, &id, Local::now()).map_err(http_error)?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

pub async fn update(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Json<Value>> {
    change_task(&st.tasks, &id, &body, Local::now()).map(Json).map_err(http_error)
}

pub async fn delete(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    remove_task(&st.tasks, &id).map(Json).map_err(http_error)
}

pub async fn run(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    request_run(&st.tasks, &id).map(Json).map_err(http_error)
}

pub async fn runs(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(runs_of(&st.tasks, &id).map_err(http_error)?))
}

pub async fn cancel(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(cancel_run(&st.tasks, &id).map_err(http_error)?))
}

pub async fn preview(State(_st): State<Arc<AppState>>, Query(query): Query<HashMap<String, String>>) -> ApiResult<Json<Value>> {
    let text = query.get("schedule").map(String::as_str).unwrap_or("");
    Ok(Json(preview_of(text, Local::now()).map_err(http_error)?))
}

pub async fn templates(State(_st): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({ "templates": templates::views(Local::now()) }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::sections::TasksSection;

    fn now() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
    }

    fn store() -> (Arc<TaskStore>, tokio::sync::broadcast::Receiver<kagent_core::protocol::AgentEvent>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (events, receiver) = tokio::sync::broadcast::channel(64);
        (TaskStore::load(dir.path(), events, TasksSection::default()), receiver, dir)
    }

    #[test]
    fn a_bad_schedule_and_an_unknown_policy_value_are_refused_nothing_is_stored() {
        let (store, mut receiver, _dir) = store();
        for (body, needle) in [
            (json!({"what": "x", "schedule": "sometimes"}), schedule::EXAMPLES),
            (json!({"what": "x", "onApproval": "ignore"}), "unknown variant `ignore`, expected `deny` or `wait`"),
            (json!({"what": "x", "session": "forever"}), "unknown variant `forever`, expected `new` or `reuse`"),
            (json!({"what": "x", "deliver": ["pigeon"]}), "deliver: unknown channel 'pigeon' (desktop, session, telegram)"),
        ] {
            let error = create_task(&store, &body, now()).unwrap_err();
            assert!(matches!(error, TaskError::Invalid(ref message) if message.contains(needle)), "{body}: {error}");
        }
        assert!(store.list().is_empty() && receiver.try_recv().is_err(), "nothing was stored or saved");
        // How each kind of error is answered.
        assert_eq!((http_error(TaskError::NotFound("t1".into())).0, http_error(TaskError::NotFound("t1".into())).1), (StatusCode::NOT_FOUND, "not_found"));
        assert_eq!(http_error(TaskError::Busy("again".into())).1, "conflict");
        assert_eq!(http_error(TaskError::Invalid("no".into())).0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn every_route_answers_the_whole_list() {
        let (store, mut receiver, _dir) = store();
        let made = create_task(&store, &json!({"title": "Stand up", "what": "Stretch", "kind": "remind", "schedule": "daily 09:00", "deliver": ["desktop"]}), now()).unwrap();
        let task = &made["tasks"][0];
        assert_eq!(task["scheduleText"], "daily at 09:00");
        assert_eq!(task["nextRun"], json!(now() + chrono::Duration::days(1) - chrono::Duration::hours(1)), "nine tomorrow");
        assert_eq!((task["deliver"].clone(), task["onApproval"].clone(), task["timeoutMin"].clone()), (json!(["desktop"]), json!("deny"), json!(30)), "a policy field left out comes from settings.tasks");
        assert_eq!(views(&store), made);
        let id = task["id"].as_str().unwrap().to_string();
        assert!(matches!(receiver.try_recv(), Ok(kagent_core::protocol::AgentEvent::TasksChanged)));
        // The name the schedule text had before S28 still works on a change.
        assert_eq!(change_task(&store, &id, &json!({"when": "weekdays 08:30"}), now()).unwrap()["tasks"][0]["scheduleText"], "weekdays at 08:30");
        assert_eq!(change_task(&store, &id, &json!({"title": "A new title", "schedule": "sometimes"}), now()).unwrap_err().to_string(), "unknown schedule 'sometimes'. Use e.g. 'in 20m', 'weekdays 08:30', '0 9 * * 1-5'");
        assert_eq!(store.get(&id).unwrap().title, "Stand up", "a refused change changes nothing, not even its valid part");
        assert_eq!(request_run(&store, &id).unwrap()["tasks"].as_array().unwrap().len(), 1);
        assert_eq!(runs_of(&store, &id).unwrap(), json!({"runs": []}));
        assert_eq!(cancel_run(&store, &id).unwrap(), json!({"cancelled": true}), "the run that was asked for and not started is dropped");
        assert_eq!(cancel_run(&store, &id).unwrap(), json!({"cancelled": false}));
        assert_eq!(remove_task(&store, &id).unwrap(), json!({"tasks": []}));
        assert_eq!((cancel_run(&store, "nope").unwrap_err(), runs_of(&store, "nope").unwrap_err()), (TaskError::NotFound("nope".into()), TaskError::NotFound("nope".into())));
    }

    #[test]
    fn preview_and_templates() {
        let (store, _receiver, _dir) = store();
        let preview = preview_of("*/20 9-17 * * 1-5", now()).unwrap();
        assert_eq!((preview["scheduleText"].clone(), preview["schedule"].clone()), (json!("cron */20 9-17 * * 1-5"), json!({"type": "cron", "expr": "*/20 9-17 * * 1-5"})));
        let empty = preview_of("", now()).unwrap();
        assert!(empty["schedule"].is_null() && empty["nextRun"].is_null());
        assert_eq!(empty["scheduleText"], "to-do (run on demand)");
        assert!(preview_of("sometimes", now()).unwrap_err().to_string().contains(schedule::EXAMPLES));
        assert_eq!(preview_of("in 1m", now()).unwrap()["nextRun"], json!(now() + chrono::Duration::minutes(1)));
        let added = add_a_template(&store, "heartbeat", now()).unwrap();
        let task = &added["tasks"][0];
        assert_eq!((task["enabled"].clone(), task["scheduleText"].clone(), task["title"].clone(), task["kind"].clone()), (json!(false), json!("every 30 minutes"), json!("Heartbeat"), json!("agent")));
        assert!(matches!(add_a_template(&store, "nope", now()), Err(TaskError::Invalid(_))));
    }
}
