//! The orchestration (agents.md sections 2 and 3), as methods of `Agent`: start a child
//! (its session, its restriction, its job) under the concurrency limit, follow its turn,
//! start the next in the queue, tell the parent how it ended, and the shared path of a
//! background task's completion (`notify`). Tested with the scripted model in
//! `tests/subagents.rs`.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::super::{Agent, AgentConfig, SendOutcome, TurnHandle, TurnResult};
use super::registry::{Child, Job};
use super::*;
use crate::protocol::{AgentEvent, Message, Role, TurnStatus, Via};
use crate::session::SessionOrigin;
use crate::tool::{CallAssessment, ToolSpec};

/// How long deleting a session still waits for its cancelled turn, so its files go after it.
const END_GRACE: Duration = Duration::from_secs(2);
impl Agent {
    /// At most `max` subagents run at once (`subagents.maxConcurrent`); the rest wait.
    pub async fn set_max_concurrent_children(self: &Arc<Self>, max: usize) {
        self.children.set_max(max.max(1));
        self.pump_children().await;
    }

    /// Where the window of a profile's model is read (`None`: the chat model's is kept).
    pub fn set_model_window(&self, source: ModelWindow) {
        self.children.set_model_window(source);
    }

    /// Restrict the tools of `session_id` to `groups` (agents.md section 2): a call outside
    /// them is refused when it runs. `None` or empty: every enabled group.
    pub fn set_session_tool_groups(&self, session_id: &str, groups: Option<Vec<String>>) {
        let mut restriction = self.restriction(session_id).unwrap_or_default();
        restriction.groups = groups.filter(|groups| !groups.is_empty());
        self.children.restrict(session_id, restriction);
    }

    /// Refuse in `session_id` every call whose assessment is not read-only (`explore`).
    pub fn set_session_read_only(&self, session_id: &str, read_only: bool) {
        let mut restriction = self.restriction(session_id).unwrap_or_default();
        restriction.read_only = read_only;
        self.children.restrict(session_id, restriction);
    }

    pub(crate) fn restriction(&self, session_id: &str) -> Option<Restriction> {
        self.children.restriction(session_id)
    }

    /// The tools a session is offered: the enabled groups, less what its restriction takes.
    pub(crate) fn tools_for(&self, session_id: &str) -> Vec<ToolSpec> {
        let restriction = self.restriction(session_id);
        self.tools
            .specs()
            .into_iter()
            .filter(|t| self.tool_enabled(&t.name) && restriction.as_ref().is_none_or(|r| r.allows(&t.name)))
            .collect()
    }

    pub(crate) fn tool_names_for(&self, session_id: &str) -> String {
        self.tools_for(session_id).iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
    }

    /// The assessment of a call of a subagent: its summary is prefixed with the name of
    /// its job, so its approval card says whose call it is (agents.md section 2). A call
    /// of any other session is returned as it is.
    pub(crate) fn named(&self, session_id: &str, mut assessment: CallAssessment) -> CallAssessment {
        if let Some((_, view, _, _)) = self.children.get(session_id) {
            assessment.summary = format!("subagent \"{}\": {}", view.title, assessment.summary);
        }
        assessment
    }

    /// The refusal of a restricted session, when there is one (read after `tool_enabled`).
    pub(crate) fn child_refusal(&self, session_id: &str, tool: &str, call: &CallAssessment) -> Option<String> {
        self.children.refusal(session_id, tool, call)
    }

    /// A subagent's turn runs with the model and the iteration limit of its job, and its
    /// system prompt ends with the subagent block.
    pub(crate) fn apply_job(&self, session_id: &str, config: &mut AgentConfig) {
        let Some(job) = self.children.job(session_id) else { return };
        if !job.model_ref.trim().is_empty() {
            config.model_ref = job.model_ref;
            if let Some(info) = self.children.model_window().and_then(|read| read(&config.model_ref)) {
                config.context_tokens = info.context_tokens;
                if info.max_output_tokens.is_some() {
                    config.options.max_output_tokens = info.max_output_tokens;
                }
            }
        }
        config.max_iterations = job.max_iterations.max(1);
        config.system_prompt.push_str(&job.block);
    }

    /// Start a subagent of `req.parent` for one job: a child session (origin `subagent`,
    /// the parent in its meta, the description as its title) whose turn runs now or waits
    /// for a free slot. A subagent cannot start subagents.
    pub async fn start_child(self: &Arc<Self>, req: ChildRequest) -> anyhow::Result<ChildView> {
        let description = req.description.trim();
        let prompt = req.prompt.trim();
        if description.is_empty() || description.chars().count() > MAX_DESCRIPTION_CHARS {
            anyhow::bail!("the description of the job is needed: at most {MAX_DESCRIPTION_CHARS} characters");
        }
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
            anyhow::bail!("the prompt of the job is needed: at most {MAX_PROMPT_CHARS} characters");
        }
        let parent = self.store.meta(&req.parent).await.map_err(|_| anyhow::anyhow!("there is no session '{}' to start a subagent of", req.parent))?;
        // The depth limit (agents.md section 2), here and again when a call runs.
        if parent.origin == SessionOrigin::Subagent {
            anyhow::bail!("a subagent cannot start subagents: do the work with your own tools and end with a complete answer");
        }
        // Plan mode researches only, so its subagents do too (agents.md section 2).
        if parent.planning && !req.profile.read_only {
            anyhow::bail!(crate::plan::PLAN_SUBAGENT_DENIED);
        }
        // The model of the child: the one the call named, else the profile's; empty = the chat
        // model. One that is not a ref, or that the providers do not know (where they can be
        // asked), is refused here and not at the first request of the child. The local model
        // passes a profile name here in about half of its calls.
        let model = req.model.as_deref().map(str::trim).filter(|m| !m.is_empty()).unwrap_or_else(|| req.profile.model.trim());
        if !model.is_empty() && !(model.contains("::") && self.children.model_window().is_none_or(|read| read(model).is_some())) {
            anyhow::bail!("model '{model}' is not a model the providers know: pass a model ref (<providerId>::<modelId>), or leave `model` out (the profile's model, else the chat model)");
        }
        let state = self.state(&req.parent).await?;
        let digest = if state.messages.lock().await.is_empty() { String::new() } else { self.store.digest(&req.parent, DIGEST_CHARS).await.unwrap_or_default() };
        let meta = self.store.create_with(description, SessionOrigin::Subagent, Some(req.parent.clone())).await?;
        let id = meta.id;
        // The child of a planning session gets the plan restriction (plan-todo.md section 2):
        // the engine's flag for its own session id, without the prompt block of plan mode.
        self.state(&id).await?;
        self.permissions.set_planning(&id, parent.planning);
        let restriction = Restriction { groups: (!req.profile.tool_groups.is_empty()).then(|| req.profile.tool_groups.clone()), read_only: req.profile.read_only, child: true };
        self.children.restrict(&id, restriction);
        let view = ChildView { id: id.clone(), title: description.into(), profile: req.profile.name.clone(), status: ChildStatus::Queued, started_at: None, ended_at: None };
        let job = Job {
            model_ref: model.to_string(),
            max_iterations: req.profile.max_iterations,
            block: subagent_block(description, &req.profile),
            text: Some(job_text(&digest, prompt)),
        };
        let (run_now, view) = self.children.insert(Child { parent: req.parent.clone(), view, job, started: None, end: watch::channel(None).0 });
        if run_now {
            self.launch_child(&id).await;
        } else {
            self.child_event(&req.parent, &view);
        }
        // A turn that could not start ended the child already.
        Ok(self.children.get(&id).map_or(view, |(_, view, _, _)| view))
    }

    fn child_event(&self, parent: &str, view: &ChildView) {
        let _ = self.events.send(AgentEvent::SubagentUpdated { session_id: parent.into(), child: view.clone(), status: view.status });
    }

    /// Start the turn of a child that has a slot. One that cannot start ends as failed
    /// here; the queue goes on where the caller loops (`pump_children`).
    async fn launch_child(self: &Arc<Self>, id: &str) {
        let Some(text) = self.children.take_text(id) else { return };
        // Said running before the turn exists, so the end of a turn that fails at once
        // (no chat model) can never reach the UI before the state it ends.
        if let Some(view) = self.children.started(id) {
            if let Some((parent, _, _, _)) = self.children.get(id) {
                self.child_event(&parent, &view);
            }
        }
        match self.send(id, text, Via::Agent).await {
            Ok(SendOutcome::Started { handle, .. }) => self.watch_child(id.to_string(), handle),
            // Never queued: a new child's session has no turn.
            Ok(SendOutcome::Queued) => {}
            Err(e) => {
                let error = format!("{e:#}");
                tracing::warn!(child = id, "subagent could not start: {error}");
                if let Some((parent, view, end)) = self.children.finish(id, ChildStatus::Failed, Some(error), String::new()) {
                    self.child_event(&parent, &view);
                    let _ = self.notify(&parent, &render(&view, Some(&end), MAX_OUTPUT_CHARS)).await;
                }
            }
        }
    }

    /// Follow the turn of a started child in its own task. A plain function, not an async
    /// one: the task's type must not be part of the futures that start children (they
    /// would refer to each other, and such a future is never `Send`).
    fn watch_child(self: &Arc<Self>, id: String, handle: TurnHandle) {
        let this = self.clone();
        tokio::spawn(async move { this.child_ended(&id, handle.wait().await).await });
    }

    /// The child's turn ended: start the next in the queue, tell the parent (a cancelled
    /// child tells nobody: it was stopped on purpose), then tell the UI, so that whoever
    /// hears a child ended knows its parent was told.
    async fn child_ended(self: &Arc<Self>, id: &str, result: TurnResult) {
        let answer = self.answer(id).await;
        let (status, output) = match result.status {
            TurnStatus::Completed => (ChildStatus::Done, answer),
            TurnStatus::Failed => (ChildStatus::Failed, if answer.is_empty() { result.text.trim().to_string() } else { answer }),
            TurnStatus::Cancelled => (ChildStatus::Cancelled, String::new()),
        };
        let Some((parent, view, end)) = self.children.finish(id, status, result.error, output) else { return };
        self.pump_children().await;
        if status != ChildStatus::Cancelled {
            let _ = self.notify(&parent, &render(&view, Some(&end), MAX_OUTPUT_CHARS)).await;
        }
        self.child_event(&parent, &view);
    }

    /// The last answer of the session: its last assistant message with text.
    async fn answer(&self, session_id: &str) -> String {
        let messages = self.messages(session_id).await.unwrap_or_default();
        messages.iter().rev().find(|m| m.role == Role::Assistant && !m.content.trim().is_empty()).map(|m| m.content.trim().to_string()).unwrap_or_default()
    }

    async fn pump_children(self: &Arc<Self>) {
        while let Some(next) = self.children.next_queued() {
            self.launch_child(&next).await;
        }
    }

    /// Store `text` in `session_id` as a message of role user with `Via::Agent` (agents.md
    /// section 3): an idle session gets a turn with it, a running one gets it as steering at
    /// its next model request. Stored only, with no turn, in a session nobody is at (a
    /// task, an alert, a Telegram chat), and in an idle one that ran on its own often
    /// enough. Whether a turn started.
    pub async fn notify(self: &Arc<Self>, session_id: &str, text: &str) -> anyhow::Result<bool> {
        let state = self.state(session_id).await?;
        // Two statements: a guard of the session's locks is never held across an await.
        let at_session = interactive(&*state.meta.lock().await);
        let own_turns = auto_turns(&*state.messages.lock().await);
        // Stored only: a session nobody is at, or an idle one that ran on its own often enough.
        if !at_session || (state.is_idle() && own_turns >= MAX_AUTO_TURNS) {
            return self.push_messages(session_id, &state, vec![Message::user(text).with_via(Via::Agent)]).await.map(|_| false);
        }
        // Idle: a new turn. Running: steering at its next model request.
        Ok(matches!(self.send(session_id, text.to_string(), Via::Agent).await?, SendOutcome::Started { .. }))
    }

    /// The children of `parent`, in the order they were started.
    pub fn children_of(&self, parent: &str) -> Vec<ChildView> {
        self.children.views(parent)
    }

    /// A child of `parent` by id: how it is now, how it ended, and a receiver that sees the end.
    pub fn child_of(&self, parent: &str, id: &str) -> Option<(ChildView, Option<End>, watch::Receiver<Option<End>>)> {
        let (owner, view, end, rx) = self.children.get(id)?;
        (owner == parent).then_some((view, end, rx))
    }

    /// `agent_stop`: cancel the running turn of a child of `parent`; a queued child never starts.
    pub async fn cancel_child(&self, parent: &str, id: &str) -> Result<ChildStatus, String> {
        match self.child_of(parent, id) {
            None => Err(format!("no subagent '{id}' belongs to this session")),
            Some((view, _, _)) if view.status == ChildStatus::Queued => {
                self.cancel_waiting(|other| other == id);
                Ok(ChildStatus::Cancelled)
            }
            Some((view, _, _)) if view.status == ChildStatus::Running => {
                self.cancel(id).await;
                Ok(ChildStatus::Cancelled)
            }
            Some((view, _, _)) => Err(format!("the subagent already ended ({})", view.status.label())),
        }
    }

    /// The queued children that `pick` names never start. How many.
    fn cancel_waiting(&self, pick: impl Fn(&str) -> bool) -> usize {
        let mut told = Vec::new();
        for id in self.children.queued().into_iter().filter(|id| pick(id)) {
            if let Some((parent, view, _)) = self.children.finish(&id, ChildStatus::Cancelled, None, String::new()) {
                told.push((parent, view));
            }
        }
        let n = told.len();
        for (parent, view) in &told {
            self.child_event(parent, view);
        }
        n
    }

    /// The kill switch: no child waiting for a slot starts after it.
    pub(crate) fn cancel_queued_children(&self) -> usize {
        self.cancel_waiting(|_| true)
    }

    /// `DELETE /api/sessions/:id` (agents.md section 2): cancels and deletes the children of the session too.
    pub async fn delete_session(self: &Arc<Self>, id: &str) -> anyhow::Result<()> {
        let children: Vec<String> = self.store.list().await?.into_iter().filter(|m| m.parent.as_deref() == Some(id)).map(|m| m.id).collect();
        for child in &children {
            let _ = self.cancel_child(id, child).await;
        }
        self.cancel(id).await;
        for sid in children.iter().map(String::as_str).chain(std::iter::once(id)) {
            let _ = tokio::time::timeout(END_GRACE, self.wait_idle(sid)).await;
            self.forget(sid);
            self.store.delete(sid).await?;
        }
        Ok(())
    }
}
