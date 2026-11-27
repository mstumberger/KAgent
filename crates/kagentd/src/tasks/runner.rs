//! The runner (automations.md section 3). One loop every 15 s claims the tasks that are
//! due; each claimed task runs in its own task. A reminder is a notification; an agent
//! task is a turn the runner waits for: up to `timeoutMin`, until a cancel, and, with
//! the policy `deny`, answering every approval card of the turn at once.
//!
//! The runner reaches the agent through `TaskRunner` and the clock through a function,
//! so its runs are tested with a fake and never wait for a wall-clock minute.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Local};
use kagent_core::agent::Agent;
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{AgentEvent, Role, TurnStatus, Via};
use kagent_core::session::SessionOrigin;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::schedule;
use super::store::{chars, RunStatus, SessionPolicy, Task, TaskStore};
use crate::notify::{Channel, Notification, Notifier, Urgency};
use crate::sections::TaskApproval;
use crate::AppState;

#[cfg(test)]
mod tests;

/// The loop looks for due tasks this often.
pub const TICK: Duration = Duration::from_secs(15);

/// What a step that needs approval is refused with, in an unattended run.
pub const NEEDS_APPROVAL: &str = "unattended task: this needs approval on the desktop";

/// How long the runner still waits for the turn to end after cancelling it, so its
/// partial text is kept.
const GRACE: Duration = Duration::from_secs(5);

/// Why a run stopped being waited for.
enum How {
    /// The turn ended.
    Done(TurnEnd),
    /// The run was cancelled (`false`) or ran out of time (`true`); the turn was
    /// cancelled and has ended, or did not within the grace period.
    Stopped(bool, TurnEnd),
}

/// How the turn of a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub status: TurnStatus,
    pub error: Option<String>,
    /// The last answer of the turn.
    pub text: String,
}

impl TurnEnd {
    fn empty() -> Self {
        Self { status: TurnStatus::Cancelled, error: None, text: String::new() }
    }
}

/// What the runner needs from the agent for one run.
#[async_trait]
pub trait TaskRunner: Send + Sync {
    /// The session of a run: a new one, or the task's own (`existing`, made when it does
    /// not exist any more).
    async fn session(&self, policy: SessionPolicy, title: &str, existing: Option<&str>) -> anyhow::Result<String>;
    /// Start the turn of `prompt`.
    async fn send(&self, session: &str, prompt: &str) -> anyhow::Result<()>;
    /// Resolve once the session has no running turn, with how its last turn ended.
    async fn wait_idle(&self, session: &str) -> TurnEnd;
    /// Cancel the running turn; true when one was.
    async fn cancel(&self, session: &str) -> bool;
    /// Refuse a waiting approval card; false when nobody waits for it.
    async fn deny(&self, request_id: &str, reason: &str) -> bool;
    /// The events of the daemon, where the cards of the run arrive.
    fn events(&self) -> broadcast::Receiver<AgentEvent>;
}

/// The agent of the daemon.
pub struct AgentRunner {
    pub agent: Arc<Agent>,
    pub events: broadcast::Sender<AgentEvent>,
}

#[async_trait]
impl TaskRunner for AgentRunner {
    async fn session(&self, policy: SessionPolicy, title: &str, existing: Option<&str>) -> anyhow::Result<String> {
        match (policy, existing) {
            (SessionPolicy::Reuse, Some(id)) if self.agent.store().meta(id).await.is_ok() => Ok(id.to_string()),
            _ => Ok(self.agent.store().create_with(&format!("Task: {title}"), SessionOrigin::Task, None).await?.id),
        }
    }

    async fn send(&self, session: &str, prompt: &str) -> anyhow::Result<()> {
        self.agent.send(session, prompt.to_string(), Via::Task).await.map(|_| ())
    }

    async fn wait_idle(&self, session: &str) -> TurnEnd {
        let result = self.agent.wait_idle(session).await;
        let messages = self.agent.messages(session).await.unwrap_or_default();
        let last = messages.iter().rev().find(|m| m.role == Role::Assistant && !m.content.trim().is_empty()).map(|m| m.content.trim().to_string()).unwrap_or_default();
        // The last assistant message is this turn's answer when it ends the text the
        // turn streamed; else the turn's own text (a reused session whose turn said
        // nothing must not show the answer of the run before).
        let streamed = result.text.trim().to_string();
        let text = if streamed.is_empty() || last.is_empty() {
            streamed
        } else if streamed.ends_with(&last) {
            last
        } else {
            streamed
        };
        TurnEnd { status: result.status, error: result.error, text }
    }

    async fn cancel(&self, session: &str) -> bool {
        self.agent.cancel(session).await
    }

    async fn deny(&self, request_id: &str, reason: &str) -> bool {
        self.agent.broker().resolve(request_id, ApprovalDecision::Deny { reason: Some(reason.to_string()) }, kagent_core::permission::ApprovalVia::Rule)
    }

    fn events(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
}

/// The state of the desktop sink in the log of a delivery.
fn desktop_sink() -> &'static str {
    if std::env::var_os("KAGENT_NO_NOTIFY").is_some() { "the desktop sink is off (KAGENT_NO_NOTIFY)" } else { "the desktop sink is on" }
}

/// Where a notification goes.
#[async_trait]
pub trait Deliverer: Send + Sync {
    async fn deliver(&self, notification: Notification);
}

#[async_trait]
impl Deliverer for Notifier {
    async fn deliver(&self, notification: Notification) {
        let _ = self.send(notification).await;
    }
}

/// The dependencies of a run: the store, the agent, notifications and the clock.
pub struct Runner {
    pub store: Arc<TaskStore>,
    pub runner: Arc<dyn TaskRunner>,
    pub deliver: Arc<dyn Deliverer>,
    pub now: Arc<dyn Fn() -> DateTime<Local> + Send + Sync>,
}

/// The prompt of an unattended run.
fn prompt(task: &Task) -> String {
    format!("Scheduled task '{}' runs unattended. Reply with exactly NOTHING if nothing needs the owner's attention.\n{}", task.title, task.what)
}

fn channels_of(deliver: &[String]) -> Vec<Channel> {
    deliver.iter().filter_map(|channel| match channel.as_str() { "desktop" => Some(Channel::Desktop), "telegram" => Some(Channel::Telegram), _ => None }).collect()
}

/// The outcome of a run, before it is recorded.
struct Outcome {
    status: RunStatus,
    result: String,
    session: Option<String>,
}

impl Runner {
    /// One pass: start a run for each due task, at most `max_agent` agent tasks at
    /// once. How many were started.
    pub fn spawn_due(self: &Arc<Self>, max_agent: u32) -> usize {
        let claimed = self.store.claim((self.now)(), max_agent);
        for task in &claimed {
            let (runner, task) = (self.clone(), task.clone());
            let limit = Duration::from_secs(u64::from(task.timeout_min) * 60);
            tracing::info!(task = %task.id, "running \"{}\" ({})", task.title, task.schedule_text());
            tokio::spawn(async move { runner.run(&task, limit).await });
        }
        claimed.len()
    }

    /// One claimed task, from its notification or its turn to its record. `limit` is
    /// how long the turn may take.
    pub async fn run(&self, task: &Task, limit: Duration) {
        let token = self.store.cancel_token(&task.id);
        let started = tokio::time::Instant::now();
        let outcome = if token.is_cancelled() {
            Outcome { status: RunStatus::Cancelled, result: "cancelled".into(), session: None }
        } else if task.kind == "agent" {
            self.agent(task, limit, &token).await
        } else {
            self.remind(task).await
        };
        self.store.finish(&task.id, outcome.status, &outcome.result, outcome.session.as_deref(), started.elapsed().as_secs(), (self.now)());
    }

    async fn remind(&self, task: &Task) -> Outcome {
        let channels = channels_of(&task.deliver);
        let channels_named = task.deliver.join(",");
        let sink = desktop_sink();
        if channels.is_empty() {
            tracing::info!(task = %task.id, channels = %channels_named, "reminder recorded: it names no channel that shows it");
        } else {
            tracing::info!(task = %task.id, channels = %channels_named, "{sink}: reminder delivered");
            self.deliver.deliver(Notification { title: format!("Reminder: {}", task.title), body: task.what.clone(), urgency: Urgency::Normal, channels }).await;
        }
        Outcome { status: RunStatus::Ok, result: format!("reminded: {}", chars(&task.what, 200)), session: None }
    }

    async fn agent(&self, task: &Task, limit: Duration, token: &CancellationToken) -> Outcome {
        let session = match self.runner.session(task.session, &format!("Task: {}", task.title), task.session_id.as_deref()).await {
            Ok(session) => session,
            Err(e) => return Outcome::failed(format!("could not create a session: {e:#}"), None),
        };
        self.store.attach_session(&task.id, &session);
        let mut cards = self.runner.events();
        if let Err(e) = self.runner.send(&session, &prompt(task)).await {
            return Outcome::failed(format!("could not start: {e:#}"), Some(session));
        }
        let mut skipped = Vec::new();
        let mut patience = Box::pin(tokio::time::sleep(limit));
        let how = loop {
            tokio::select! {
                end = self.runner.wait_idle(&session) => break How::Done(end),
                _ = token.cancelled() => {
                    let (timed_out, end) = self.stop(&session, false).await;
                    break How::Stopped(timed_out, end);
                }
                _ = &mut patience => {
                    let (timed_out, end) = self.stop(&session, true).await;
                    break How::Stopped(timed_out, end);
                }
                event = cards.recv() => match event {
                    Ok(AgentEvent::ApprovalRequested(card)) => {
                        // Policy `deny`: the card is answered at once, and the run says so.
                        if card.session_id == session && task.on_approval == TaskApproval::Deny && self.runner.deny(&card.request_id, NEEDS_APPROVAL).await {
                            skipped.push(format!("{}: {}", card.tool, card.summary));
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        // No more events will come: the turn alone ends the wait.
                        match tokio::time::timeout(limit, self.runner.wait_idle(&session)).await {
                            Ok(end) => break How::Done(end),
                            Err(_) => {
                                let (timed_out, end) = self.stop(&session, true).await;
                                break How::Stopped(timed_out, end);
                            }
                        }
                    }
                },
            }
        };
        let (status, text, error) = match how {
            How::Done(end) => (
                match end.status {
                    TurnStatus::Completed => RunStatus::Ok,
                    TurnStatus::Failed => RunStatus::Failed,
                    TurnStatus::Cancelled => RunStatus::Cancelled,
                },
                end.text,
                end.error,
            ),
            How::Stopped(timed_out, end) => (if timed_out { RunStatus::Timeout } else { RunStatus::Cancelled }, end.text, end.error),
        };
        let text = text.trim().to_string();
        let mut status = status;
        let mut result = if status == RunStatus::Ok && text == "NOTHING" {
            // Nothing to report: no notification, an empty result.
            String::new()
        } else if text.is_empty() {
            match status {
                RunStatus::Ok => {
                    status = RunStatus::Failed;
                    "the agent gave no answer (see the task's session)".to_string()
                }
                RunStatus::Failed => format!("the task failed: {}", error.as_deref().unwrap_or("see the task's session")),
                RunStatus::Timeout => format!("timed out after {} minutes (see the task's session)", task.timeout_min),
                _ => "cancelled".to_string(),
            }
        } else if status == RunStatus::Timeout {
            format!("timed out after {} minutes: {text}", task.timeout_min)
        } else if status == RunStatus::Failed {
            format!("{text}\n\nthe task failed: {}", error.as_deref().unwrap_or("see the task's session"))
        } else {
            text
        };
        if !skipped.is_empty() {
            // The final message lists what was skipped.
            if result.is_empty() {
                result.push_str("NOTHING");
            }
            result.push_str("\n\nSkipped, they need approval on the desktop:\n");
            result.push_str(&skipped.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"));
        }
        self.announce(task, status, &result).await;
        Outcome { status, result, session: Some(session) }
    }

    /// Cancel the turn and wait a moment for it to end, so its partial text is kept.
    async fn stop(&self, session: &str, timed_out: bool) -> (bool, TurnEnd) {
        self.runner.cancel(session).await;
        let end = tokio::time::timeout(GRACE, self.runner.wait_idle(session)).await.unwrap_or_else(|_| TurnEnd::empty());
        (timed_out, end)
    }

    /// Tell the owner what came of a run, on the channels of the task.
    async fn announce(&self, task: &Task, status: RunStatus, result: &str) {
        if status == RunStatus::Cancelled || result.is_empty() {
            tracing::info!(task = %task.id, status = %status, result_chars = result.chars().count(), "run recorded; nothing is announced");
            return;
        }
        let channels = channels_of(&task.deliver);
        if channels.is_empty() {
            tracing::info!(task = %task.id, status = %status, "run recorded; its channels show nothing");
            return;
        }
        tracing::info!(task = %task.id, status = %status, channels = %task.deliver.join(","), "{}: the result is announced", desktop_sink());
        let (what, body) = match status {
            RunStatus::Timeout => ("timed out", "Stopped at its timeout; see the task's session".to_string()),
            RunStatus::Ok => ("done", chars(result, 300).to_string()),
            _ if result.starts_with("the agent gave no answer") => ("failed", "no result; open the task's session".to_string()),
            _ => ("failed", chars(result, 300).to_string()),
        };
        self.deliver.deliver(Notification { title: format!("Task {what}: {}", task.title), body, urgency: Urgency::Normal, channels }).await;
    }
}

impl Outcome {
    fn failed(result: String, session: Option<String>) -> Self {
        Self { status: RunStatus::Failed, result, session }
    }
}

/// Start the loop of the daemon: a pass every 15 s, with `tasks.maxConcurrent` read
/// from the settings each time. Runs a stopped daemon left `running` are marked failed.
pub fn start(st: Arc<AppState>) {
    let stale = st.tasks.fail_stale();
    if stale > 0 {
        tracing::warn!(tasks = stale, "runs left running by an earlier daemon are marked failed");
    }
    // settings.tasks for the `tasks` tool and the routes: read at each request.
    let settings = st.settings.clone();
    st.tasks.set_defaults(Arc::new(move || settings.read().unwrap().tasks_config()));
    let runner = Arc::new(Runner {
        store: st.tasks.clone(),
        runner: Arc::new(AgentRunner { agent: st.agent.clone(), events: st.events.clone() }),
        deliver: st.notifier.clone(),
        now: Arc::new(|| schedule::stamp(Local::now())),
    });
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(TICK);
        loop {
            tick.tick().await;
            let max_agent = st.settings.read().unwrap().tasks_config().max_concurrent;
            runner.spawn_due(max_agent);
        }
    });
}
