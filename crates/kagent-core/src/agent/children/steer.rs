//! The owner talks to a running subagent (steering.md sections 3 and 4): a message that is read at its next model request,
//! and an interrupt that cancels what it is doing and resumes the same session with the message first. The message is
//! data for the worker: it is never an approval, and no path here reaches the approval broker.

use std::fmt;
use std::sync::Arc;

use super::super::notes::STEER_PREFIX;
use super::super::state::TurnInput;
use super::super::Agent;
use super::{ChildStatus, ChildView};
use crate::protocol::Via;

/// At most this many characters in a message to a worker.
pub const MAX_MESSAGE_CHARS: usize = 2_000;

/// Why a message did not reach a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TellError {
    /// No such worker (for the caller).
    Unknown(String),
    /// The text is empty or too long.
    Invalid(String),
    /// It waits for a slot: it has not started, so the message would be lost.
    NotStarted,
    /// It ended.
    Ended(ChildStatus),
}

impl fmt::Display for TellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(id) => write!(f, "no subagent '{id}'"),
            Self::Invalid(why) => f.write_str(why),
            Self::NotStarted => f.write_str("the worker is still queued: it has not started, so it cannot read a message (stop it, or wait for it)"),
            Self::Ended(status) => write!(f, "the worker already ended ({})", status.label()),
        }
    }
}

impl Agent {
    /// The owner's message `text` to child `id`. With `parent`, only a child of that session (the tool's calling session);
    /// without, any child (the owner's window). Whether it interrupts or not, the receipt is on the child's view; the answer
    /// is the child with it.
    pub async fn tell_child(self: &Arc<Self>, parent: Option<&str>, id: &str, text: &str, interrupt: bool) -> Result<ChildView, TellError> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(TellError::Invalid(format!("the message is 1 to {MAX_MESSAGE_CHARS} characters")));
        }
        let (owner, view, _, _) = self.children.get(id).ok_or_else(|| TellError::Unknown(id.into()))?;
        if parent.is_some_and(|p| p != owner) {
            return Err(TellError::Unknown(id.into()));
        }
        match view.status {
            ChildStatus::Queued => return Err(TellError::NotStarted),
            ChildStatus::Running => {}
            ended => return Err(TellError::Ended(ended)),
        }
        let (steer_id, parent_id, view) = self.children.steer_add(id, text, interrupt).ok_or_else(|| TellError::Unknown(id.into()))?;
        if !interrupt {
            self.notes.push_steer(id, &steer_id, text);
            self.child_event(&parent_id, &view);
            return Ok(view);
        }
        // Interrupt: the message waits behind the running turn, which is cancelled; where the cancelled turn settles, the
        // session starts the next one with it first. The mark tells `child_ended` that the child goes on.
        let state = self.state(id).await.map_err(|_| TellError::Unknown(id.into()))?;
        self.children.mark_interrupt(id, true);
        state.pending.lock().unwrap().push_back(TurnInput { text: format!("{STEER_PREFIX}{}", text.replace('\n', " ")), via: Via::Agent, system_suffix: None, images: Vec::new() });
        if !self.cancel(id).await {
            // It ended between the check and the cancel.
            self.children.mark_interrupt(id, false);
            state.pending.lock().unwrap().pop_back();
            let status = self.children.get(id).map_or(ChildStatus::Done, |(_, v, _, _)| v.status);
            return Err(TellError::Ended(status));
        }
        self.child_event(&parent_id, &view);
        Ok(view)
    }

    /// The worker read these messages (their receipts): `delivered`, announced.
    pub(crate) fn steer_delivered(&self, id: &str, steer_ids: &[String]) {
        if let Some((parent, view)) = self.children.steer_delivered(id, steer_ids) {
            self.child_event(&parent, &view);
        }
    }
}

