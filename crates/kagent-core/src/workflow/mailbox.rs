//! Questions of workers and messages to them (workflows.md section 4): who answers, the mailbox with its receipts, and the
//! progress a worker reports. All of it is kept in the run, so the board shows it and a restart does not lose it.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use super::engine::{Engine, EngineError};
use super::model::{Delivery, Message, ProgressStep, Question, QuestionState, StepStatus};
use super::prompts;
use crate::protocol::{AgentEvent, Via};

/// At most this many messages wait for one worker, and this many bytes of text in all.
pub const MAX_WAITING: usize = 8;
pub const MAX_WAITING_BYTES: usize = 12 * 1024;
/// A longer text goes to a file and the message names it.
pub const MAX_INLINE_CHARS: usize = 1500;
pub const MAX_QUESTION_CHARS: usize = 1000;
pub const MAX_OPTIONS: usize = 6;
pub const MAX_PROGRESS_STEPS: usize = 20;
pub const MAX_STEP_TITLE: usize = 120;
/// Messages kept per run.
const MAX_MESSAGES: usize = 200;

/// Who answers a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answerer {
    Brain,
    Owner,
}

impl Answerer {
    fn word(self) -> &'static str {
        match self {
            Self::Brain => "brain",
            Self::Owner => "owner",
        }
    }
}

/// A text that pretends to come from the owner or from another coworker (workflows.md section 4.3).
pub fn forged(text: &str) -> bool {
    let t = text.trim_start().to_lowercase();
    t.starts_with("[from ") || t.starts_with("[owner]") || t.starts_with("[note from the daemon")
}

impl Engine {
    /// The run and task a worker session belongs to (the task is empty for the planner).
    pub fn worker_of(&self, session: &str) -> Option<(String, String)> {
        self.workers.lock().unwrap().get(session).cloned()
    }

    /// `report_progress`: the worker's own steps, shown under its task.
    pub fn report_progress(&self, session: &str, steps: Vec<ProgressStep>) -> Result<String, String> {
        let (run, task) = self.worker_of(session).ok_or("report_progress is for a worker of a workflow run")?;
        if task.is_empty() {
            return Err("the planner has no task to report progress on".into());
        }
        if steps.len() > MAX_PROGRESS_STEPS || steps.iter().any(|s| s.title.trim().is_empty() || s.title.chars().count() > MAX_STEP_TITLE) {
            return Err(format!("steps: at most {MAX_PROGRESS_STEPS}, each with a title of 1 to {MAX_STEP_TITLE} characters"));
        }
        let n = steps.len();
        let done = steps.iter().filter(|s| s.status == StepStatus::Completed).count();
        self.modify(&run, |r| {
            if let Some(t) = r.task_mut(&task) {
                t.progress = steps;
            }
        })
        .map_err(|e| e.to_string())?;
        Ok(format!("Progress recorded: {done} of {n} steps done."))
    }

    /// `ask`: a worker's question. It is shown on the run and goes to the Brain of the conversation that started the run, or at
    /// once to the owner; unanswered for `answer_wait` it goes to the owner too. With `blocking` the answer is the result.
    pub async fn ask(self: &Arc<Self>, session: &str, text: &str, options: Vec<String>, blocking: bool, cancel: &CancellationToken) -> Result<String, String> {
        let (run_id, task) = self.worker_of(session).ok_or("ask is for a worker of a workflow run")?;
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_QUESTION_CHARS || options.len() > MAX_OPTIONS {
            return Err(format!("question: 1 to {MAX_QUESTION_CHARS} characters, at most {MAX_OPTIONS} options"));
        }
        let run = self.get(&run_id).ok_or("the run is gone")?;
        let role = if task.is_empty() { "planner".to_string() } else { run.definition.step(super::Role::Implementer).filter(|_| run.task(&task).is_some_and(|t| t.state != super::TaskState::InReview)).map_or("reviewer", |_| "implementer").to_string() };
        let qid = format!("q_{}", &uuid::Uuid::new_v4().simple().to_string()[..6]);
        let state = if run.origin_session.is_some() { QuestionState::Open } else { QuestionState::Escalated };
        let q = Question { id: qid.clone(), task: task.clone(), text: text.to_string(), options: options.clone(), blocking, state, asked_at: Utc::now(), answer: None, answered_by: None };
        self.modify(&run_id, |r| r.questions.push(q)).map_err(|e| e.to_string())?;
        match &run.origin_session {
            Some(origin) => {
                let note = prompts::question_for_brain(&run_id, &run.title, &role, &task, &qid, text, &options);
                let (agent, origin) = (self.agent.clone(), origin.clone());
                tokio::spawn(async move {
                    let _ = agent.notify(&origin, &note).await;
                });
                let (this, run_id, qid) = (self.clone(), run_id.clone(), qid.clone());
                let wait = self.config().answer_wait;
                tokio::spawn(async move {
                    tokio::time::sleep(wait).await;
                    let _ = this.escalate(&run_id, &qid);
                });
            }
            None => self.question_to_owner(&run_id, text),
        }
        if !blocking {
            return Ok(format!("Your question is recorded ({qid}). Go on with your best guess and say in your report that you guessed; an answer arrives later as a message from the orchestrator."));
        }
        let deadline = tokio::time::Instant::now() + self.config().ask_wait;
        loop {
            if let Some(q) = self.get(&run_id).and_then(|r| r.questions.into_iter().find(|q| q.id == qid)) {
                if let Some(answer) = q.answer {
                    let who = if q.answered_by.as_deref() == Some("owner") { "the owner" } else { "orchestrator" };
                    return Ok(format!("[from {who}] {answer}"));
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(40)) => {}
                _ = cancel.cancelled() => return Err("the call was cancelled".into()),
                _ = tokio::time::sleep_until(deadline) => return Ok("No answer came. Go on with your best guess and say in your report that you guessed.".into()),
            }
        }
    }

    fn question_to_owner(&self, run_id: &str, text: &str) {
        self.needs_owner(run_id, &format!("a worker asks: {}", crate::agent::children::cut(text, 200)));
        self.agent.emit(AgentEvent::keyed_notice("warn", format!("A worker asks: {}", crate::agent::children::cut(text, 200)), format!("run:{run_id}:question")));
    }

    /// The Brain could not answer: the question goes to the owner (a card on the board and a notice).
    pub fn escalate(&self, run_id: &str, qid: &str) -> Result<(), EngineError> {
        let run = self.get(run_id).ok_or_else(|| EngineError::NotFound(format!("no run '{run_id}'")))?;
        let q = run.questions.iter().find(|q| q.id == qid).ok_or_else(|| EngineError::NotFound(format!("no question '{qid}'")))?;
        if q.state != QuestionState::Open {
            return Ok(());
        }
        let text = q.text.clone();
        self.modify(run_id, |r| {
            if let Some(q) = r.questions.iter_mut().find(|q| q.id == qid) {
                q.state = QuestionState::Escalated;
            }
        })?;
        self.question_to_owner(run_id, &text);
        Ok(())
    }

    /// The answer to a question (by the Brain or by the owner). A worker that waits gets it as its result, one that went on
    /// gets a message of the mailbox.
    pub fn answer_question(&self, run_id: &str, qid: &str, text: &str, by: Answerer) -> Result<(), EngineError> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 4000 {
            return Err(EngineError::Invalid("text: 1 to 4000 characters".into()));
        }
        if by == Answerer::Brain && forged(text) {
            return Err(EngineError::Invalid("the answer starts like a message of the owner or of another worker, which it is not".into()));
        }
        let run = self.get(run_id).ok_or_else(|| EngineError::NotFound(format!("no run '{run_id}'")))?;
        let q = run.questions.iter().find(|q| q.id == qid).ok_or_else(|| EngineError::NotFound(format!("no question '{qid}'")))?.clone();
        if q.answer.is_some() {
            return Err(EngineError::Conflict("the question was answered already".into()));
        }
        self.modify(run_id, |r| {
            if let Some(q) = r.questions.iter_mut().find(|q| q.id == qid) {
                q.state = QuestionState::Answered;
                q.answer = Some(text.to_string());
                q.answered_by = Some(by.word().to_string());
            }
        })?;
        if !q.blocking && !q.task.is_empty() {
            let who = if by == Answerer::Owner { "the owner" } else { "the orchestrator" };
            self.send_message(run_id, &q.task, Some(&format!("answer-{qid}")), &format!("Answer from {who} to your question \"{}\": {text}", crate::agent::children::cut(&q.text, 120)))?;
        }
        Ok(())
    }

    /// A message of the orchestrator to the worker of `task` (workflows.md section 4.3). A repeat of an id is the same message.
    pub fn send_message(&self, run_id: &str, task: &str, id: Option<&str>, text: &str) -> Result<Message, EngineError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(EngineError::Invalid("text: the message is empty".into()));
        }
        if forged(text) {
            return Err(EngineError::Invalid("the text starts like a message of the owner or of another worker; the engine adds the prefix itself".into()));
        }
        let run = self.get(run_id).ok_or_else(|| EngineError::NotFound(format!("no run '{run_id}'")))?;
        if run.task(task).is_none() {
            return Err(EngineError::NotFound(format!("no task '{task}' in this run")));
        }
        let id = id.map(str::to_string).unwrap_or_else(|| format!("m_{}", &uuid::Uuid::new_v4().simple().to_string()[..6]));
        if let Some(m) = run.messages.iter().find(|m| m.id == id && m.task == task) {
            return Ok(m.clone());
        }
        // What waited too long goes first, so its room is free again.
        self.flush_messages(run_id, task);
        let run = self.get(run_id).ok_or_else(|| EngineError::NotFound(format!("no run '{run_id}'")))?;
        let waiting: Vec<&Message> = run.messages.iter().filter(|m| m.task == task && m.state == Delivery::Queued).collect();
        if waiting.len() >= MAX_WAITING || waiting.iter().map(|m| m.text.len()).sum::<usize>() + text.len() > MAX_WAITING_BYTES {
            return Err(EngineError::Conflict(format!("the worker has {} messages waiting (at most {MAX_WAITING}, {} KB): they are delivered when it is running", waiting.len(), MAX_WAITING_BYTES / 1024)));
        }
        let (body, file) = if text.chars().count() > MAX_INLINE_CHARS {
            let rel = self.store.save_message(run_id, &id, text).map_err(|e| EngineError::Failed(e.to_string()))?;
            (format!("{}\n(the full text is in {rel} under the run's folder)", crate::agent::children::cut(text, 300)), Some(rel))
        } else {
            (text.to_string(), None)
        };
        let message = Message { id, task: task.to_string(), text: body, state: Delivery::Queued, at: Utc::now(), delivered_at: None, file };
        let out = message.clone();
        self.modify(run_id, |r| {
            r.messages.push(message);
            if r.messages.len() > MAX_MESSAGES {
                r.messages.remove(0);
            }
        })?;
        self.flush_messages(run_id, task);
        Ok(self.get(run_id).and_then(|r| r.messages.into_iter().find(|m| m.id == out.id && m.task == task)).unwrap_or(out))
    }

    /// Deliver what waits for a worker that is running now, and expire what waited too long.
    pub(super) fn flush_messages(&self, run_id: &str, task: &str) {
        let Some(run) = self.get(run_id) else { return };
        let expiry = chrono::Duration::from_std(self.config().ask_wait).unwrap_or_else(|_| chrono::Duration::minutes(30));
        let now = Utc::now();
        let running = self.workers.lock().unwrap().iter().filter(|(_, (r, t))| r == run_id && t == task).map(|(s, _)| s.clone()).find(|s| self.agent.child_of(&run.run_session, s).is_some_and(|(v, _, _)| v.status == crate::agent::children::ChildStatus::Running));
        let mut delivered: Vec<String> = Vec::new();
        let mut expired: Vec<String> = Vec::new();
        for m in run.messages.iter().filter(|m| m.task == task && m.state == Delivery::Queued) {
            if let Some(session) = &running {
                // Queued into the running turn, read at its next model request, as steering is.
                let agent = self.agent.clone();
                let (s, text) = (session.clone(), format!("[from orchestrator] {}", m.text.replace('\n', " ")));
                tokio::spawn(async move {
                    let _ = agent.send(&s, text, Via::Agent).await;
                });
                delivered.push(m.id.clone());
            } else if now - m.at > expiry {
                expired.push(m.id.clone());
            }
        }
        if delivered.is_empty() && expired.is_empty() {
            return;
        }
        let _ = self.modify(run_id, |r| {
            for m in r.messages.iter_mut().filter(|m| m.task == task) {
                if delivered.contains(&m.id) {
                    m.state = Delivery::Delivered;
                    m.delivered_at = Some(now);
                } else if expired.contains(&m.id) {
                    m.state = Delivery::Expired;
                }
            }
        });
    }
}
