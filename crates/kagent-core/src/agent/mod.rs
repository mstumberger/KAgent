//! The tool-calling loop (port of ZCode `docs/design/v2/loop/tool-calling-loop.md`).
//!
//! Invariants kept from ZCode:
//! - every assistant tool call gets exactly one tool result (synthetic when a call is
//!   denied, cancelled or skipped), so no request ever carries a dangling call;
//! - steering input sent while a turn runs is drained only at a model-request boundary,
//!   never between an assistant tool call and its result;
//! - no hard tool-call cap: the loop ends on a plain answer, cancellation or a
//!   configurable iteration safety valve (small local models sometimes loop).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::model::{ModelClient, ModelDelta, ModelOptions, ModelRequest};
use crate::permission::{
    AllowRule, PermissionBroker, PermissionEngine,
};
use crate::protocol::{AgentEvent, Message, Role, ToolCall, TurnStatus, Usage, Via};
use crate::session::{SessionMeta, SessionStore};
use crate::tool::{PrivilegeMethod, ToolRegistry};
mod calls;
pub mod title;

pub use crate::context::sanitize_history;
use crate::context::{fit_to_budget, history_budget_tokens};

pub const DEFAULT_TITLE: &str = "New session";

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model_ref: String,
    pub system_prompt: String,
    pub max_iterations: usize,
    pub options: ModelOptions,
    pub privilege: PrivilegeMethod,
    /// Model context window in tokens (0 = unknown, no trimming).
    pub context_tokens: usize,
    /// Used for the rest of the turn when the chat model is overloaded or out of quota
    /// (HTTP 429/503); empty = none.
    pub fallback_model_ref: String,
    pub fallback_context_tokens: usize,
    pub fallback_max_output: Option<u64>,
}

/// Provider errors worth retrying on another model: capacity or quota, and a broken
/// key or account (the user still gets an answer, plus a notice naming the cause).
/// Not bad input: the fallback would fail the same way.
pub fn is_capacity_error(e: &str) -> bool {
    let e = e.to_lowercase();
    [
        "503", "429", "overloaded", "high demand", "unavailable", "quota", "rate limit", "resource_exhausted",
        "api key not valid", "valid api key", "invalid api key", "invalid_api_key", "unauthenticated", "permission_denied", "401 unauthorized", "403 forbidden",
    ]
    .iter()
    .any(|m| e.contains(m))
}

pub type ConfigSource = Arc<dyn Fn() -> AgentConfig + Send + Sync>;
pub type RuleSink = Arc<dyn Fn(Vec<AllowRule>) + Send + Sync>;

struct SessionState {
    meta: tokio::sync::Mutex<SessionMeta>,
    messages: tokio::sync::Mutex<Vec<Message>>,
    running: Mutex<Option<CancellationToken>>,
    pending: Mutex<VecDeque<(String, Via)>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum SendOutcome {
    #[serde(rename_all = "camelCase")]
    Started { turn_id: String },
    /// A turn is running; the input will be injected at the next model request.
    Queued,
}

pub struct Agent {
    audit: Option<Arc<crate::audit::AuditLog>>,
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    permissions: Arc<PermissionEngine>,
    broker: Arc<PermissionBroker>,
    store: SessionStore,
    events: broadcast::Sender<AgentEvent>,
    config: ConfigSource,
    on_rules_changed: Option<RuleSink>,
    sessions: Mutex<HashMap<String, Arc<SessionState>>>,
    /// Session titles refined in the background (see `title.rs`).
    refined: title::Refined,
    tool_groups: Mutex<Option<Vec<String>>>,
    services: Option<Arc<crate::tool::ToolServices>>,
}

pub struct AgentDeps {
    /// Change journal + root helper for mutating tools (`None` in tests).
    pub services: Option<Arc<crate::tool::ToolServices>>,
    pub audit: Option<Arc<crate::audit::AuditLog>>,
    pub model: Arc<dyn ModelClient>,
    pub tools: Arc<ToolRegistry>,
    pub permissions: Arc<PermissionEngine>,
    pub broker: Arc<PermissionBroker>,
    pub store: SessionStore,
    pub events: broadcast::Sender<AgentEvent>,
    pub config: ConfigSource,
    pub on_rules_changed: Option<RuleSink>,
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
            sessions: Mutex::new(HashMap::new()),
            refined: Default::default(),
            tool_groups: Mutex::new(None),
            services: deps.services,
        })
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

    pub fn config(&self) -> AgentConfig {
        (self.config)()
    }

    async fn state(&self, session_id: &str) -> anyhow::Result<Arc<SessionState>> {
        if let Some(s) = self.sessions.lock().unwrap().get(session_id) {
            return Ok(s.clone());
        }
        let meta = self.store.meta(session_id).await?;
        let messages = self.store.load_messages(session_id).await?;
        let state = Arc::new(SessionState {
            meta: tokio::sync::Mutex::new(meta),
            messages: tokio::sync::Mutex::new(messages),
            running: Mutex::new(None),
            pending: Mutex::new(VecDeque::new()),
        });
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_insert(state)
            .clone())
    }

    pub async fn messages(&self, session_id: &str) -> anyhow::Result<Vec<Message>> {
        Ok(self.state(session_id).await?.messages.lock().await.clone())
    }

    pub async fn is_running(&self, session_id: &str) -> bool {
        match self.state(session_id).await {
            Ok(s) => s.running.lock().unwrap().is_some(),
            Err(_) => false,
        }
    }

    pub fn forget(&self, session_id: &str) {
        if let Some(s) = self.sessions.lock().unwrap().remove(session_id) {
            if let Some(c) = s.running.lock().unwrap().take() {
                c.cancel();
            }
        }
    }

    pub async fn cancel(&self, session_id: &str) -> bool {
        let Ok(s) = self.state(session_id).await else { return false };
        let token = s.running.lock().unwrap().clone();
        match token {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// Kill switch: cancel every running turn; returns how many.
    pub async fn stop_all(&self) -> usize {
        let sessions: Vec<Arc<SessionState>> = self.sessions.lock().unwrap().values().cloned().collect();
        let mut n = 0;
        for s in sessions {
            // Cancel but leave the token in place: the turn task clears it once it has
            // actually unwound, so no new turn can start on top of a dying one.
            if let Some(t) = s.running.lock().unwrap().as_ref() {
                if !t.is_cancelled() {
                    t.cancel();
                    n += 1;
                }
            }
        }
        n
    }

    /// Restrict which tool groups are advertised to the model (tool budget).
    pub fn set_tool_groups(&self, groups: Option<Vec<String>>) {
        *self.tool_groups.lock().unwrap() = groups;
    }

    /// Whether a tool may run: it must belong to an enabled group (an empty or unset
    /// list means all groups). Hidden tools are also refused, so a model cannot call a
    /// disabled tool it learned about earlier in the conversation.
    pub fn tool_enabled(&self, name: &str) -> bool {
        match self.tool_groups.lock().unwrap().as_ref() {
            Some(groups) if !groups.is_empty() => groups.iter().any(|g| g == crate::tools::group_of(name)),
            _ => true,
        }
    }

    /// Tool specs the model may see (enabled groups only). Used by voice and MCP.
    pub fn advertised_specs(&self) -> Vec<crate::tool::ToolSpec> {
        self.advertised_tools()
    }

    fn advertised_tools(&self) -> Vec<crate::tool::ToolSpec> {
        let specs = self.tools.specs();
        match self.tool_groups.lock().unwrap().as_ref() {
            Some(groups) if !groups.is_empty() => specs
                .into_iter()
                .filter(|t| groups.iter().any(|g| g == crate::tools::group_of(&t.name)))
                .collect(),
            _ => specs,
        }
    }

    /// Start a turn, or queue the input as steering when one is running. Boxed so the
    /// spawned tail of a turn can start the next one without a recursive async fn.
    pub fn send(
        self: &Arc<Self>,
        session_id: &str,
        input: String,
        via: Via,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<SendOutcome>> + Send + '_>> {
        let this = self.clone();
        let sid = session_id.to_string();
        Box::pin(async move { this.send_inner(&sid, input, via).await })
    }

    async fn send_inner(self: Arc<Self>, session_id: &str, input: String, via: Via) -> anyhow::Result<SendOutcome> {
        let state = self.state(session_id).await?;
        let cancel = {
            let mut running = state.running.lock().unwrap();
            if running.is_some() {
                state.pending.lock().unwrap().push_back((input, via));
                return Ok(SendOutcome::Queued);
            }
            let token = CancellationToken::new();
            *running = Some(token.clone());
            token
        };
        let turn_id = uuid::Uuid::new_v4().to_string();
        let this = self.clone();
        let sid = session_id.to_string();
        let tid = turn_id.clone();
        tokio::spawn(async move {
            this.run_turn(&sid, &tid, input, via, state.clone(), cancel).await;
            *state.running.lock().unwrap() = None;
            // Input that arrived after the last model request starts the next turn.
            let next = state.pending.lock().unwrap().pop_front();
            if let Some((text, via)) = next {
                let _ = this.send(&sid, text, via).await;
            }
        });
        Ok(SendOutcome::Started { turn_id })
    }

    /// Append messages from outside the loop (voice transcripts) to the history.
    pub async fn append_external(&self, session_id: &str, messages: Vec<Message>) -> anyhow::Result<()> {
        let state = self.state(session_id).await?;
        self.push_messages(session_id, &state, messages).await
    }

    async fn push_messages(&self, session_id: &str, state: &SessionState, messages: Vec<Message>) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        self.store.append(session_id, &messages).await?;
        state.messages.lock().await.extend(messages.iter().cloned());
        let mut meta = state.meta.lock().await;
        meta.message_count += messages.len();
        meta.updated_at = chrono::Utc::now();
        if let Some(t) = self.refined.lock().unwrap().remove(session_id) {
            meta.title = t;
        }
        if title::is_placeholder(&meta.title) {
            if let Some(first) = messages.iter().find(|m| m.role == Role::User && title::meaningful(&m.content)) {
                if let Some(q) = title::quick_title(&first.content) {
                    meta.title = q;
                    let _ = self.events.send(AgentEvent::SessionUpdated { session_id: session_id.into(), title: meta.title.clone() });
                    self.refine_title(session_id, first.content.clone());
                }
            }
        }
        self.store.save_meta(&meta).await?;
        drop(meta);
        for m in messages {
            let _ = self.events.send(AgentEvent::MessageAppended { session_id: session_id.into(), message: m });
        }
        Ok(())
    }

    async fn run_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        input: String,
        via: Via,
        state: Arc<SessionState>,
        cancel: CancellationToken,
    ) {
        let _ = self.events.send(AgentEvent::TurnStarted {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            input: input.clone(),
            via,
        });
        let mut usage = Usage::default();
        let result = self
            .turn_loop(session_id, turn_id, input, via, &state, &cancel, &mut usage)
            .await;
        let (status, error) = match result {
            Ok(()) if cancel.is_cancelled() => (TurnStatus::Cancelled, None),
            Ok(()) => (TurnStatus::Completed, None),
            Err(_) if cancel.is_cancelled() => (TurnStatus::Cancelled, None),
            Err(e) => (TurnStatus::Failed, Some(format!("{e:#}"))),
        };
        if let Some(err) = &error {
            tracing::warn!(session_id, turn_id, "turn failed: {err}");
        }
        let _ = self.events.send(AgentEvent::TurnFinished {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            status,
            error,
            usage,
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn turn_loop(
        &self,
        session_id: &str,
        turn_id: &str,
        input: String,
        via: Via,
        state: &SessionState,
        cancel: &CancellationToken,
        usage: &mut Usage,
    ) -> anyhow::Result<()> {
        let mut config = (self.config)();
        let mut fell_back = false;
        if config.model_ref.trim().is_empty() {
            anyhow::bail!("no chat model selected - pick one in Settings -> Providers");
        }
        self.push_messages(session_id, state, vec![Message::user(input).with_via(via)]).await?;
        let mut iteration = 0usize;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            // Model-request boundary: drain steering input queued while we worked.
            let steering: Vec<Message> = state
                .pending
                .lock()
                .unwrap()
                .drain(..)
                .map(|(t, v)| Message::user(t).with_via(v))
                .collect();
            self.push_messages(session_id, state, steering).await?;

            iteration += 1;
            if iteration > config.max_iterations.max(1) {
                anyhow::bail!("stopped after {} model requests (max iterations)", config.max_iterations);
            }

            let all_tools = self.advertised_tools();
            let n_all = all_tools.len();
            // The latest user message decides which groups win when not all fit.
            let hint = state.messages.lock().await.iter().rev().find(|m| m.role == crate::protocol::Role::User).map(|m| m.content.clone()).unwrap_or_default();
            let tools = crate::context::budget_tools(all_tools, config.context_tokens, 0.35, &hint);
            if tools.len() < n_all && iteration == 1 {
                tracing::info!(session_id, kept = tools.len(), of = n_all, "small context window: advertising a reduced tool set");
            }
            let tools_chars: usize = tools.iter().map(|t| t.description.len() + t.input_schema.to_string().len()).sum();
            let budget = history_budget_tokens(
                config.context_tokens,
                config.options.max_output_tokens.unwrap_or(4_096) as usize,
                config.system_prompt.len(),
                tools_chars,
            );
            let (messages, dropped) = fit_to_budget(sanitize_history(&state.messages.lock().await), budget);
            if dropped > 0 {
                tracing::info!(session_id, dropped, "trimmed history to fit the context window");
            }
            let request = ModelRequest {
                model_ref: config.model_ref.clone(),
                system: config.system_prompt.clone(),
                messages,
                tools,
                options: config.options.clone(),
            };
            let (tx, mut rx) = mpsc::unbounded_channel::<ModelDelta>();
            let events = self.events.clone();
            let (sid, tid) = (session_id.to_string(), turn_id.to_string());
            let forward = tokio::spawn(async move {
                while let Some(d) = rx.recv().await {
                    let ev = match d {
                        ModelDelta::Text(text) => AgentEvent::TextDelta { session_id: sid.clone(), turn_id: tid.clone(), text },
                        ModelDelta::Reasoning(text) => {
                            AgentEvent::ReasoningDelta { session_id: sid.clone(), turn_id: tid.clone(), text }
                        }
                    };
                    let _ = events.send(ev);
                }
            });
            let response = self.model.complete(request, tx, cancel.clone()).await;
            let _ = forward.await;
            let response = match response {
                Err(e) if !fell_back && !config.fallback_model_ref.is_empty() && config.fallback_model_ref != config.model_ref && is_capacity_error(&format!("{e:#}")) => {
                    tracing::warn!(session_id, "chat model {} unavailable ({e:#}); falling back to {}", config.model_ref, config.fallback_model_ref);
                    let _ = self.events.send(AgentEvent::Notice {
                        level: "warn".into(),
                        text: if format!("{e:#}").to_lowercase().contains("key") {
                            format!("{} rejected the API key (Providers) - answering with {}", config.model_ref, config.fallback_model_ref)
                        } else {
                            format!("{} is busy - answering with {}", config.model_ref, config.fallback_model_ref)
                        },
                    });
                    config.model_ref = config.fallback_model_ref.clone();
                    config.context_tokens = config.fallback_context_tokens;
                    config.options.max_output_tokens = config.fallback_max_output;
                    fell_back = true;
                    iteration -= 1;
                    continue;
                }
                r => r?,
            };
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;

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
            let mut assistant = Message::assistant(response.text, calls.clone()).with_via(via);
            assistant.provider_state = response.provider_state;
            self.push_messages(session_id, state, vec![assistant]).await?;

            if calls.is_empty() {
                if state.pending.lock().unwrap().is_empty() || cancel.is_cancelled() {
                    return Ok(());
                }
                continue;
            }
            let results = self.execute_calls(session_id, turn_id, &calls, cancel, config.privilege).await;
            self.push_messages(session_id, state, results).await?;
        }
    }

    fn tool_names(&self) -> String {
        self.advertised_tools().iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
    }
}

