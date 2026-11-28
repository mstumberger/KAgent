//! The routes of the runs (workflows.md section 6, api.md section 13). The work is in functions over the engine, tested without a
//! server; the handlers wrap them. Worker text passes the daemon's redactor before it leaves (token-guard.md).

use std::sync::Arc;
use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kagent_core::workflow::mailbox::Answerer;
use kagent_core::workflow::{Decision, Engine, EngineError, Run, StartRequest, TaskEdit};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Runs;
use crate::server::{bad_request, ApiResult, HttpError};
use crate::AppState;

impl From<EngineError> for HttpError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::Invalid(m) => bad_request(m),
            EngineError::NotFound(m) => HttpError(StatusCode::NOT_FOUND, "not_found", m),
            EngineError::Conflict(m) => HttpError(StatusCode::CONFLICT, "conflict", m),
            EngineError::Failed(m) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", m),
        }
    }
}

fn shape(what: &str, e: serde_json::Error) -> HttpError {
    bad_request(format!("{what}: {e}"))
}

/// The list row of a run.
pub fn summary(run: &Run) -> Value {
    let (done, total) = run.counts();
    let needs_owner = run.state == kagent_core::workflow::RunState::AwaitingApproval
        || run.tasks.iter().any(|t| t.state == kagent_core::workflow::TaskState::NeedsOwner)
        || run.questions.iter().any(|q| q.state == kagent_core::workflow::model::QuestionState::Escalated);
    json!({"id": run.id, "title": run.title, "workflow": run.workflow, "project": run.project, "state": run.state, "reason": run.reason,
        "done": done, "total": total, "needsOwner": needs_owner, "ticket": run.ticket, "createdAt": run.created_at, "updatedAt": run.updated_at})
}

pub fn workflows_json(runs: &Runs) -> Value {
    let (defs, invalid) = runs.workflows();
    let rows: Vec<Value> = defs
        .iter()
        .map(|(d, shipped)| {
            let steps: Vec<Value> = d.steps.iter().map(|s| json!({"id": s.id, "role": s.role, "worker": s.worker, "model": s.model, "gate": s.gate})).collect();
            json!({"name": d.name, "description": d.description, "shipped": shipped, "steps": steps})
        })
        .collect();
    json!({"workflows": rows, "invalid": invalid})
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartBody {
    pub workflow: String,
    pub project: String,
    pub goal: String,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub models: HashMap<String, String>,
    #[serde(default)]
    pub session: Option<String>,
}

/// `POST /api/runs`: the folder is checked like any project (sidebar.md section 3).
pub async fn start(engine: &Arc<Engine>, body: &Value) -> Result<Run, HttpError> {
    let b: StartBody = serde_json::from_value(body.clone()).map_err(|e| shape("a run is {workflow, project, goal, ticket?, models?, session?}", e))?;
    let project = crate::project_api::check(&b.project).await?.to_string_lossy().to_string();
    Ok(engine.start_run(StartRequest { workflow: b.workflow, project, goal: b.goal, origin_session: b.session, ticket: b.ticket, models: b.models }).await?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum PlanAction {
    Approve,
    Reject,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlanBody {
    action: PlanAction,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    tasks: Option<Vec<TaskEdit>>,
}

pub fn plan(engine: &Arc<Engine>, id: &str, body: &Value) -> Result<Run, HttpError> {
    let b: PlanBody = serde_json::from_value(body.clone()).map_err(|e| shape("expected {action: approve | reject, reason?, tasks?}", e))?;
    Ok(match b.action {
        PlanAction::Approve => engine.approve_plan(id, b.tasks)?,
        PlanAction::Reject => engine.reject_plan(id, b.reason.as_deref().unwrap_or_default())?,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionBody {
    choice: Decision,
}

pub fn decision(engine: &Arc<Engine>, id: &str, task: &str, body: &Value) -> Result<Run, HttpError> {
    let b: DecisionBody = serde_json::from_value(body.clone()).map_err(|e| shape("expected {choice: more_round | accept | drop}", e))?;
    Ok(engine.decide(id, task, b.choice)?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnswerBody {
    text: String,
}

pub fn answer(engine: &Arc<Engine>, id: &str, qid: &str, body: &Value) -> Result<Run, HttpError> {
    let b: AnswerBody = serde_json::from_value(body.clone()).map_err(|e| shape("expected {text}", e))?;
    engine.answer_question(id, qid, &b.text, Answerer::Owner)?;
    engine.get(id).ok_or_else(|| HttpError(StatusCode::NOT_FOUND, "not_found", format!("no run '{id}'")))
}

/// The text of an answer with the daemon's redactor over it, as a tool output has.
pub fn redact_text(redact: Option<&Arc<dyn Fn(&str) -> String + Send + Sync>>, value: &Value) -> String {
    let text = value.to_string();
    match redact {
        Some(redact) => redact(&text),
        None => text,
    }
}

fn redacted(st: &AppState, status: StatusCode, value: Value) -> Response {
    let text = redact_text(st.services.redact.read().unwrap().as_ref(), &value);
    (status, [(axum::http::header::CONTENT_TYPE, "application/json")], text).into_response()
}

fn run_json(run: &Run) -> Value {
    serde_json::to_value(run).unwrap_or(Value::Null)
}

fn get_run(st: &AppState, id: &str) -> Result<Run, HttpError> {
    st.runs.engine.get(id).ok_or_else(|| HttpError(StatusCode::NOT_FOUND, "not_found", format!("no run '{id}'")))
}

pub async fn workflows(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(Json(workflows_json(&st.runs)).into_response())
}

pub async fn list(State(st): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> ApiResult<Response> {
    let project = q.get("project").map(|p| p.trim()).filter(|p| !p.is_empty());
    let runs: Vec<Value> = st.runs.engine.list(project).iter().map(summary).collect();
    Ok(redacted(&st, StatusCode::OK, json!({"runs": runs})))
}

pub async fn post(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let run = start(&st.runs.engine, &body).await?;
    Ok(redacted(&st, StatusCode::CREATED, run_json(&run)))
}

pub async fn get(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    Ok(redacted(&st, StatusCode::OK, run_json(&get_run(&st, &id)?)))
}

pub async fn stop(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let run = st.runs.engine.stop_run(&id).await?;
    Ok(redacted(&st, StatusCode::OK, run_json(&run)))
}

pub async fn pause(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let run = st.runs.engine.pause_run(&id).await?;
    Ok(redacted(&st, StatusCode::OK, run_json(&run)))
}

pub async fn resume(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let run = st.runs.engine.continue_run(&id).await?;
    Ok(redacted(&st, StatusCode::OK, run_json(&run)))
}

pub async fn post_plan(State(st): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    let run = plan(&st.runs.engine, &id, &body)?;
    Ok(redacted(&st, StatusCode::OK, run_json(&run)))
}

pub async fn post_decision(State(st): State<Arc<AppState>>, Path((id, task)): Path<(String, String)>, Json(body): Json<Value>) -> ApiResult<Response> {
    let run = decision(&st.runs.engine, &id, &task, &body)?;
    Ok(redacted(&st, StatusCode::OK, run_json(&run)))
}

pub async fn post_answer(State(st): State<Arc<AppState>>, Path((id, qid)): Path<(String, String)>, Json(body): Json<Value>) -> ApiResult<Response> {
    let run = answer(&st.runs.engine, &id, &qid, &body)?;
    Ok(redacted(&st, StatusCode::OK, run_json(&run)))
}

/// The changes of one task: the working tree against the last commit, only the files its worker changed (workflows.md section 8).
pub async fn task_diff(State(st): State<Arc<AppState>>, Path((id, task)): Path<(String, String)>) -> ApiResult<Response> {
    let run = get_run(&st, &id)?;
    let t = run.task(&task).ok_or_else(|| HttpError(StatusCode::NOT_FOUND, "not_found", format!("no task '{task}' in this run")))?;
    let mut diff = crate::diff::git::diff(std::path::Path::new(&run.project), "HEAD", None).await?;
    let wanted = kagent_core::workflow::relative_files(&run.project, if t.changed_files.is_empty() { &t.files } else { &t.changed_files });
    diff.files.retain(|f| wanted.iter().any(|w| &f.path == w || f.path.starts_with(&format!("{}/", w.trim_end_matches('/')))));
    let diff = crate::diff::Diff::of(diff.files, diff.truncated);
    Ok(redacted(&st, StatusCode::OK, json!({"project": run.project, "from": "HEAD", "files": wanted, "diff": diff})))
}
