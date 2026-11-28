//! Notices of the loop (notices.md section 1): the key and kind of a provider error and
//! the recovery that follows it, on the rig of `common/`.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use kagent_core::agent::{AgentConfig, SendOutcome};
use kagent_core::model::ModelResponse;
use kagent_core::protocol::{AgentEvent, TurnStatus};

type Notice = (String, String, Option<String>, Option<String>, Option<String>);

async fn turn(r: &mut Rig, text: &str) -> Vec<Notice> {
    let SendOutcome::Started { handle, .. } = r.agent.send(&r.session, text.into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    assert_eq!(handle.wait().await.status, TurnStatus::Completed);
    let mut out = Vec::new();
    while let Ok(ev) = r.events.try_recv() {
        if let AgentEvent::Notice { level, text, key, session_id, kind } = ev {
            out.push((level, text, key, session_id, kind));
        }
    }
    out
}

fn ok() -> Result<ModelResponse, String> {
    Ok(ModelResponse { text: "ok".into(), ..Default::default() })
}

async fn rig_of(script: Vec<Result<ModelResponse, String>>) -> Rig {
    let cfg = AgentConfig { fallback_model_ref: "fake::backup".into(), fallback_context_tokens: 8_192, ..common::config() };
    rig_with(ScriptedModel::scripted(script), Arc::new(Mutex::new(Spans::default())), cfg).await
}

#[tokio::test]
async fn a_provider_error_has_a_key_a_kind_and_its_session() {
    for (error, class) in [("429 quota", "quota"), ("401 Unauthorized: invalid API key", "key"), ("503 Service Unavailable", "busy")] {
        let mut r = rig_of(vec![Err(error.into()), ok()]).await;
        let notices = turn(&mut r, "hi").await;
        assert_eq!(notices.len(), 1, "{error}");
        let (level, _, key, session, kind) = &notices[0];
        assert_eq!(level, "warn");
        assert_eq!(key.as_deref(), Some(format!("provider:fake:{class}").as_str()), "{error}");
        assert_eq!(session.as_deref(), Some(r.session.as_str()));
        assert_eq!(kind.as_deref(), Some("error"));
    }
}

#[tokio::test]
async fn the_provider_answering_again_sends_a_recovery_with_the_key_of_the_error() {
    let mut r = rig_of(vec![Err("429 quota".into()), ok(), ok(), ok()]).await;
    let first = turn(&mut r, "one").await;
    let error_key = first[0].2.clone();
    assert!(error_key.is_some());
    // The next turn starts on the chat model again, which now answers.
    let second = turn(&mut r, "two").await;
    assert_eq!(second.len(), 1, "{second:?}");
    let (level, text, key, _, kind) = &second[0];
    assert_eq!((level.as_str(), kind.as_deref()), ("info", Some("recovery")));
    assert_eq!(*key, error_key, "the same key closes the error toast");
    assert!(text.contains("answers again"), "{text}");
    // Once: a third turn says nothing.
    assert!(turn(&mut r, "three").await.is_empty());
}

#[tokio::test]
async fn no_recovery_without_a_previous_error() {
    let mut r = rig_of(vec![ok(), ok()]).await;
    assert!(turn(&mut r, "one").await.is_empty());
    assert!(turn(&mut r, "two").await.is_empty());
}
