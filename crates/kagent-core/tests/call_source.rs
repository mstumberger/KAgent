//! usage.md section 2: the model calls of a turn are made for who started it (`CALL_SOURCE`), so the counters can keep them apart.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use kagent_core::model::{CallSource, ModelResponse};
use kagent_core::protocol::{AgentEvent, Via};

#[tokio::test]
async fn a_turn_makes_its_calls_for_whoever_started_it() {
    let reply = || ModelResponse { text: "ok".into(), ..Default::default() };
    let mut r = rig(vec![reply(), reply(), reply(), reply(), reply()], Arc::new(Mutex::new(Spans::default()))).await;
    for via in [Via::Text, Via::Voice, Via::Task, Via::Agent, Via::Telegram] {
        r.agent.send(&r.session, "hello".into(), via).await.unwrap();
        wait_event(&mut r.events, 10, |e| matches!(e, AgentEvent::TurnFinished { .. })).await;
    }
    assert_eq!(*r.model.sources.lock().unwrap(), [CallSource::Chat, CallSource::Voice, CallSource::Task, CallSource::Subagent, CallSource::Chat]);
}
