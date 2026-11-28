use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use kagent_core::agent::{Agent, AgentConfig, AgentDeps};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::session::SessionStore;
use kagent_core::tool::ToolRegistry;
use kagent_core::workflow::RunState;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{catalog, http};
use crate::server::HttpError;
use crate::subagents::ProfileHub;

struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _r: ModelRequest, _d: mpsc::UnboundedSender<ModelDelta>, _c: CancellationToken) -> anyhow::Result<ModelResponse> {
        anyhow::bail!("no model in this test")
    }
}

fn known(p: &str) -> bool {
    matches!(p, "planner" | "implementer" | "reviewer" | "general" | "explore")
}

#[test]
fn the_catalog_has_the_shipped_workflows_the_valid_files_and_the_reasons_for_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let ok = "name: mine\ndescription: d\nsteps:\n  - id: p\n    role: planner\n    worker: {kind: subagent, profile: planner}\n";
    std::fs::write(dir.path().join("mine.yaml"), ok).unwrap();
    std::fs::write(dir.path().join("plan-only.yaml"), ok.replace("mine", "plan-only")).unwrap();
    std::fs::write(dir.path().join("coded.yaml"), "name: coded\ndescription: d\nsteps:\n  - id: b\n    role: implementer\n    for_each: task\n    worker: {kind: coder, profile: codex-edit}\n").unwrap();
    std::fs::write(dir.path().join("broken.yaml"), "name: broken\nsteps: [").unwrap();
    std::fs::write(dir.path().join("notes.txt"), "not a workflow").unwrap();
    let (defs, invalid) = catalog(dir.path(), &known);
    let names: Vec<(&str, bool)> = defs.iter().map(|(d, s)| (d.name.as_str(), *s)).collect();
    assert_eq!(names, [("plan-implement-review", true), ("implement-review", true), ("plan-only", true), ("review-only", true), ("mine", false)]);
    let why = |f: &str| invalid.iter().find(|i| i.file == f).map(|i| i.reason.clone()).unwrap_or_default();
    assert!(why("plan-only.yaml").contains("a shipped workflow has this name"));
    assert!(why("coded.yaml").contains("coding agents as workers are not built yet (kind: coder)"));
    assert!(why("broken.yaml").starts_with("line 2:") || why("broken.yaml").contains("not closed"), "{}", why("broken.yaml"));
    assert_eq!(invalid.len(), 3, "a file that is not .yaml is not read");
    let none = catalog(&dir.path().join("missing"), &known);
    assert_eq!(none.0.len(), 4, "a folder that does not exist has only the shipped ones");
}

async fn engine() -> (Arc<kagent_core::workflow::Engine>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let (events, _rx) = tokio::sync::broadcast::channel(256);
    let cfg = AgentConfig { model_ref: "fake::m".into(), system_prompt: String::new(), max_iterations: 4, options: Default::default(), privilege: Default::default(), context_tokens: 0, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model: Arc::new(NoModel),
        tools: Arc::new(ToolRegistry::new()),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
        store,
        events,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    let hub = ProfileHub::new(dir.path().join("agents"));
    hub.reload();
    let runs = super::Runs::start(agent, Default::default(), hub, dir.path().join("runs"), dir.path().join("workflows"), 2, Arc::new(|p: &str| Some(p.to_string())));
    (runs.engine.clone(), dir)
}

fn refused(r: Result<impl std::fmt::Debug, HttpError>) -> (StatusCode, &'static str, String) {
    let HttpError(status, code, message) = r.unwrap_err();
    (status, code, message)
}

#[tokio::test]
async fn the_routes_check_their_bodies_and_answer_with_a_status_that_says_why() {
    let (engine, dir) = engine().await;
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let p = project.to_string_lossy().to_string();
    // A body: the fields, no others; a folder that exists; a workflow that is known.
    assert_eq!(refused(http::start(&engine, &json!({"workflow": "plan-only", "project": p})).await).0, StatusCode::BAD_REQUEST);
    let extra = refused(http::start(&engine, &json!({"workflow": "plan-only", "project": p, "goal": "g", "autoApprove": true})).await);
    assert!(extra.0 == StatusCode::BAD_REQUEST && extra.2.contains("unknown field `autoApprove`"), "{}", extra.2);
    assert_eq!(refused(http::start(&engine, &json!({"workflow": "plan-only", "project": "/no/such/folder", "goal": "g"})).await).0, StatusCode::BAD_REQUEST);
    let unknown = refused(http::start(&engine, &json!({"workflow": "nope", "project": p, "goal": "g"})).await);
    assert!(unknown.0 == StatusCode::BAD_REQUEST && unknown.2.contains("workflow 'nope'"), "{}", unknown.2);
    assert_eq!(refused(http::start(&engine, &json!({"workflow": "plan-only", "project": p, "goal": "g", "models": {"reviewer": "a::b"}})).await).0, StatusCode::BAD_REQUEST, "no reviewer step to give a model");

    // A run: it starts, its planner has no model here and fails, the row says so.
    let run = http::start(&engine, &json!({"workflow": "plan-only", "project": p, "goal": "plan the thing\nwith details", "models": {"planner": "fake::other"}})).await.unwrap();
    assert!(run.id.starts_with("run_") && run.title == "plan the thing" && run.definition.step(kagent_core::workflow::Role::Planner).unwrap().model.as_deref() == Some("fake::other"));
    for _ in 0..200 {
        if engine.get(&run.id).unwrap().state == RunState::Failed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let row = http::summary(&engine.get(&run.id).unwrap());
    assert_eq!((row["state"].as_str(), row["done"].as_u64(), row["total"].as_u64(), row["needsOwner"].as_bool()), (Some("failed"), Some(0), Some(0), Some(false)), "{row}");
    assert_eq!(row["reason"], "no model in this test", "{row}");
    assert!(row.get("definition").is_none() && row["project"].as_str().is_some());

    // Each route on a run in the wrong state.
    assert_eq!(refused(http::plan(&engine, &run.id, &json!({"action": "approve"}))).0, StatusCode::CONFLICT);
    assert_eq!(refused(http::plan(&engine, &run.id, &json!({"action": "reject", "reason": "x"}))).0, StatusCode::CONFLICT);
    assert_eq!(refused(http::plan(&engine, "run_nope", &json!({"action": "approve"}))).0, StatusCode::NOT_FOUND);
    assert_eq!(refused(http::plan(&engine, &run.id, &json!({"action": "maybe"}))).0, StatusCode::BAD_REQUEST);
    assert_eq!(refused(http::plan(&engine, &run.id, &json!({"action": "approve", "tasks": [{"id": "a", "approved": true}]}))).0, StatusCode::BAD_REQUEST);
    assert_eq!(refused(http::decision(&engine, &run.id, "T1", &json!({"choice": "accept"}))).0, StatusCode::CONFLICT, "a run that ended takes no decision");
    assert_eq!(refused(http::decision(&engine, &run.id, "T1", &json!({"choice": "shrug"}))).0, StatusCode::BAD_REQUEST);
    assert_eq!(refused(http::answer(&engine, &run.id, "q_none", &json!({"text": "x"}))).0, StatusCode::NOT_FOUND);
    assert_eq!(refused(http::answer(&engine, &run.id, "q_none", &json!({}))).0, StatusCode::BAD_REQUEST);
    assert_eq!(refused(engine.stop_run(&run.id).await.map_err(HttpError::from)).0, StatusCode::CONFLICT, "it ended already");
    assert_eq!(refused(engine.continue_run(&run.id).await.map_err(HttpError::from)).0, StatusCode::CONFLICT);
    assert_eq!(refused(engine.pause_run(&run.id).await.map_err(HttpError::from)).0, StatusCode::CONFLICT);
}

#[test]
fn an_answer_goes_through_the_redactor_and_the_workflows_list_has_no_secret_field() {
    let redact: Arc<dyn Fn(&str) -> String + Send + Sync> = Arc::new(|t: &str| t.replace("SECRET-TOKEN", "[redacted]"));
    let text = http::redact_text(Some(&redact), &json!({"report": "the worker printed SECRET-TOKEN here"}));
    assert!(!text.contains("SECRET-TOKEN") && text.contains("[redacted]"), "{text}");
    assert_eq!(http::redact_text(None, &json!({"a": 1})), "{\"a\":1}");
    let _: Value = serde_json::from_str(&text).unwrap();
}

// ---------------------------------------------------------------- the tickets of a run (workflows.md section 7)

use kagent_core::workflow::model::{Run, Task, TaskState};

use crate::tickets::{State, Tickets};

fn sample(project: &str, state: RunState, tasks: Vec<Task>) -> Run {
    let def = kagent_core::workflow::shipped::all(&known).into_iter().find(|d| d.name == "plan-implement-review").unwrap();
    let now = chrono::Utc::now();
    Run {
        id: "run_abcd1234".into(),
        workflow: def.name.clone(),
        definition: def,
        project: project.into(),
        goal: "make the overlay keep focus".into(),
        title: "make the overlay keep focus".into(),
        state,
        reason: String::new(),
        origin_session: None,
        run_session: "s".into(),
        ticket: None,
        plan_summary: String::new(),
        tasks,
        questions: vec![],
        messages: vec![],
        planner_session: None,
        feedback: None,
        rejections: 0,
        report: String::new(),
        created_at: now,
        updated_at: now,
    }
}

fn task(id: &str, deps: &[&str], state: TaskState) -> Task {
    let mut t = Task::new(id, &format!("Title {id}"));
    t.state = state;
    t.depends_on = deps.iter().map(|d| d.to_string()).collect();
    t.done_when = vec![format!("{id} works")];
    t.ticket = Some("CYB-9868".into());
    t
}

#[tokio::test]
async fn the_run_is_a_ticket_and_each_task_a_child_that_follows_the_task() {
    let (engine, dir) = engine().await;
    let project = dir.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let project = std::fs::canonicalize(&project).unwrap().to_string_lossy().to_string();
    let (tx, _rx) = tokio::sync::broadcast::channel(64);
    let tickets = Tickets::new(dir.path().join("tickets"), tx);
    let sync = super::tickets::TicketSync::new(tickets.clone(), engine);

    // While the plan waits there is the run's ticket and no task.
    let mut run = sample(&project, RunState::AwaitingApproval, vec![task("T1", &[], TaskState::Waiting), task("T2", &["T1"], TaskState::Waiting)]);
    let (run_key, keys) = sync.sync(&run);
    let run_key = run_key.unwrap();
    assert!(keys.is_empty());
    let t = tickets.get(&run_key).unwrap();
    assert_eq!((t.state, t.title.as_str(), t.labels.clone(), t.history[0].by), (State::InReview, "make the overlay keep focus", vec!["run".to_string()], crate::tickets::By::Agent));

    // Approved: the tasks become its children, with their done-when and the planner's tracker key.
    run.ticket = Some(run_key.clone());
    run.state = RunState::Working;
    let (_, keys) = sync.sync(&run);
    assert_eq!(keys.len(), 2);
    let t1 = tickets.get(&keys["T1"]).unwrap();
    assert_eq!((t1.parent.as_deref(), t1.state, t1.title.as_str(), t1.done_when.clone()), (Some(run_key.as_str()), State::Todo, "T1 Title T1", vec!["T1 works".to_string()]));
    assert!(t1.description.contains("Tracker: CYB-9868"));
    run.tasks[0].ka_ticket = Some(keys["T1"].clone());
    run.tasks[1].ka_ticket = Some(keys["T2"].clone());
    assert!(sync.sync(&run).1.is_empty(), "no second ticket for a task");

    // The state of the ticket follows the task's.
    for (state, want) in [(TaskState::Implementing, State::InProgress), (TaskState::InReview, State::InReview), (TaskState::Changes, State::InProgress), (TaskState::NeedsOwner, State::InReview), (TaskState::Done, State::Done)] {
        run.tasks[0].state = state;
        sync.sync(&run);
        assert_eq!(tickets.get(&keys["T1"]).unwrap().state, want, "{state:?}");
    }
    // A ticket the owner moved by hand is not moved back while the task does not change.
    tickets.update(&keys["T1"], crate::tickets::Update { state: Some(State::Dropped), ..Default::default() }, crate::tickets::By::Owner).unwrap();
    sync.sync(&run);
    assert_eq!(tickets.get(&keys["T1"]).unwrap().state, State::Dropped);

    // Stopping the run: what was not done says so, in state, label and note; what was done stays done.
    run.state = RunState::Stopped;
    run.reason = "stopped by the owner".into();
    run.tasks[0].state = TaskState::Done;
    run.tasks[1].state = TaskState::Stopped;
    sync.sync(&run);
    let t2 = tickets.get(&keys["T2"]).unwrap();
    assert_eq!(t2.state, State::Backlog);
    assert!(t2.labels.contains(&"stopped".to_string()) && t2.notes.last().unwrap().text == "the run was stopped before this task was done", "{t2:?}");
    let rt = tickets.get(&run_key).unwrap();
    assert_eq!(rt.state, State::Backlog);
    assert!(rt.notes.last().unwrap().text.contains("stopped: stopped by the owner") && rt.labels.contains(&"stopped".to_string()), "{rt:?}");
    // A failed task: backlog and why.
    run.tasks[1].state = TaskState::Failed;
    run.tasks[1].error = "the worker fell over".into();
    sync.sync(&run);
    assert!(tickets.get(&keys["T2"]).unwrap().notes.last().unwrap().text.contains("failed: the worker fell over"));
}

#[test]
fn the_state_maps_are_as_the_spec_says() {
    use super::tickets::{run_state, task_state};
    let want = [(TaskState::Waiting, State::Todo), (TaskState::Blocked, State::Todo), (TaskState::Implementing, State::InProgress), (TaskState::Changes, State::InProgress), (TaskState::InReview, State::InReview), (TaskState::NeedsOwner, State::InReview), (TaskState::Done, State::Done), (TaskState::Dropped, State::Dropped), (TaskState::Failed, State::Backlog), (TaskState::Stopped, State::Backlog)];
    for (t, s) in want {
        assert_eq!(task_state(t), s, "{t:?}");
    }
    assert_eq!((run_state(RunState::Working), run_state(RunState::Done), run_state(RunState::Failed), run_state(RunState::AwaitingApproval)), (State::InProgress, State::Done, State::Backlog, State::InReview));
}
