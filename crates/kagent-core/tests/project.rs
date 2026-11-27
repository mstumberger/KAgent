//! The folder of a conversation (sidebar.md section 4): the block in the prompt of every
//! request, a change between turns, the guard on a link to a credential file, and the
//! two setters with their event.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use kagent_core::agent::project::agents_md;
use kagent_core::agent::SendOutcome;
use kagent_core::model::ModelResponse;
use kagent_core::protocol::{AgentEvent, TurnStatus};
use kagent_core::session::SessionOrigin;

fn answer(text: &str) -> ModelResponse {
    ModelResponse { text: text.into(), ..Default::default() }
}

async fn turn(r: &Rig, text: &str) {
    let SendOutcome::Started { handle, .. } = r.agent.send(&r.session, text.into(), Default::default()).await.unwrap() else { panic!("a turn starts") };
    assert_eq!(handle.wait().await.status, TurnStatus::Completed);
}

fn folder(agents: Option<&str>) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    if let Some(text) = agents {
        std::fs::write(dir.path().join("AGENTS.md"), text).unwrap();
    }
    let path = dir.path().to_string_lossy().to_string();
    (dir, path)
}

#[tokio::test]
async fn every_request_of_a_session_with_a_folder_carries_the_block_and_one_without_does_not() {
    let (_keep, path) = folder(Some("Rule: say pong."));
    let r = rig(vec![answer("a"), answer("b"), answer("c")], Arc::new(Mutex::new(Spans::default()))).await;
    turn(&r, "first").await;
    r.agent.set_project(&r.session, Some(path.clone())).await.unwrap();
    turn(&r, "second").await;
    r.agent.set_project(&r.session, None).await.unwrap();
    turn(&r, "third").await;
    let systems = r.model.systems.lock().unwrap().clone();
    assert_eq!(systems.len(), 3);
    assert!(!systems[0].contains("Project: "), "no folder, no block");
    assert!(systems[1].contains(&format!("\nProject: {path}\n")) && systems[1].contains("Rule: say pong."), "a folder attached between two turns is in the next one: {}", systems[1]);
    assert!(!systems[2].contains("Project: "), "detached again");
    let stored = r.store.load_messages(&r.session).await.unwrap();
    assert!(stored.iter().all(|m| !m.content.contains("Project: ")), "the block is never stored in the session");
}

#[tokio::test]
async fn a_folder_without_notes_names_itself_only() {
    let (_keep, path) = folder(None);
    let r = rig(vec![answer("a")], Arc::new(Mutex::new(Spans::default()))).await;
    r.agent.set_project(&r.session, Some(path.clone())).await.unwrap();
    turn(&r, "hi").await;
    let system = r.model.systems.lock().unwrap()[0].clone();
    assert!(system.contains(&format!("Project: {path}")) && !system.contains("Project notes"), "{system}");
}

#[tokio::test]
async fn the_setters_write_the_file_announce_a_change_and_leave_the_rest_alone() {
    let (_keep, path) = folder(None);
    let mut r = rig(vec![], Arc::new(Mutex::new(Spans::default()))).await;
    let before = r.store.meta(&r.session).await.unwrap();
    let changed = |ev: &AgentEvent| matches!(ev, AgentEvent::SessionMetaChanged { .. });
    let meta = r.agent.set_project(&r.session, Some(path.clone())).await.unwrap();
    assert_eq!(meta.project.as_deref(), Some(path.as_str()));
    let AgentEvent::SessionMetaChanged { meta: sent, .. } = wait_event(&mut r.events, 2, changed).await else { unreachable!() };
    assert_eq!(sent.project.as_deref(), Some(path.as_str()));
    let stored = r.store.meta(&r.session).await.unwrap();
    assert_eq!((stored.project.as_deref(), stored.updated_at), (Some(path.as_str()), before.updated_at), "written, and attaching does not reorder");
    assert_eq!(r.agent.project(&r.session).await.unwrap().as_deref(), Some(path.as_str()));

    r.agent.set_project(&r.session, Some(path.clone())).await.unwrap();
    r.agent.set_pinned(&r.session, true).await.unwrap();
    // The same project again made no event: the next one is the pin.
    let AgentEvent::SessionMetaChanged { meta: sent, .. } = wait_event(&mut r.events, 2, changed).await else { unreachable!() };
    assert!(sent.pinned, "the pin is the next event, not the repeated folder");
    r.agent.set_pinned(&r.session, true).await.unwrap();
    r.agent.set_pinned(&r.session, false).await.unwrap();
    r.agent.set_project(&r.session, None).await.unwrap();
    let mut got = Vec::new();
    while let Ok(ev) = r.events.try_recv() {
        if let AgentEvent::SessionMetaChanged { meta, .. } = ev {
            got.push((meta.pinned, meta.project));
        }
    }
    assert_eq!(got, [(false, Some(path.clone())), (false, None)], "unpin, then detach; the repeated pin made none");
    assert!(r.agent.set_project("nobody", None).await.is_err(), "an unknown session");
}

#[tokio::test]
async fn a_subagent_session_cannot_be_attached_or_pinned() {
    let r = rig(vec![], Arc::new(Mutex::new(Spans::default()))).await;
    let child = r.store.create_with("job", SessionOrigin::Subagent, Some(r.session.clone())).await.unwrap();
    assert!(r.agent.set_project(&child.id, Some("/tmp".into())).await.unwrap_err().to_string().contains("has the folder of the session that started it"));
    assert!(r.agent.set_pinned(&child.id, true).await.unwrap_err().to_string().contains("cannot be pinned"));
}

#[tokio::test]
async fn a_link_to_a_credential_file_gives_no_notes() {
    // `KAGENT_HOME` names `<config>/token` for the guard; this binary has one test that sets it.
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("token"), "secret-token").unwrap();
    let project = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(config.join("token"), project.path().join("AGENTS.md")).unwrap();
    // SAFETY: no other test of this binary reads the variable.
    unsafe { std::env::set_var("KAGENT_HOME", home.path()) };
    let got = agents_md(project.path()).await;
    unsafe { std::env::remove_var("KAGENT_HOME") };
    assert_eq!(got, None, "a link must not bring the API token into a prompt");
}
