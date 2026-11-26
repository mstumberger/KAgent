//! The tools of a main agent over its subagents (tools.md section 21): `agent_run`,
//! `agent_wait`, `agent_stop`, `agent_list`. The orchestration is `agent::children`; the
//! tools reach it, and the profiles, through `AgentAccess`, as the todo tools reach the
//! sessions through `TodoAccess`. `task_output` and `task_stop` are `tasktools`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::agent::children::{self, ChildRequest, ChildStatus, ChildView, End, Profile, GENERAL, MAX_DESCRIPTION_CHARS, MAX_OUTPUT_CHARS, WAIT_OUTPUT_CHARS};
use crate::agent::Agent;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput, ToolRegistry};

/// How long `agent_wait` looks again (it does not need to be exact to the millisecond).
const POLL: Duration = Duration::from_millis(200);

/// "MUT, risk low, no approval" (tools.md section 21): the call changes nothing by itself,
/// so it asks nobody, and it may run next to other calls (two `agent_run`s in one answer
/// start together).
pub(super) fn unasking(name: &str, description: &str, schema: Value) -> ToolContract {
    let mut contract = ToolContract::read_only(name, description, schema).mutating(SideEffect::None, Risk::Low);
    contract.needs_approval = false;
    contract.concurrency_safe = true;
    contract
}

/// Where the profiles are read before each `agent_run`: the daemon's folder, so a file
/// edited by hand counts at once.
pub type ProfileSource = Arc<dyn Fn() -> Vec<Profile> + Send + Sync>;

/// How the tools reach the agent and its profiles. The daemon creates the handle before
/// its registry (the agent does not exist yet), gives it to the tools, attaches the agent
/// once it exists and hands it the profiles of `<config>/agents`.
#[derive(Clone, Default)]
pub struct AgentAccess {
    agent: Arc<OnceLock<Weak<Agent>>>,
    profiles: Arc<RwLock<Vec<Profile>>>,
    source: Arc<OnceLock<ProfileSource>>,
    /// How long `agent_run` with `background: false` waits, in milliseconds (0 = `RUN_WAIT`).
    run_wait: Arc<AtomicU64>,
}

impl AgentAccess {
    /// The agent whose sessions start subagents. The first one stays.
    pub fn attach(&self, agent: &Arc<Agent>) {
        let _ = self.agent.set(Arc::downgrade(agent));
    }

    /// The profiles the tools may start: the daemon's, read from `<config>/agents`.
    pub fn set_profiles(&self, profiles: Vec<Profile>) {
        *self.profiles.write().unwrap() = profiles;
    }

    /// Read the profiles through `source` before each `agent_run`. The first one stays.
    pub fn set_profile_source(&self, source: ProfileSource) {
        let _ = self.source.set(source);
    }

    /// The profiles now: through the source when there is one, else those that were set.
    pub fn profiles(&self) -> Vec<Profile> {
        if let Some(read) = self.source.get() {
            *self.profiles.write().unwrap() = read();
        }
        self.profiles.read().unwrap().clone()
    }

    /// How long `agent_run` with `background: false` waits for an answer (tests shorten it).
    pub fn set_run_wait(&self, wait: Duration) {
        self.run_wait.store(wait.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    /// Register the four tools, each with this handle.
    pub fn register(&self, registry: &mut ToolRegistry) {
        registry.register(Arc::new(AgentRun::new(self.clone())));
        registry.register(Arc::new(AgentWait::new(self.clone())));
        registry.register(Arc::new(AgentStop::new(self.clone())));
        registry.register(Arc::new(AgentList::new(self.clone())));
    }

    fn run_wait(&self) -> Duration {
        let millis = self.run_wait.load(Ordering::Relaxed);
        Duration::from_millis(if millis == 0 { children::RUN_WAIT.as_millis() as u64 } else { millis })
    }

    fn agent(&self) -> anyhow::Result<Arc<Agent>> {
        self.agent.get().and_then(Weak::upgrade).ok_or_else(|| anyhow::anyhow!("subagents are not available: the agent is not attached"))
    }

    fn profile(&self, name: &str) -> Result<Profile, String> {
        let known = self.profiles();
        if let Some(profile) = known.iter().find(|p| p.name == name) {
            return Ok(profile.clone());
        }
        if let Some(profile) = Profile::builtin(name) {
            return Ok(profile);
        }
        let own: Vec<&str> = known.iter().map(|p| p.name.as_str()).filter(|name| Profile::builtin(name).is_none()).collect();
        Err(format!("unknown profile '{name}': general, explore{}", own.iter().map(|name| format!(", {name}")).collect::<String>()))
    }
}

/// One text, or its lines (as `skill_save.body`, tools.md section 17): Ollama drops a tool
/// call whose long argument it cannot parse (S20).
fn text_or_lines<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    use serde::de::Error;
    match Value::deserialize(d)? {
        Value::String(text) => Ok(text),
        Value::Array(items) => items
            .into_iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(|| D::Error::custom("prompt: every line must be text")))
            .collect::<Result<Vec<_>, _>>()
            .map(|lines| lines.join("\n")),
        other => Err(D::Error::custom(format!("prompt: expected text or an array of lines, got {other}"))),
    }
}

/// Wait until the child ended, the time is up, or the calling turn is cancelled.
async fn wait_for(agent: &Arc<Agent>, parent: &str, id: &str, limit: Duration, cancel: &CancellationToken) -> Option<(ChildView, End)> {
    let end = Instant::now() + limit;
    loop {
        let (view, ended, _) = agent.child_of(parent, id)?;
        if let Some(end) = ended {
            return Some((view, end));
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || cancel.is_cancelled() {
            return None;
        }
        tokio::select! {
            _ = tokio::time::sleep(left.min(POLL)) => {}
            _ = cancel.cancelled() => return None,
        }
    }
}

/// The wording was found with the real local model (qwen2.5:7b-instruct, raw replays of the
/// first request of "Research in parallel ..."): with the first one 0 of 8 answers called
/// the tool (two read-only calls of its own instead), with this one 12 of 12 did, two calls
/// in 6 of them.
const RUN_DESCRIPTION: &str = "Delegate jobs to subagents (a second agent with its own session each) and go on yourself: for work that would fill this conversation, or for several jobs at once. For N jobs make N agent_run calls in one answer, one per job (a short `description` each), then call agent_wait and summarize their answers. A subagent sees only a short digest of this conversation, so `prompt` holds the whole job. Its one final answer arrives later as a message that starts with [Agent ...]. Its approvals still ask the user.";
const WAIT_DESCRIPTION: &str = "Wait for the subagents this session started (their ids from agent_run; none given: all of them) and get their answers, until they have ended or `timeout_sec` has passed. An agent that ends on its own reports to you without agent_wait.";
const STOP_DESCRIPTION: &str = "Stop a subagent of this session (its id from agent_run or agent_list). A stopped agent reports nothing.";
const LIST_DESCRIPTION: &str = "The subagents this session started: id, profile, state, how long they ran.";

pub struct AgentRun {
    contract: ToolContract,
    access: AgentAccess,
}

impl AgentRun {
    pub fn new(access: AgentAccess) -> Self {
        let mut contract = unasking(
            "agent_run",
            RUN_DESCRIPTION,
            json!({"type":"object","required":["description","prompt"],"properties":{
                "description":{"type":"string","description":"ONE job in a few words (at most 80 characters), shown to the user; one call per job"},
                "prompt":{"description":"the whole job: the goal, the steps, what to report. One text, or one line per item","anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]},
                "profile":{"type":"string","description":"general (every tool) or explore (read-only research), or a profile of Settings -> Subagents; default general"},
                "model":{"type":"string","description":"Optional. A model ref in the form <providerId>::<modelId>, exactly as Settings -> Models lists it; never a profile name. Leave it out unless a specific model is needed: the profile's model is used, else the chat model."},
                "background":{"type":"boolean","description":"true (default): the answer comes at once; false: wait up to 120 s for the subagent's answer"}
            }}),
        );
        // `background: false` waits up to 120 s (tools.md section 21).
        contract.default_timeout_ms = contract.default_timeout_ms.max(children::RUN_WAIT.as_millis() as u64 + 5_000);
        contract.max_timeout_ms = contract.default_timeout_ms;
        Self { contract, access }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunArgs {
    description: String,
    #[serde(deserialize_with = "text_or_lines")]
    prompt: String,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    background: Option<bool>,
}

#[async_trait]
impl Tool for AgentRun {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let description = input.get("description").and_then(Value::as_str).unwrap_or_default();
        CallAssessment::from_contract(&self.contract, format!("subagent: {}", children::cut(description, MAX_DESCRIPTION_CHARS)), "agent_run".into())
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        // `validate_schema` does not look into `anyOf`: the form is checked here.
        let Ok(args) = serde_json::from_value::<RunArgs>(input) else {
            return Ok(ToolOutput::error("agent_run needs `description` (the job in a few words) and `prompt` (the whole job: one text, or one line per item)"));
        };
        let agent = match self.access.agent() {
            Ok(agent) => agent,
            Err(e) => return Ok(ToolOutput::error(e.to_string())),
        };
        let profile = match self.access.profile(args.profile.as_deref().unwrap_or(GENERAL)) {
            Ok(profile) => profile,
            Err(reason) => return Ok(ToolOutput::error(reason)),
        };
        let request = ChildRequest { parent: ctx.session_id.clone(), description: args.description, prompt: args.prompt, profile, model: args.model };
        let started = match agent.start_child(request).await {
            Ok(view) => view,
            Err(e) => return Ok(ToolOutput::error(format!("the subagent was not started: {e:#}"))),
        };
        if args.background == Some(false) && !started.status.ended() {
            let waited = wait_for(&agent, &ctx.session_id, &started.id, self.access.run_wait(), &ctx.cancel).await;
            if let Some((view, end)) = waited.or_else(|| agent.child_of(&ctx.session_id, &started.id).and_then(|(view, end, _)| end.map(|end| (view, end)))) {
                return Ok(ToolOutput::ok(children::render(&view, Some(&end), MAX_OUTPUT_CHARS)));
            }
            // The time is up, or the turn was cancelled: the subagent goes on.
            let note = if ctx.cancel.is_cancelled() { String::new() } else {
                format!(" It is still running in the background: its answer comes as a message that starts with [Agent], or call agent_wait with its id.")
            };
            return Ok(ToolOutput::json(&json!({"agentId": started.id, "status": started.status.label(), "title": started.title, "profile": started.profile, "note": note.trim()})));
        }
        Ok(ToolOutput::json(&json!({"agentId": started.id, "status": started.status.label(), "title": started.title, "profile": started.profile})))
    }
}

pub struct AgentWait {
    contract: ToolContract,
    access: AgentAccess,
}

impl AgentWait {
    pub fn new(access: AgentAccess) -> Self {
        // `timeout_sec` is at most 120 (tools.md section 21).
        let mut contract = ToolContract::read_only("agent_wait", WAIT_DESCRIPTION, json!({"type":"object","properties":{
            "ids":{"type":"array","maxItems":8,"items":{"type":"string"},"description":"agent ids (from agent_run); none given: every subagent of this session"},
            "timeout_sec":{"type":"integer","description":"wait up to this many seconds; default 60, at most 120"}
        }}));
        contract.default_timeout_ms = children::RUN_WAIT.as_millis() as u64 + 5_000;
        contract.max_timeout_ms = contract.default_timeout_ms;
        Self { contract, access }
    }
}

/// `ids` and `timeout_sec`, as tools.md names them (snake_case, unlike the settings).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    #[serde(default)]
    ids: Option<Vec<String>>,
    #[serde(default)]
    timeout_sec: Option<u64>,
}

#[async_trait]
impl Tool for AgentWait {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Ok(args) = serde_json::from_value::<WaitArgs>(input) else {
            return Ok(ToolOutput::error("agent_wait takes ids (an array of agent ids) and timeout_sec"));
        };
        let agent = match self.access.agent() {
            Ok(agent) => agent,
            Err(e) => return Ok(ToolOutput::error(e.to_string())),
        };
        let (session, mut ids) = (ctx.session_id.clone(), Vec::new());
        for id in args.ids.unwrap_or_default() {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if ids.len() > 8 {
            return Ok(ToolOutput::error(format!("agent_wait names {} agents; at most 8 at a time", ids.len())));
        }
        if ids.is_empty() {
            ids = agent.children_of(&session).into_iter().map(|child| child.id).collect();
        }
        if ids.is_empty() {
            return Ok(ToolOutput::ok("this session started no subagents: start one with agent_run"));
        }
        if let Some(unknown) = ids.iter().find(|id| agent.child_of(&session, id).is_none()) {
            return Ok(ToolOutput::error(format!("no subagent of this session has the id '{unknown}': agent_list names them")));
        }
        let states = |ids: &[String]| ids.iter().filter_map(|id| agent.child_of(&session, id).map(|(view, end, _)| (view, end))).collect::<Vec<_>>();
        let deadline = Instant::now() + Duration::from_secs(args.timeout_sec.unwrap_or(60).min(120));
        let mut now = states(&ids);
        while !now.is_empty() && !now.iter().all(|(_, end)| end.is_some()) && Instant::now() < deadline && !ctx.cancel.is_cancelled() {
            tokio::select! {
                _ = tokio::time::sleep(deadline.saturating_duration_since(Instant::now()).min(POLL)) => {}
                _ = ctx.cancel.cancelled() => {}
            }
            now = states(&ids);
        }
        Ok(ToolOutput::ok(now.iter().map(|(view, end)| children::render(view, end.as_ref(), WAIT_OUTPUT_CHARS)).collect::<Vec<_>>().join("\n")))
    }
}

pub struct AgentStop {
    contract: ToolContract,
    access: AgentAccess,
}

impl AgentStop {
    pub fn new(access: AgentAccess) -> Self {
        Self {
            contract: unasking(
                "agent_stop",
                STOP_DESCRIPTION,
                json!({"type":"object","required":["id"],"properties":{"id":{"type":"string","description":"the agent id (from agent_run or agent_list)"}}}),
            ),
            access,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopArgs {
    id: String,
}

#[async_trait]
impl Tool for AgentStop {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let id = input.get("id").and_then(Value::as_str).unwrap_or_default();
        CallAssessment::from_contract(&self.contract, format!("stop subagent {id}"), "agent_stop".into())
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Ok(args) = serde_json::from_value::<StopArgs>(input) else {
            return Ok(ToolOutput::error("agent_stop needs id: the agent id (from agent_run or agent_list)"));
        };
        let agent = match self.access.agent() {
            Ok(agent) => agent,
            Err(e) => return Ok(ToolOutput::error(e.to_string())),
        };
        Ok(match agent.cancel_child(&ctx.session_id, &args.id).await {
            Ok(_) => ToolOutput::ok("stopped"),
            Err(reason) => ToolOutput::error(reason),
        })
    }
}

pub struct AgentList {
    contract: ToolContract,
    access: AgentAccess,
}

impl AgentList {
    pub fn new(access: AgentAccess) -> Self {
        Self { contract: ToolContract::read_only("agent_list", LIST_DESCRIPTION, json!({"type":"object","properties":{}})), access }
    }
}

#[async_trait]
impl Tool for AgentList {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, _input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let agent = match self.access.agent() {
            Ok(agent) => agent,
            Err(e) => return Ok(ToolOutput::error(e.to_string())),
        };
        let children = agent.children_of(&ctx.session_id);
        if children.is_empty() {
            return Ok(ToolOutput::ok("this session started no subagents"));
        }
        // id, profile, state, duration (tools.md section 21).
        let lines: Vec<String> = children
            .iter()
            .enumerate()
            .map(|(i, c)| format!("[{}] {} {} {} ({})", i + 1, c.id, c.profile, c.status.label(), if c.status == ChildStatus::Queued { "waiting for a free slot".to_string() } else { format!("{} s", c.running_for()) }))
            .collect();
        Ok(ToolOutput::ok(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests;
