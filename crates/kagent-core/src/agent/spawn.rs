//! A turn's task: it runs the loop under a panic guard, says how the turn ended, starts
//! the `Stop` hooks without waiting for them and hands queued input to the next turn.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::state::{SessionState, TurnInput};
use super::turn::{TurnCell, TurnHandle, TurnResult};
use super::{Agent, INTERNAL_ERROR};
use crate::protocol::{AgentEvent, TurnStatus, Usage};

impl Agent {
    /// Run a turn that `SessionState::take` started, in its own task. The task ends the
    /// turn (the handle resolves, the session may go idle) and starts the turn that
    /// takes over input queued in the meantime.
    pub(super) fn spawn_turn(
        self: &Arc<Self>,
        session_id: &str,
        turn_id: String,
        input: TurnInput,
        state: Arc<SessionState>,
        cancel: CancellationToken,
    ) -> TurnHandle {
        let (handle, done, cell) = TurnHandle::new();
        let (this, sid) = (self.clone(), session_id.to_string());
        tokio::spawn(async move {
            let via = input.via;
            // The calls of the turn are counted under who started it (usage.md section 2).
            let result = crate::model::CALL_SOURCE.scope(crate::model::CallSource::of_via(via), this.run_turn(&sid, &turn_id, input, &state, &cancel, &cell)).await;
            // Hooks (hooks.md section 4): `Stop` after `TurnFinished`, not awaited: the turn
            // ends now; what the hooks add waits for a later turn.
            this.spawn_stop_hooks(&sid, &turn_id, via, result.status, result.error.clone());
            // Settle the session before the handle resolves: a caller that awaited the
            // turn finds it no longer running.
            if let Some((input, cancel)) = state.settle(result.clone()) {
                let turn_id = uuid::Uuid::new_v4().to_string();
                this.spawn_turn(&sid, turn_id, input, state, cancel);
            }
            *cell.result.lock().unwrap() = Some(result);
            let _ = done.send(());
        });
        handle
    }

    /// Run one turn and return how it ended. The loop runs under a panic guard: a panic
    /// (a tool, a provider) ends the turn as failed instead of leaving the session
    /// stuck with a turn that never clears.
    async fn run_turn(
        &self,
        session_id: &str,
        turn_id: &str,
        input: TurnInput,
        state: &SessionState,
        cancel: &CancellationToken,
        cell: &TurnCell,
    ) -> TurnResult {
        let _ = self.events.send(AgentEvent::TurnStarted {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            input: input.text.clone(),
            via: input.via,
        });
        let mut usage = Usage::default();
        let run = futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(
            self.turn_loop(session_id, turn_id, input, state, cancel, &mut usage, cell.text.clone()),
        ));
        let (status, error) = match run.await {
            Ok(Ok(())) if cancel.is_cancelled() => (TurnStatus::Cancelled, None),
            Ok(Ok(())) => (TurnStatus::Completed, None),
            Ok(Err(_)) if cancel.is_cancelled() => (TurnStatus::Cancelled, None),
            Ok(Err(e)) => {
                let error = format!("{e:#}");
                tracing::warn!(session_id, turn_id, "turn failed: {error}");
                (TurnStatus::Failed, Some(error))
            }
            Err(payload) => {
                let panic = if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic payload".to_string()
                };
                tracing::error!(session_id, turn_id, "turn panicked: {panic}");
                (TurnStatus::Failed, Some(INTERNAL_ERROR.into()))
            }
        };
        let text = std::mem::take(&mut *cell.text.lock().unwrap());
        let _ = self.events.send(AgentEvent::TurnFinished {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            status,
            error: error.clone(),
            usage: usage.clone(),
        });
        TurnResult { status, error, text, usage }
    }
}
