//! A run's tasks are tickets (workflows.md section 7, tickets.md section 9): the run is a ticket, each task a child of it,
//! and their states follow the run's and the task's. The sync listens to `runUpdated`; the keys it makes are stored on the run,
//! so a restart finds them. History entries say `by: agent`. The owner who moves a ticket by hand is not fought: a ticket is
//! only written when the task's state changed since the last time this daemon wrote it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use kagent_core::protocol::AgentEvent;
use kagent_core::workflow::{Engine, Run, RunState, TaskState};
use tokio::sync::broadcast;

use crate::tickets::{By, Create, State, Tickets, Update};

/// The ticket state of a task (workflows.md section 7).
pub fn task_state(state: TaskState) -> State {
    match state {
        TaskState::Waiting | TaskState::Blocked => State::Todo,
        TaskState::Implementing | TaskState::Changes => State::InProgress,
        TaskState::InReview | TaskState::NeedsOwner => State::InReview,
        TaskState::Done => State::Done,
        TaskState::Dropped => State::Dropped,
        TaskState::Failed | TaskState::Stopped => State::Backlog,
    }
}

/// The ticket state of a run.
pub fn run_state(state: RunState) -> State {
    match state {
        RunState::Planning | RunState::Working | RunState::Paused => State::InProgress,
        RunState::AwaitingApproval => State::InReview,
        RunState::Done => State::Done,
        RunState::Failed | RunState::Stopped => State::Backlog,
    }
}

pub struct TicketSync {
    tickets: Arc<Tickets>,
    engine: Arc<Engine>,
    /// The state a ticket was last written for: the key, the word of the task or run state.
    seen: Mutex<HashMap<String, String>>,
    lock: Mutex<()>,
}

fn cut(text: &str, n: usize) -> String {
    kagent_core::agent::children::cut(text, n).to_string()
}

impl TicketSync {
    pub fn new(tickets: Arc<Tickets>, engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { tickets, engine, seen: Mutex::new(HashMap::new()), lock: Mutex::new(()) })
    }

    /// Keep the tickets of every run up to date as `runUpdated` comes (and once now, for the runs read back at start).
    pub fn spawn(self: &Arc<Self>, mut events: broadcast::Receiver<AgentEvent>) {
        let this = self.clone();
        tokio::spawn(async move {
            for run in this.engine.list(None) {
                this.sync_run(&run.id);
            }
            loop {
                match events.recv().await {
                    Ok(AgentEvent::RunUpdated { run_id, .. }) => this.sync_run(&run_id),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        for run in this.engine.list(None) {
                            this.sync_run(&run.id);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    fn sync_run(&self, id: &str) {
        let _guard = self.lock.lock().unwrap();
        let Some(run) = self.engine.get(id) else { return };
        let (run_key, tasks) = self.sync(&run);
        if let Err(e) = self.engine.set_tickets(id, run_key.as_deref(), &tasks) {
            tracing::warn!(run = id, "the keys of the tickets could not be stored: {e}");
        }
    }

    /// Make and change the tickets of `run`: the run's key (when it was made now) and the keys of its tasks.
    pub fn sync(&self, run: &Run) -> (Option<String>, HashMap<String, String>) {
        let mut made = None;
        let mut keys: HashMap<String, String> = HashMap::new();
        let run_key = match run.ticket.clone() {
            Some(k) => k,
            None => {
                let create = Create {
                    project: Some(run.project.clone()),
                    title: cut(&run.title, 200),
                    description: cut(&format!("Workflow run {} ({}).\n\n{}", run.id, run.workflow, run.goal), 20_000),
                    state: Some(run_state(run.state)),
                    labels: vec!["run".into()],
                    ..Default::default()
                };
                match self.tickets.create(create, By::Agent) {
                    Ok(t) => {
                        self.seen.lock().unwrap().insert(t.key.clone(), run.state.word().into());
                        made = Some(t.key.clone());
                        t.key
                    }
                    Err(e) => {
                        tracing::warn!(run = %run.id, "the ticket of the run was not made: {}", e.message());
                        return (None, keys);
                    }
                }
            }
        };
        self.follow(&run_key, run.state.word(), run_state(run.state), matches!(run.state, RunState::Failed | RunState::Stopped).then(|| format!("the run {} {}{}", run.id, run.state.word(), if run.reason.is_empty() { String::new() } else { format!(": {}", run.reason) })), run.state == RunState::Stopped);
        // The tasks become tickets once the plan stands (approved, or not gated).
        if matches!(run.state, RunState::Planning | RunState::AwaitingApproval) {
            return (made, keys);
        }
        for t in &run.tasks {
            let key = match &t.ka_ticket {
                Some(k) => k.clone(),
                None => {
                    let tracker = t.ticket.as_deref().map(|k| format!("\n\nTracker: {k}")).unwrap_or_default();
                    let create = Create {
                        project: Some(run.project.clone()),
                        title: cut(&format!("{} {}", t.id, t.title), 200),
                        description: cut(&format!("{}{tracker}\n\nTask of run {}.", t.description, run.id), 20_000),
                        state: Some(task_state(t.state)),
                        labels: vec!["run-task".into()],
                        parent: Some(run_key.clone()),
                        done_when: t.done_when.iter().take(20).map(|l| cut(l, 300)).collect(),
                        ..Default::default()
                    };
                    match self.tickets.create(create, By::Agent) {
                        Ok(tk) => {
                            self.seen.lock().unwrap().insert(tk.key.clone(), t.state.word().into());
                            keys.insert(t.id.clone(), tk.key.clone());
                            tk.key
                        }
                        Err(e) => {
                            tracing::warn!(run = %run.id, task = %t.id, "the ticket of the task was not made: {}", e.message());
                            continue;
                        }
                    }
                }
            };
            let note = match t.state {
                TaskState::Stopped => Some("the run was stopped before this task was done".to_string()),
                TaskState::Failed => Some(format!("failed: {}", cut(&t.error, 300))),
                TaskState::Blocked => Some(cut(&t.error, 300)),
                _ => None,
            };
            self.follow(&key, t.state.word(), task_state(t.state), note.filter(|_| matches!(t.state, TaskState::Stopped | TaskState::Failed)), t.state == TaskState::Stopped);
        }
        (made, keys)
    }

    /// Write `state` to the ticket when `word` (the task's or run's own state) is not what was written last.
    fn follow(&self, key: &str, word: &str, state: State, note: Option<String>, stopped: bool) {
        if self.seen.lock().unwrap().get(key).is_some_and(|w| w == word) {
            return;
        }
        let Ok(ticket) = self.tickets.get(key) else { return };
        let mut update = Update { note, ..Default::default() };
        if ticket.state != state {
            update.state = Some(state);
        }
        if stopped && !ticket.labels.iter().any(|l| l == "stopped") {
            let mut labels = ticket.labels.clone();
            labels.push("stopped".into());
            update.labels = Some(labels);
        }
        match self.tickets.update(key, update, By::Agent) {
            Ok(_) => {}
            Err(e) => tracing::warn!(ticket = key, "the ticket was not updated: {}", e.message()),
        }
        self.seen.lock().unwrap().insert(key.to_string(), word.to_string());
    }
}
