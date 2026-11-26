//! The wiring of the subagents into the daemon (agents.md sections 2 and 4): the tools
//! reach the agent and the profiles; the window of a profile's model is read from the
//! providers; background tasks write under `<data>/task-out` and tell their session; the
//! concurrency limit follows `settings.subagents`.

use std::path::Path;
use std::sync::Arc;

use kagent_core::agent::children::ModelInfo;
use kagent_core::agent::Agent;
use kagent_core::tools::agents::AgentAccess;
use kagent_core::tools::background::Background;
use kagent_providers::ProviderStore;

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
    ) -> Arc<Self> {
        profiles.reload();
        access.attach(&agent);
        let folder = profiles.clone();
        access.set_profile_source(Arc::new(move || folder.profiles()));
        // The window and the output limit of a profile's model: a child's prompt, history
        // and tools are budgeted with them (an unknown ref keeps the chat model's).
        agent.set_model_window(Arc::new(move |model_ref: &str| {
            providers.resolve(model_ref).ok().map(|r| ModelInfo { context_tokens: r.config.context_window as usize, max_output_tokens: Some(r.config.max_output_tokens) })
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
        agent.set_max_concurrent_children(max_concurrent as usize).await;
        Arc::new(Self { profiles, access })
    }
}
