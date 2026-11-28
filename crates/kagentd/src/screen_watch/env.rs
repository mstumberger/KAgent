//! The daemon's side of a watch: the real `Env` (screen-watch.md sections 2 to 5). Each method is a
//! call into a module that already exists; the rules live in `look.rs`, `run.rs` and `vision.rs`.

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use kagent_core::agent::Agent;
use kagent_core::protocol::Message;
use kagent_providers::ProviderStore;

use super::capture::{self, CaptureError, WindowInfo};
use super::tell::Deliver;
use super::vision::{self, LocalModel};
use super::{Env, ScreenWatchSection, Target, Watch};
use crate::local_models::{ollama, LocalModels};
use crate::notify::Notification;
use crate::settings::Settings;

pub struct RealEnv {
    pub providers: Arc<ProviderStore>,
    pub settings: Arc<RwLock<Settings>>,
    pub local_models: Arc<LocalModels>,
    pub agent: Arc<Agent>,
    pub notifier: Arc<crate::notify::Notifier>,
}

#[async_trait]
impl Deliver for RealEnv {
    async fn session_message(&self, session_id: &str, message: Message) -> Result<(), String> {
        self.agent.append_external(session_id, vec![message]).await.map_err(|e| format!("{e:#}"))
    }
    fn say(&self, session_id: &str, text: &str) -> bool {
        super::say::speak(session_id, text)
    }
    async fn notify(&self, notification: Notification) {
        let _ = self.notifier.send(notification).await;
    }
    fn telegram_on(&self) -> bool {
        self.settings.read().unwrap().screen_watch_config().telegram
    }
}

#[async_trait]
impl Env for RealEnv {
    fn config(&self) -> ScreenWatchSection {
        self.settings.read().unwrap().screen_watch_config()
    }

    async fn local_model(&self) -> Result<LocalModel, String> {
        vision::local_model(&self.providers, &self.settings, &self.local_models).await
    }

    async fn active_window(&self) -> Result<WindowInfo, String> {
        capture::active_window().await
    }

    async fn capture(&self, target: Target, window_id: Option<&str>) -> Result<Vec<u8>, CaptureError> {
        capture::grab(target, window_id).await
    }

    async fn locked(&self) -> bool {
        crate::screen::live::is_locked().await
    }

    fn cards_open(&self) -> usize {
        self.agent.broker().pending().len()
    }

    /// A turn runs, and the model that answers it (the chat or the fast model) is the one the watch looks with.
    fn turn_busy(&self, model: &LocalModel) -> bool {
        if !self.agent.any_running() {
            return false;
        }
        let refs = {
            let s = self.settings.read().unwrap();
            [s.chat_model.clone(), s.fast_model.clone()]
        };
        refs.iter().filter_map(|r| self.providers.resolve(r).ok()).any(|r| model.same_as(&ollama::clean_base(&r.provider.api.base_url), &r.model.id))
    }

    async fn look_local(&self, model: &LocalModel, jpeg: &[u8], goal: &str) -> Result<String, String> {
        vision::ask_local(model, jpeg, goal).await
    }

    /// The cloud vision model of the `screen` tool, when it is not a local one: a frame is never sent to a server on this network by this path.
    async fn look_cloud(&self, jpeg: &[u8], goal: &str) -> Option<Result<String, String>> {
        let r = {
            let s = self.settings.read().unwrap();
            crate::screen::cloud_vision_model(&self.providers, &s)?
        };
        if vision::is_local_url(&r.provider.api.base_url) {
            return None;
        }
        Some(vision::ask_cloud(&r.provider.api.base_url, &r.provider.api_key, &r.model.id, jpeg, goal).await)
    }

    async fn tell(&self, watch: &Watch, text: &str) {
        tracing::info!("screen watch {} ({}): {text}", watch.id, watch.state.name());
        super::tell::tell(self, watch, text).await;
    }

    async fn restarted(&self, watch: &Watch, text: &str) {
        super::tell::restarted(self, watch, text).await;
    }
}
