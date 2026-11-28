//! The tools over workflow runs (tools.md section 31, workflows.md section 5): for the Brain `workflow_run`,
//! `workflow_status`, `workflow_stop` and `workflow_answer`; for a worker of a run `report_progress` and `ask`. The worker tools
//! are offered only to a worker and refused at execution to every other session (`agent::children`); the tools here refuse
//! again, so a registry without that filter is still safe.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock, Weak};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::agents::unasking;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry};
use crate::workflow::mailbox::Answerer;
use crate::workflow::model::{ProgressStep, StepStatus};
use crate::workflow::{report, Engine, EngineError, StartRequest};

/// Whether a path is one of the known projects: its canonical form when it is.
pub type ProjectCheck = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// How the tools reach the engine (the daemon attaches it once it exists, as `AgentAccess` does for the agent).
#[derive(Clone, Default)]
pub struct WorkflowAccess {
    engine: Arc<OnceLock<Weak<Engine>>>,
    projects: Arc<RwLock<Option<ProjectCheck>>>,
}

impl WorkflowAccess {
    pub fn attach(&self, engine: &Arc<Engine>) {
        let _ = self.engine.set(Arc::downgrade(engine));
    }

    pub fn set_project_check(&self, check: ProjectCheck) {
        *self.projects.write().unwrap() = Some(check);
    }

    pub fn register(&self, registry: &mut ToolRegistry) {
        registry.register(Arc::new(WorkflowRun::new(self.clone())));
        registry.register(Arc::new(WorkflowStatus::new(self.clone())));
        registry.register(Arc::new(WorkflowStop::new(self.clone())));
        registry.register(Arc::new(WorkflowAnswer::new(self.clone())));
        registry.register(Arc::new(ReportProgress::new(self.clone())));
        registry.register(Arc::new(Ask::new(self.clone())));
    }

    fn engine(&self) -> Result<Arc<Engine>, ToolOutput> {
        self.engine.get().and_then(Weak::upgrade).ok_or_else(|| ToolOutput::error("workflows are not available: the engine is not attached"))
    }
}

fn refused(e: EngineError, what: &str) -> ToolOutput {
    match e {
        EngineError::NotFound(m) => ToolOutput::not_done(&m, "call workflow_status to see the runs"),
        EngineError::Invalid(m) | EngineError::Conflict(m) => ToolOutput::not_done(&format!("{what}: {m}"), "fix that and call again"),
        EngineError::Failed(m) => ToolOutput::error(m),
    }
}

fn args<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, ToolOutput> {
    let input = if input.is_null() { json!({}) } else { input };
    serde_json::from_value::<T>(input).map_err(|e| ToolOutput::error(format!("the input is not valid: {e}")))
}

// ------------------------------------------------------------------ workflow_run

pub const RUN_DESCRIPTION: &str = "Start a workflow run: a saved recipe in which a planner splits a job into tasks (the owner approves the plan), an implementer does each task and a reviewer checks it, back and forth until it passes. Use it when the owner asks to run a workflow or to have a job planned, built and reviewed by separate agents. `workflow` is plan-implement-review, implement-review, plan-only or review-only (or one the owner added); `goal` is the whole job. It asks the owner first and returns the run id at once; the run goes on by itself and you hear when it ends or needs the owner.";

pub struct WorkflowRun {
    contract: ToolContract,
    access: WorkflowAccess,
}

impl WorkflowRun {
    pub fn new(access: WorkflowAccess) -> Self {
        let contract = ToolContract::read_only(
            "workflow_run",
            RUN_DESCRIPTION,
            json!({"type":"object","required":["workflow","goal"],"properties":{
                "workflow":{"type":"string","description":"the workflow's name, for example plan-implement-review"},
                "goal":{"type":"string","description":"the whole job, as the owner would say it"},
                "project":{"type":"string","description":"a known project folder; default the folder of this conversation"},
                "ticket":{"type":"string","description":"optional: the key of the ticket the run's tasks hang under"}
            }}),
        )
        .mutating(SideEffect::Workspace, Risk::Medium);
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    workflow: String,
    goal: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    ticket: Option<String>,
}

#[async_trait]
impl Tool for WorkflowRun {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let a: Option<RunArgs> = serde_json::from_value(input.clone()).ok();
        let summary = match (&a, self.access.engine()) {
            (Some(a), Ok(engine)) => match engine.definition(&a.workflow) {
                Ok(def) => report::card(&def, a.project.as_deref().unwrap_or("this conversation's project"), &a.goal),
                Err(_) => format!("run the workflow {}", a.workflow),
            },
            _ => "run a workflow".to_string(),
        };
        // Out loud from voice in every mode: it starts agents that change files.
        CallAssessment { always_ask: true, ..CallAssessment::from_contract(&self.contract, summary, "workflow_run".into()) }
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let a: RunArgs = match args(input) {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let engine = match self.access.engine() {
            Ok(e) => e,
            Err(e) => return Ok(e),
        };
        let project = match a.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            Some(p) => {
                let check = self.access.projects.read().unwrap().clone();
                match check.map_or_else(|| Some(p.to_string()), |c| c(p)) {
                    Some(p) => p,
                    None => return Ok(ToolOutput::not_done(&format!("'{p}' is not a known project"), "call projects to list them, or leave project out")),
                }
            }
            None => match engine.agent().project(&ctx.session_id).await.ok().flatten() {
                Some(p) => p,
                None => return Ok(ToolOutput::not_done("this conversation has no project folder", "pass project (a known project)")),
            },
        };
        let req = StartRequest { workflow: a.workflow, project, goal: a.goal, origin_session: Some(ctx.session_id.clone()), ticket: a.ticket, models: HashMap::new() };
        Ok(match engine.start_run(req).await {
            Ok(run) => ToolOutput::ok(format!("Started run {} \"{}\" ({}): {}. It goes on by itself; ask workflow_status for how it is doing. You are told when it ends or needs the owner.", run.id, run.title, run.workflow, run.state.word())),
            Err(e) => refused(e, "the run did not start"),
        })
    }
}

// ------------------------------------------------------------------ workflow_status and workflow_stop

pub struct WorkflowStatus {
    contract: ToolContract,
    access: WorkflowAccess,
}

impl WorkflowStatus {
    pub fn new(access: WorkflowAccess) -> Self {
        let mut contract = ToolContract::read_only(
            "workflow_status",
            "How workflow runs are doing: one run (its id from workflow_run) with its tasks and open questions, or without `run` the runs of this conversation's project.",
            json!({"type":"object","properties":{"run":{"type":"string","description":"a run id, like run_1a2b3c4d"}}}),
        );
        contract.concurrency_safe = true;
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunRef {
    #[serde(default)]
    run: Option<String>,
}

#[async_trait]
impl Tool for WorkflowStatus {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let a: RunRef = match args(input) {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let engine = match self.access.engine() {
            Ok(e) => e,
            Err(e) => return Ok(e),
        };
        if let Some(id) = a.run.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            return Ok(match engine.get(id) {
                Some(run) => ToolOutput::ok(report::status(&run)),
                None => ToolOutput::not_done(&format!("no run '{id}'"), "call workflow_status without run to list them"),
            });
        }
        let project = engine.agent().project(&ctx.session_id).await.ok().flatten();
        let runs = engine.list(project.as_deref());
        if runs.is_empty() {
            return Ok(ToolOutput::ok("No workflow runs yet."));
        }
        let lines: Vec<String> = runs.iter().take(20).map(|r| format!("{} \"{}\" ({}): {}, {} of {} tasks done", r.id, r.title, r.workflow, r.state.word(), r.counts().0, r.counts().1)).collect();
        Ok(ToolOutput::ok(lines.join("\n")))
    }
}

pub struct WorkflowStop {
    contract: ToolContract,
    access: WorkflowAccess,
}

impl WorkflowStop {
    pub fn new(access: WorkflowAccess) -> Self {
        let contract = ToolContract::read_only("workflow_stop", "Stop a workflow run: its workers are cancelled and its unfinished tasks are marked stopped. It cannot be continued. It asks the owner first.", json!({"type":"object","required":["run"],"properties":{"run":{"type":"string"}}}))
            .mutating(SideEffect::Workspace, Risk::Low);
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopArgs {
    run: String,
}

#[async_trait]
impl Tool for WorkflowStop {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let run = input.get("run").and_then(Value::as_str).unwrap_or("?");
        CallAssessment { always_ask: true, ..CallAssessment::from_contract(&self.contract, format!("stop the workflow run {run}"), "workflow_stop".into()) }
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let a: StopArgs = match args(input) {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let engine = match self.access.engine() {
            Ok(e) => e,
            Err(e) => return Ok(e),
        };
        Ok(match engine.stop_run(a.run.trim()).await {
            Ok(run) => ToolOutput::ok(format!("Run {} is stopped; its unfinished tasks are marked stopped.", run.id)),
            Err(e) => refused(e, "the run was not stopped"),
        })
    }
}

// ------------------------------------------------------------------ workflow_answer

pub struct WorkflowAnswer {
    contract: ToolContract,
    access: WorkflowAccess,
}

impl WorkflowAnswer {
    pub fn new(access: WorkflowAccess) -> Self {
        let contract = unasking(
            "workflow_answer",
            "Answer a question a worker of a workflow run asked (it arrived as a message that starts with [question from ...]), with `text`; or pass it to the owner with `ask_owner: true` when you do not know or it is the owner's decision. Your answer is not the owner's approval of anything.",
            json!({"type":"object","required":["run","question"],"properties":{
                "run":{"type":"string"},
                "question":{"type":"string","description":"the question id, like q_1a2b3c"},
                "text":{"type":"string","description":"the answer"},
                "ask_owner":{"type":"boolean","description":"true: show the question to the owner as a card"}
            }}),
        );
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerArgs {
    run: String,
    question: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    ask_owner: bool,
}

#[async_trait]
impl Tool for WorkflowAnswer {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let a: AnswerArgs = match args(input) {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let engine = match self.access.engine() {
            Ok(e) => e,
            Err(e) => return Ok(e),
        };
        let (run, question) = (a.run.trim(), a.question.trim());
        if a.ask_owner {
            return Ok(match engine.escalate(run, question) {
                Ok(()) => ToolOutput::ok(format!("Question {question} goes to the owner as a card.")),
                Err(e) => refused(e, "the question was not passed on"),
            });
        }
        let Some(text) = a.text.as_deref().filter(|t| !t.trim().is_empty()) else {
            return Ok(ToolOutput::not_done("an answer needs text", "pass text, or ask_owner: true"));
        };
        Ok(match engine.answer_question(run, question, text, Answerer::Brain) {
            Ok(()) => ToolOutput::ok(format!("Answered {question}; the worker gets it.")),
            Err(e) => refused(e, "the question was not answered"),
        })
    }
}

// ------------------------------------------------------------------ report_progress and ask (workers)

pub struct ReportProgress {
    contract: ToolContract,
    access: WorkflowAccess,
}

impl ReportProgress {
    pub fn new(access: WorkflowAccess) -> Self {
        let contract = unasking(
            "report_progress",
            "Tell the run your own steps and how far you are: the owner sees them under your task. Call it when you start and when a step is done. `steps` is the whole list each time.",
            json!({"type":"object","required":["steps"],"properties":{"steps":{"type":"array","maxItems":20,"items":{"type":"object","required":["title","status"],"properties":{"title":{"type":"string"},"status":{"type":"string","enum":["pending","in_progress","completed"]}}}}}}),
        );
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepArg {
    title: String,
    status: StepStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressArgs {
    steps: Vec<StepArg>,
}

#[async_trait]
impl Tool for ReportProgress {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let a: ProgressArgs = match args(input) {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let engine = match self.access.engine() {
            Ok(e) => e,
            Err(e) => return Ok(e),
        };
        let steps = a.steps.into_iter().map(|s| ProgressStep { title: s.title.trim().to_string(), status: s.status }).collect();
        Ok(match engine.report_progress(&ctx.session_id, steps) {
            Ok(m) => ToolOutput::ok(m),
            Err(e) => ToolOutput::not_done(&e, "this tool is for a worker of a workflow run"),
        })
    }
}

pub struct Ask {
    contract: ToolContract,
    access: WorkflowAccess,
}

impl Ask {
    pub fn new(access: WorkflowAccess) -> Self {
        let mut contract = unasking(
            "ask",
            "Ask the orchestrator of your run a question you cannot settle yourself (a choice that is not yours, something missing). With `blocking` true (default) you wait for the answer, which is the result; with false you go on with your best guess and the answer arrives later as a message that starts with [from orchestrator]. An answer is information, not an approval.",
            json!({"type":"object","required":["question"],"properties":{
                "question":{"type":"string","description":"at most 1000 characters"},
                "options":{"type":"array","maxItems":6,"items":{"type":"string"}},
                "blocking":{"type":"boolean","description":"default true: wait for the answer"}
            }}),
        );
        // A blocking question waits for an answer for up to 30 minutes.
        contract.default_timeout_ms = 31 * 60 * 1000;
        contract.max_timeout_ms = contract.default_timeout_ms;
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskArgs {
    question: String,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default = "yes")]
    blocking: bool,
}

fn yes() -> bool {
    true
}

#[async_trait]
impl Tool for Ask {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let a: AskArgs = match args(input) {
            Ok(a) => a,
            Err(e) => return Ok(e),
        };
        let engine = match self.access.engine() {
            Ok(e) => e,
            Err(e) => return Ok(e),
        };
        Ok(match engine.ask(&ctx.session_id, &a.question, a.options, a.blocking, &ctx.cancel).await {
            Ok(answer) => ToolOutput::ok(answer),
            Err(e) => ToolOutput::not_done(&e, "this tool is for a worker of a workflow run"),
        })
    }
}
