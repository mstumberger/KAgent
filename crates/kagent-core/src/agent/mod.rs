//! The tool-calling loop (port of ZCode `docs/design/v2/loop/tool-calling-loop.md`).
//!
//! Invariants kept from ZCode:
//! - every assistant tool call gets exactly one tool result (synthetic when a call is
//!   denied, cancelled or skipped), so no request ever carries a dangling call;
//! - steering input sent while a turn runs is drained only at a model-request boundary,
//!   never between an assistant tool call and its result;
//! - no hard tool-call cap: the loop ends on a plain answer, cancellation or a
//! configurable iteration safety valve (small local models sometimes loop).

use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::model::{ModelClient, ModelDelta, ModelRequest};
use crate::permission::{PermissionBroker, PermissionEngine};
use crate::protocol::{AgentEvent, Message, ToolCall, Usage, Via};
use crate::session::SessionStore;
use crate::tool::ToolRegistry;
mod calls;
pub mod children;
mod config;
pub mod hooks;
pub mod title;
pub(crate) mod state;
mod picker;
mod spawn;
mod turn;

pub use crate::context::sanitize_history;
use crate::compact::Step;
use crate::context::history_budget_tokens;
use state::{SessionState, Taken, TurnInput};
pub use config::{AgentConfig, AgentDeps, CompactionSource, ConfigSource, RuleSink, SendOutcome, TurnContext, TurnOpts};
pub use hooks::{AgentHooks, HookEvent, HookInput, HookVerdict, ToolHooks};
pub use turn::{TurnHandle, TurnResult};
pub use crate::compact::is_capacity_error;
pub const DEFAULT_TITLE: &str = "New session";
/// The sessions in memory, by id; shared with the background title task (`title.rs`).
pub(crate) type Sessions = Arc<Mutex<std::collections::HashMap<String, Arc<SessionState>>>>;

/// Error of a turn whose loop panicked; the panic itself goes to the log.
const INTERNAL_ERROR: &str = "internal error (see log)";
/// How a turn ends when the model answered with neither text nor a tool call twice
/// (Ollama drops a call it cannot parse): failed, so the owner sees it.
pub const NO_ANSWER: &str = "the model gave no answer (asked twice): try again, or choose another chat model";

pub struct Agent {
    pub(crate) audit: Option<Arc<crate::audit::AuditLog>>,
    pub(crate) model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    permissions: Arc<PermissionEngine>,
    broker: Arc<PermissionBroker>,
    store: SessionStore,
    pub(crate) events: broadcast::Sender<AgentEvent>,
    config: ConfigSource,
    on_rules_changed: Option<RuleSink>,
    sessions: Sessions,
    /// Session titles refined in the background (see `title.rs`).
    refined: title::Refined,
    tool_groups: Mutex<Option<Vec<String>>>,
    services: Option<Arc<crate::tool::ToolServices>>,
    compaction: Mutex<Option<CompactionSource>>,
    /// The subagents of this run and what a restricted session may do (`children.rs`).
    pub(crate) children: children::Children,
    /// The daemon's hooks of the loop, and the context they added (`hooks.rs`).
    pub(crate) hooks: hooks::HookState,
}

impl Agent {
    pub fn new(deps: AgentDeps) -> Arc<Self> {
        Arc::new(Self {
            audit: deps.audit,
            model: deps.model,
            tools: deps.tools,
            permissions: deps.permissions,
            broker: deps.broker,
            store: deps.store,
            events: deps.events,
            config: deps.config,
            on_rules_changed: deps.on_rules_changed,
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            refined: Default::default(),
            tool_groups: Mutex::new(None),
            services: deps.services,
            compaction: Mutex::new(None),
            children: Default::default(),
            hooks: hooks::HookState::new(deps.hooks),
        })
    }

    /// Where `settings.compaction` and the fast model are read before each request. Unset: the defaults, no fast model.
    pub fn set_compaction_source(&self, source: CompactionSource) {
        *self.compaction.lock().unwrap() = Some(source);
    }

    pub(crate) fn compaction_config(&self) -> crate::compact::CompactionConfig {
        let source = self.compaction.lock().unwrap().clone();
        source.map_or_else(Default::default, |read| read())
    }

    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }
    pub fn store(&self) -> &SessionStore {
        &self.store
    }
    pub fn broker(&self) -> &Arc<PermissionBroker> {
        &self.broker
    }
    pub fn permissions(&self) -> &Arc<PermissionEngine> {
        &self.permissions
    }
    pub fn services(&self) -> Option<&Arc<crate::tool::ToolServices>> {
        self.services.as_ref()
    }

    /// The configuration for no turn in particular (live voice, compaction): the source
    /// is asked with an empty context, so it sees no user message.
    pub fn config(&self) -> AgentConfig {
        (self.config)(TurnContext::default())
    }

    /// Start a turn, or queue the input as steering when one is running.
    pub fn send(
        self: &Arc<Self>,
        session_id: &str,
        input: String,
        via: Via,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SendOutcome>> + Send + '_>> {
        self.send_with(session_id, input, via, TurnOpts::default())
    }

    /// `send` with options of this one turn (`TurnOpts`).
    pub fn send_with(
        self: &Arc<Self>,
        session_id: &str,
        input: String,
        via: Via,
        opts: TurnOpts,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SendOutcome>> + Send + '_>> {
        let this = self.clone();
        let sid = session_id.to_string();
        let plan = opts.plan;
        let input = TurnInput { text: input, via, system_suffix: opts.system_suffix };
        Box::pin(async move {
            // Before the turn starts (plan-todo.md section 2), so its first call is restricted too.
            if plan {
                this.set_planning(&sid, true).await?;
            }
            this.start(&sid, input).await
        })
    }

    async fn start(self: Arc<Self>, session_id: &str, input: TurnInput) -> anyhow::Result<SendOutcome> {
        let state = self.state(session_id).await?;
        match state.take(&input) {
            Taken::Turn(cancel) => {
                let turn_id = uuid::Uuid::new_v4().to_string();
                let handle = self.spawn_turn(session_id, turn_id.clone(), input, state, cancel);
                Ok(SendOutcome::Started { turn_id, handle })
            }
            Taken::Queued => Ok(SendOutcome::Queued),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn turn_loop(
        &self,
        session_id: &str,
        turn_id: &str,
        input: TurnInput,
        state: &SessionState,
        cancel: &CancellationToken,
        usage: &mut Usage,
        text: Arc<Mutex<String>>,
    ) -> anyhow::Result<()> {
        // Before the message is stored: for the prompt, this turn's message is the latest user message.
        let turn = TurnContext { session_id: session_id.to_string(), user_text: input.text.clone() };
        let mut config = (self.config)(turn);
        // A subagent's turn: the model, the iteration limit and the block of its job.
        self.apply_job(session_id, &mut config);
        // A conversation's own model and effort (picker.md section 4.2).
        self.apply_choice(session_id, state, &mut config).await;
        if let Some(block) = &input.system_suffix {
            config.system_prompt.push_str(block);
        }
        let (mut fell_back, mut recompacted, mut asked_again) = (false, false, false);
        if config.model_ref.trim().is_empty() {
            anyhow::bail!("no chat model selected - pick one in Settings -> Providers");
        }
        // Hooks (hooks.md section 4): `SessionStart` before the first turn of a session, then
        // `UserPromptSubmit`. What they add, and what a `PostToolUse` hook adds, is a stored
        // user message in the next model request: at most `hook_budget` characters a turn.
        self.turn_hooks(session_id, state, &input.text, input.via).await;
        let mut hook_budget = hooks::CONTEXT_CHARS_PER_TURN;
        self.push_messages(session_id, state, vec![Message::user(input.text).with_via(input.via)]).await?;
        let mut iteration = 0usize;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            // Model-request boundary: drain steering input queued while we worked.
            let mut late = String::new();
            let steering: Vec<Message> = state
                .pending
                .lock()
                .unwrap()
                .drain(..)
                .map(|queued| {
                    // A queued message that names a skill adds its block from this request on.
                    late.push_str(queued.system_suffix.as_deref().unwrap_or_default());
                    Message::user(queued.text).with_via(queued.via)
                })
                .collect();
            self.push_messages(session_id, state, steering).await?;
            config.system_prompt.push_str(&late);
            self.push_messages(session_id, state, self.hook_messages(session_id, &mut hook_budget)).await?;

            iteration += 1;
            if iteration > config.max_iterations.max(1) {
                anyhow::bail!("stopped after {} model requests (max iterations)", config.max_iterations);
            }

            // The tools of this session: a subagent is offered only those of its profile.
            let all_tools = self.tools_for(session_id);
            let n_all = all_tools.len();
            // The latest user message decides which groups win when not all fit.
            let hint = state.messages.lock().await.iter().rev().find(|m| m.role == crate::protocol::Role::User).map(|m| m.content.clone()).unwrap_or_default();
            let tools = crate::context::budget_tools(all_tools, config.context_tokens, 0.35, &hint);
            if tools.len() < n_all && iteration == 1 {
                tracing::info!(session_id, kept = tools.len(), of = n_all, "small context window: advertising a reduced tool set");
            }
            let tools_chars: usize = tools.iter().map(|t| t.description.len() + t.input_schema.to_string().len()).sum();
            // Plan mode (plan-todo.md section 2), read before every request: the session
            // enters and leaves it while a turn runs. Both blocks are built here and never
            // kept in `config`, so a compaction or a fallback cannot lose them.
            let (planning, approved_plan) = self.current_plan(state).await;
            let mut system = config.system_prompt.clone();
            if planning {
                system.push_str(crate::plan::PLAN_PROMPT);
            }
            if let Some(plan) = approved_plan.as_ref() {
                system.push_str(&crate::plan::approved_prompt(&plan.text));
            }
            let budget = history_budget_tokens(
                config.context_tokens,
                config.options.max_output_tokens.unwrap_or(4_096) as usize,
                system.len(),
                tools_chars,
            );
            // Summarized or trimmed first (compaction.md sections 1 and 4).
            let (messages, _dropped) = self.request_history(session_id, state, &config, budget, cancel).await?;
            let request = ModelRequest {
                model_ref: config.model_ref.clone(),
                system,
                messages,
                tools,
                options: config.options.clone(),
            };
            let (tx, mut rx) = mpsc::unbounded_channel::<ModelDelta>();
            let events = self.events.clone();
            let (sid, tid) = (session_id.to_string(), turn_id.to_string());
            // Deltas go to the UI and, the text ones, to the turn's result.
            let streamed = text.clone();
            let forward = tokio::spawn(async move {
                while let Some(d) = rx.recv().await {
                    let ev = match d {
                        ModelDelta::Text(chunk) => {
                            streamed.lock().unwrap().push_str(&chunk);
                            AgentEvent::TextDelta { session_id: sid.clone(), turn_id: tid.clone(), text: chunk }
                        }
                        ModelDelta::Reasoning(chunk) => {
                            AgentEvent::ReasoningDelta { session_id: sid.clone(), turn_id: tid.clone(), text: chunk }
                        }
                    };
                    let _ = events.send(ev);
                }
            });
            let response = self.model.complete(request, tx, cancel.clone()).await;
            let _ = forward.await;
            // A full or overloaded model (compaction.md section 1): compacted once and retried, then the fallback.
            if let Err(e) = &response {
                match self.on_capacity_error(session_id, state, &mut config, e, recompacted, fell_back, cancel).await {
                    Step::Retry => {
                        recompacted = true;
                        iteration -= 1;
                        continue;
                    }
                    Step::Fallback => {
                        fell_back = true;
                        iteration -= 1;
                        continue;
                    }
                    Step::Fail => {}
                }
            }
            let response = response?;
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;
            // The provider's count of the prompt calibrates the estimate (compaction.md section 6).
            state.last_input_tokens.store(response.usage.input_tokens, std::sync::atomic::Ordering::Relaxed);

            let calls: Vec<ToolCall> = response
                .tool_calls
                .into_iter()
                .enumerate()
                .map(|(i, mut c)| {
                    if c.id.is_empty() {
                        c.id = format!("call_{}_{i}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
                    }
                    c
                })
                .collect();
            // Neither text nor a call: asked once more, then the turn fails with NO_ANSWER.
            if calls.is_empty() && response.text.trim().is_empty() && !cancel.is_cancelled() {
                tracing::warn!(session_id, turn_id, retry = !asked_again, "the model gave no answer");
                anyhow::ensure!(!asked_again, NO_ANSWER);
                asked_again = true;
                iteration -= 1;
                continue;
            }
            let mut assistant = Message::assistant(response.text, calls.clone()).with_via(input.via);
            assistant.provider_state = response.provider_state;
            self.push_messages(session_id, state, vec![assistant]).await?;

            if calls.is_empty() {
                if state.pending.lock().unwrap().is_empty() || cancel.is_cancelled() {
                    return Ok(());
                }
                continue;
            }
            let results = self.execute_calls_from(session_id, turn_id, &calls, cancel, config.privilege, crate::permission::Origin::of_via(input.via)).await;
            self.push_messages(session_id, state, results).await?;
        }
    }
}
