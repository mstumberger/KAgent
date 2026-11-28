//! The wiring of the subagents into the daemon (agents.md sections 2 and 4): the tools
//! reach the agent and the profiles; the window of a profile's model is read from the
//! providers; background tasks write under `<data>/task-out` and tell their session; the
//! concurrency limit follows `settings.subagents`.

use std::path::Path;
use std::sync::Arc;

use kagent_core::agent::children::ModelInfo;
use kagent_core::agent::Agent;
use kagent_core::protocol::AgentEvent;
use kagent_core::tools::agents::AgentAccess;
use kagent_core::tools::background::Background;
use kagent_providers::ProviderStore;
use tokio::sync::broadcast;

use crate::subagents::ProfileHub;

pub struct Subagents {
    pub profiles: Arc<ProfileHub>,
    pub access: AgentAccess,
}

impl Subagents {
    /// Once the agent exists: the tools reach it and the profiles (the folder is read
    /// before each `agent_run`), and the rest is set up. `max_concurrent` is
    /// `settings.subagents.maxConcurrent`.
    pub async fn start(
        agent: Arc<Agent>,
        access: AgentAccess,
        profiles: Arc<ProfileHub>,
        providers: Arc<ProviderStore>,
        task_out: impl AsRef<Path>,
        max_concurrent: u32,
        events: broadcast::Sender<AgentEvent>,
    ) -> Arc<Self> {
        profiles.reload();
        access.attach(&agent);
        let folder = profiles.clone();
        access.set_profile_source(Arc::new(move || folder.profiles()));
        // The window and the output limit of a profile's model: a child's prompt, history
        // and tools are budgeted with them (an unknown ref keeps the chat model's).
        let known = providers.clone();
        agent.set_model_window(Arc::new(move |model_ref: &str| {
            providers.resolve(model_ref).ok().map(|r| ModelInfo { context_tokens: r.config.context_window as usize, max_output_tokens: Some(r.config.max_output_tokens), reasoning_levels: r.config.reasoning_levels })
        }));
        // What a conversation's choice is asked of (picker.md section 4.3).
        agent.set_usable_models(Arc::new(move |model_ref: &str| {
            known.usable(model_ref).map(|r| ModelInfo { context_tokens: r.config.context_window as usize, max_output_tokens: Some(r.config.max_output_tokens), reasoning_levels: r.config.reasoning_levels }).ok_or_else(|| known.unusable(model_ref).unwrap_or("unknown model"))
        }));
        // A background task that ends tells the session that started it, as a subagent's
        // parent is told (agents.md section 4).
        let tell = agent.clone();
        Background::shared().configure(task_out.as_ref(), Arc::new(move |session: String, text: String| {
            let tell = tell.clone();
            tokio::spawn(async move {
                let _ = tell.notify(&session, &text).await;
            });
        }));
        // A change of status of a task goes to the windows (work-windows.md section 4.1), before the notification above.
        Background::shared().set_event_sink(Arc::new(move |id, session, status| {
            let _ = events.send(AgentEvent::BackgroundTask { task_id: id.to_string(), session_id: session.to_string(), status: status.label().to_string() });
        }));
        agent.set_max_concurrent_children(max_concurrent as usize).await;
        Arc::new(Self { profiles, access })
    }
}
