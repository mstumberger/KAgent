//! Screen watch (screen-watch.md, S48): the owner asks KAgent to keep an eye on the screen for
//! a goal and a time; a local vision model looks at a frame every few seconds and KAgent speaks
//! up when the goal is met. One watch at a time, in memory only, and no frame is ever stored.
//! `look.rs` decides on a frame and reads the reply, `run.rs` is the loop, `vision.rs` the
//! models, `capture.rs` the frame; the daemon's side (the session, the voice, the notifier) is
//! behind `Env`, so the tests run with a fake capture and a scripted model.

pub mod capture;
pub mod env;
pub mod http;
pub mod look;
pub mod run;
pub mod say;
pub mod settings;
pub mod tell;
pub mod tool;
pub mod vision;

#[cfg(test)]
mod gated;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use kagent_core::protocol::AgentEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use capture::{CaptureError, WindowInfo};
pub use settings::ScreenWatchSection;
pub use vision::LocalModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Window,
    Monitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Once,
    Repeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Watching,
    Met,
    Expired,
    Stopped,
    Failed,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Once => "once",
            Mode::Repeat => "repeat",
        }
    }
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Watching => "watching",
            State::Met => "met",
            State::Expired => "expired",
            State::Stopped => "stopped",
            State::Failed => "failed",
        }
    }
}

/// The one watch (screen-watch.md section 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    pub id: String,
    pub session_id: String,
    pub goal: String,
    pub target: Target,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    pub until: String,
    /// The minutes it was asked for.
    pub minutes: u32,
    pub every_sec: u32,
    pub mode: Mode,
    pub state: State,
    pub looks: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_look_at: Option<String>,
    pub last_note: String,
    /// The screen is locked: no frame is taken and no look counts.
    pub paused: bool,
    /// Why it ended, for `failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Watch {
    /// What the watch looks at, for a summary: the window title or "the monitor".
    pub fn what(&self) -> String {
        match (self.target, &self.window_title) {
            (Target::Window, Some(t)) if !t.is_empty() => t.clone(),
            (Target::Window, _) => "the active window".into(),
            (Target::Monitor, _) => "the monitor".into(),
        }
    }
}

/// What a start asks for.
#[derive(Debug, Clone, Default)]
pub struct StartArgs {
    pub session_id: String,
    pub goal: String,
    pub minutes: Option<u32>,
    pub every_sec: Option<u32>,
    pub mode: Option<Mode>,
    pub target: Option<Target>,
}

/// The daemon's side of a watch. The real one is `env.rs`; the tests have a fake.
#[async_trait]
pub trait Env: Send + Sync {
    fn config(&self) -> ScreenWatchSection;
    /// The local vision model, or the refusal that names the page (section 4).
    async fn local_model(&self) -> Result<LocalModel, String>;
    async fn active_window(&self) -> Result<WindowInfo, String>;
    /// The PNG of the target. Never stored by the caller.
    async fn capture(&self, target: Target, window_id: Option<&str>) -> Result<Vec<u8>, CaptureError>;
    async fn locked(&self) -> bool;
    /// Approval cards waiting on the screen: KAgent never captures its own.
    fn cards_open(&self) -> usize;
    /// A turn is thinking on the local model the watch uses.
    fn turn_busy(&self, model: &LocalModel) -> bool;
    /// The raw reply of the local model about this frame (JPEG).
    async fn look_local(&self, model: &LocalModel, jpeg: &[u8], goal: &str) -> Result<String, String>;
    /// The raw reply of the cloud vision model about this frame, or `None` when there is none (then nothing leaves).
    async fn look_cloud(&self, jpeg: &[u8], goal: &str) -> Option<Result<String, String>>;
    /// Tell the owner: the session, the voice, the desktop, Telegram (`tell.rs`).
    async fn tell(&self, watch: &Watch, text: &str);
    /// Tell the session of a watch that a restart ended that it did.
    async fn restarted(&self, watch: &Watch, text: &str);
}

struct Active {
    watch: Watch,
    cancel: CancellationToken,
}

pub struct ScreenWatch {
    pub(crate) env: Arc<dyn Env>,
    events: broadcast::Sender<AgentEvent>,
    cur: Mutex<Option<Active>>,
    last: Mutex<Option<Watch>>,
    /// Where the running watch is noted (its goal and session, never a frame), so a restart can say it ended it.
    marker: Option<std::path::PathBuf>,
    /// What the owner said lately, for the stop phrases (`say.rs`).
    pub(crate) heard: Mutex<String>,
}

/// The file that says a watch was running when the daemon stopped.
const MARKER: &str = "screen-watch.json";

/// The id of a watch: `w_` and four hex digits.
fn new_id() -> String {
    format!("w_{}", &uuid::Uuid::new_v4().simple().to_string()[..4])
}

impl ScreenWatch {
    /// `data`: the daemon's data folder for the marker of a running watch; none in some tests.
    pub fn new(env: Arc<dyn Env>, events: broadcast::Sender<AgentEvent>, data: Option<&std::path::Path>) -> Arc<Self> {
        Arc::new(Self { env, events, cur: Mutex::new(None), last: Mutex::new(None), marker: data.map(|d| d.join(MARKER)), heard: Mutex::new(String::new()) })
    }

    fn write_marker(&self, w: &Watch) {
        if let Some(p) = &self.marker {
            if let Err(e) = std::fs::write(p, serde_json::to_vec(w).unwrap_or_default()) {
                tracing::warn!("screen watch: the marker could not be written: {e}");
            }
        }
    }

    fn clear_marker(&self) {
        if let Some(p) = &self.marker {
            let _ = std::fs::remove_file(p);
        }
    }

    /// At the daemon's start: a marker left behind means a watch was running when it stopped. The session
    /// that asked for it is told, once ("the screen watch ended with the restart").
    pub async fn start_up(&self) {
        let Some(p) = &self.marker else { return };
        let old = std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<Watch>(&b).ok());
        self.clear_marker();
        if let Some(mut w) = old {
            w.state = State::Failed;
            w.reason = Some("the daemon restarted".into());
            self.env.restarted(&w, &format!("The screen watch ended with the restart. (goal: {})", w.goal)).await;
            *self.last.lock().unwrap() = Some(w);
        }
    }

    /// The running watch.
    pub fn current(&self) -> Option<Watch> {
        self.cur.lock().unwrap().as_ref().map(|a| a.watch.clone())
    }

    /// The watch that ended last since the daemon started.
    pub fn last_ended(&self) -> Option<Watch> {
        self.last.lock().unwrap().clone()
    }

    pub(crate) fn emit(&self, watch: &Watch) {
        let _ = self.events.send(AgentEvent::ScreenWatchChanged { watch: serde_json::to_value(watch).unwrap_or(Value::Null) });
    }

    /// Change the running watch and give back how it is now; `None` when none runs (it ended).
    pub(crate) fn update(&self, id: &str, f: impl FnOnce(&mut Watch)) -> Option<Watch> {
        let mut cur = self.cur.lock().unwrap();
        let a = cur.as_mut().filter(|a| a.watch.id == id)?;
        f(&mut a.watch);
        Some(a.watch.clone())
    }

    /// End the watch `id` as `state`: the first caller wins, a second finds it gone. Cancels its loop,
    /// remembers it as the last one and sends the event. `None` when it was not running.
    pub(crate) fn end(&self, id: &str, state: State, last_note: Option<&str>, reason: Option<&str>) -> Option<Watch> {
        let mut a = {
            let mut cur = self.cur.lock().unwrap();
            if cur.as_ref().is_none_or(|a| a.watch.id != id) {
                return None;
            }
            cur.take()?
        };
        a.cancel.cancel();
        self.clear_marker();
        a.watch.state = state;
        a.watch.paused = false;
        if let Some(n) = last_note {
            a.watch.last_note = n.to_string();
        }
        a.watch.reason = reason.map(str::to_string);
        *self.last.lock().unwrap() = Some(a.watch.clone());
        self.emit(&a.watch);
        Some(a.watch)
    }

    /// `stop`, `DELETE /api/screen-watch`, the stop phrase and the kill switch: ends the watch as `stopped`.
    pub fn stop(&self) -> Option<Watch> {
        let id = self.current()?.id;
        self.end(&id, State::Stopped, None, None)
    }

    /// Start the one watch (screen-watch.md section 1). The refusal says what was refused and what to do instead.
    pub async fn start(self: &Arc<Self>, a: StartArgs) -> Result<Watch, String> {
        let cfg = self.env.config();
        if !cfg.enabled {
            return Err("Screen watch is switched off: the owner turns it on in Settings -> Desktop and apps -> Screen watch.".into());
        }
        let goal = a.goal.split_whitespace().collect::<Vec<_>>().join(" ");
        if goal.is_empty() {
            return Err("a screen watch needs a goal: say what to wait for, for example \"the build in the terminal finishes\".".into());
        }
        if goal.chars().count() > 200 {
            return Err("the goal is over 200 characters: say it shorter.".into());
        }
        let minutes = a.minutes.unwrap_or(cfg.default_minutes);
        if !(1..=120).contains(&minutes) {
            return Err(format!("minutes must be between 1 and 120 (a watch lasts two hours at most), not {minutes}: ask for less."));
        }
        let every_sec = a.every_sec.unwrap_or(cfg.every_sec);
        if !(5..=60).contains(&every_sec) {
            return Err(format!("everySec must be between 5 and 60, not {every_sec}."));
        }
        if let Some(w) = self.current() {
            return Err(format!("a screen watch is already running (\"{}\", {} looks so far): stop it first with screen_watch stop, then start the new one.", w.goal, w.looks));
        }
        let model = self.env.local_model().await?;
        let target = a.target.unwrap_or(Target::Window);
        let (window_id, window_title) = match target {
            Target::Monitor => (None, None),
            Target::Window => {
                let w = self.env.active_window().await?;
                if w.own {
                    return Err("the active window is KAgent itself, which is never watched: click the window to watch first, or start the watch with target \"monitor\".".into());
                }
                (Some(w.id), Some(w.title))
            }
        };
        let now = chrono::Local::now();
        let watch = Watch {
            id: new_id(),
            session_id: a.session_id,
            goal,
            target,
            window_id,
            window_title,
            until: (now + chrono::Duration::minutes(i64::from(minutes))).to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            minutes,
            every_sec,
            mode: a.mode.unwrap_or(Mode::Once),
            state: State::Watching,
            looks: 0,
            last_look_at: None,
            last_note: String::new(),
            paused: false,
            reason: None,
        };
        let cancel = CancellationToken::new();
        {
            let mut cur = self.cur.lock().unwrap();
            // Checked again under the lock: two starts at once make one watch.
            if let Some(w) = cur.as_ref() {
                return Err(format!("a screen watch is already running (\"{}\"): stop it first with screen_watch stop, then start the new one.", w.watch.goal));
            }
            *cur = Some(Active { watch: watch.clone(), cancel: cancel.clone() });
        }
        self.write_marker(&watch);
        self.emit(&watch);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(u64::from(minutes) * 60);
        tokio::spawn(run::run(self.clone(), watch.clone(), model, cancel, deadline));
        Ok(watch)
    }
}
