use super::*;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::{AgentConfig, AgentDeps};
use kagent_core::model::{ModelClient, ModelDelta, ModelRequest, ModelResponse};
use kagent_core::permission::{PermissionBroker, PermissionEngine, PermissionMode};
use kagent_core::protocol::AgentEvent;
use kagent_core::tool::ToolRegistry;

struct NoModel;

#[async_trait]
impl ModelClient for NoModel {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, _cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        anyhow::bail!("no model in this test")
    }
}

struct Rig {
    agent: Arc<Agent>,
    events: broadcast::Receiver<AgentEvent>,
    id: String,
    /// A folder to attach, with an AGENTS.md.
    folder: tempfile::TempDir,
    _dir: tempfile::TempDir,
}

async fn rig() -> Rig {
    rig_with(Arc::new(NoModel)).await
}

/// A model that never answers until its turn is cancelled: a turn that stays running.
struct Hang;

#[async_trait]
impl ModelClient for Hang {
    async fn complete(&self, _request: ModelRequest, _deltas: mpsc::UnboundedSender<ModelDelta>, cancel: CancellationToken) -> anyhow::Result<ModelResponse> {
        cancel.cancelled().await;
        anyhow::bail!("cancelled")
    }
}

async fn rig_with(model: Arc<dyn ModelClient>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().join("sessions")).unwrap();
    let meta = store.create("t").await.unwrap();
    let (events, rx) = broadcast::channel(16);
    let cfg = AgentConfig { model_ref: "fake::test".into(), system_prompt: String::new(), max_iterations: 4, options: Default::default(), privilege: Default::default(), context_tokens: 0, fallback_model_ref: String::new(), fallback_context_tokens: 0, fallback_max_output: None };
    let agent = Agent::new(AgentDeps {
        services: None,
        hooks: None,
        audit: None,
        model,
        tools: Arc::new(ToolRegistry::new()),
        permissions: Arc::new(PermissionEngine::new(PermissionMode::Ask, vec![])),
        broker: PermissionBroker::new(events.clone(), Duration::from_secs(5)),
        store,
        events,
        config: Arc::new(move |_| cfg.clone()),
        on_rules_changed: None,
    });
    let folder = tempfile::tempdir().unwrap();
    Rig { agent, events: rx, id: meta.id, folder, _dir: dir }
}

type Refused = (StatusCode, &'static str, String);

fn refusal<T>(r: Result<T, HttpError>) -> Refused {
    match r {
        Ok(_) => panic!("not refused"),
        Err(HttpError(status, code, message)) => (status, code, message),
    }
}

fn bad(m: &str) -> Refused {
    (StatusCode::BAD_REQUEST, "invalid_input", m.to_string())
}

fn events(r: &mut Rig) -> usize {
    let mut n = 0;
    while let Ok(ev) = r.events.try_recv() {
        n += matches!(ev, AgentEvent::SessionMetaChanged { .. }) as usize;
    }
    n
}

fn canonical(p: &FsPath) -> String {
    std::fs::canonicalize(p).unwrap().to_string_lossy().to_string()
}

use std::path::Path as FsPath;

#[test]
fn the_body_of_a_project_is_a_string_or_null_under_path() {
    assert_eq!(path_of(&json!({"path": "/tmp"})), Ok(Some("/tmp".into())));
    assert_eq!(path_of(&json!({"path": null})), Ok(None));
    assert_eq!(path_of(&json!({"path": "  "})), Ok(None));
    assert_eq!(path_of(&json!({"path": ""})), Ok(None));
    for body in [json!({}), json!({"project": "/tmp"}), json!({"path": 5}), json!({"path": ["/tmp"]}), json!("/tmp"), json!(null)] {
        assert_eq!(path_of(&body), Err("expected {path: a folder, or null to detach}".to_string()), "{body}");
    }
}

#[tokio::test]
async fn every_refusal_of_a_folder_has_its_status_code_and_message_and_changes_nothing() {
    let mut r = rig().await;
    let (agent, id) = (r.agent.clone(), r.id.clone());
    let put = |body: Value| {
        let (agent, id) = (agent.clone(), id.clone());
        async move { set_project(&agent, &id, &body).await }
    };
    for body in [json!({}), json!({"path": 3}), json!([1]), json!({"folder": "/tmp"})] {
        assert_eq!(refusal(put(body.clone()).await), bad("expected {path: a folder, or null to detach}"), "{body}");
    }
    assert_eq!(refusal(put(json!({"path": "relative/dir"})).await), bad("path must be absolute, or start with ~/"));
    assert_eq!(refusal(put(json!({"path": "~"})).await), bad("path must be absolute, or start with ~/"));
    assert_eq!(refusal(put(json!({"path": "/nowhere/at/all"})).await), bad("not a directory: /nowhere/at/all"));
    let file = r.folder.path().join("a-file");
    std::fs::write(&file, "x").unwrap();
    let given = file.to_string_lossy().to_string();
    assert_eq!(refusal(put(json!({"path": given})).await), bad(&format!("not a directory: {given}")));
    assert_eq!(r.agent.store().meta(&r.id).await.unwrap().project, None, "every refusal left the session as it was");
    assert_eq!(events(&mut r), 0);

    let e = refusal(set_project(&r.agent, "no-such", &json!({"path": null})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::NOT_FOUND, "not_found", "session no-such not found"));
    let child = r.agent.store().create_with("job", SessionOrigin::Subagent, Some(r.id.clone())).await.unwrap();
    let e = refusal(set_project(&r.agent, &child.id, &json!({"path": null})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::CONFLICT, "conflict", "the session of a subagent cannot be changed: it has the folder of the session that started it"));
}

#[tokio::test]
async fn kagents_own_folders_and_what_is_inside_them_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let (config, data) = (home.path().join("config"), home.path().join("data"));
    std::fs::create_dir_all(config.join("agents")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let own = || (config.clone(), data.clone());
    for inside in [config.clone(), data.clone(), config.join("agents")] {
        let given = inside.to_string_lossy().to_string();
        let e = refusal(check_with(&given, own()).await);
        assert_eq!(e, bad(&format!("{given} is KAgent's own folder (<config> or <data>): a conversation cannot be attached to it")));
    }
    // A link into it is refused as well, and a folder beside it is fine.
    let link = home.path().join("link");
    std::os::unix::fs::symlink(&config, &link).unwrap();
    assert_eq!(refusal(check_with(&link.to_string_lossy(), own()).await).0, StatusCode::BAD_REQUEST);
    let beside = home.path().join("config-beside");
    std::fs::create_dir(&beside).unwrap();
    assert_eq!(check_with(&beside.to_string_lossy(), own()).await.ok(), Some(std::fs::canonicalize(&beside).unwrap()), "a name that only starts like <config> is not inside it");
}

#[tokio::test]
async fn a_folder_is_stored_canonical_and_detached_with_null_or_an_empty_string() {
    let mut r = rig().await;
    let real = r.folder.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = r.folder.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let meta = set_project(&r.agent, &r.id, &json!({"path": link.to_string_lossy()})).await.ok().unwrap();
    assert_eq!(meta.project, Some(canonical(&real)), "the link is resolved");
    assert_eq!(r.agent.store().meta(&r.id).await.unwrap().project, Some(canonical(&real)), "written");
    assert_eq!(events(&mut r), 1);

    // The same folder again: the meta is answered, no event.
    set_project(&r.agent, &r.id, &json!({"path": real.to_string_lossy()})).await.ok().unwrap();
    assert_eq!(events(&mut r), 0, "nothing changed");

    for body in [json!({"path": null}), json!({"path": ""})] {
        set_project(&r.agent, &r.id, &json!({"path": real.to_string_lossy()})).await.ok().unwrap();
        events(&mut r);
        assert_eq!(set_project(&r.agent, &r.id, &body).await.ok().unwrap().project, None, "{body}");
        assert_eq!(events(&mut r), 1);
    }
    assert_eq!(set_project(&r.agent, &r.id, &json!({"path": null})).await.ok().unwrap().project, None);
    assert_eq!(events(&mut r), 0, "detaching a session that has none changes nothing");
}

#[tokio::test]
async fn a_path_that_starts_with_a_tilde_is_expanded_before_it_is_stored() {
    let home = std::env::var("HOME").unwrap();
    let r = rig().await;
    let meta = set_project(&r.agent, &r.id, &json!({"path": "~/"})).await.ok().unwrap();
    assert_eq!(meta.project, Some(canonical(FsPath::new(&home))));
}

#[tokio::test]
async fn pin_and_unpin_with_their_refusals_and_the_event_only_on_a_change() {
    let mut r = rig().await;
    let (agent, id) = (r.agent.clone(), r.id.clone());
    let pin = |body: Value| {
        let (agent, id) = (agent.clone(), id.clone());
        async move { set_pinned(&agent, &id, &body).await }
    };
    for body in [json!({}), json!({"pinned": "yes"}), json!({"pinned": null}), json!({"pinned": 1}), json!(true)] {
        assert_eq!(refusal(pin(body.clone()).await), bad("expected {pinned: true or false}"), "{body}");
    }
    assert!(pin(json!({"pinned": true})).await.ok().unwrap().pinned);
    assert_eq!(events(&mut r), 1);
    assert!(pin(json!({"pinned": true})).await.ok().unwrap().pinned, "the meta is answered");
    assert_eq!(events(&mut r), 0);
    assert!(r.agent.store().meta(&r.id).await.unwrap().pinned);
    assert!(!pin(json!({"pinned": false})).await.ok().unwrap().pinned);
    assert_eq!(events(&mut r), 1);

    let e = refusal(set_pinned(&r.agent, "no-such", &json!({"pinned": true})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::NOT_FOUND, "not_found", "session no-such not found"));
    let child = r.agent.store().create_with("job", SessionOrigin::Subagent, Some(r.id.clone())).await.unwrap();
    let e = refusal(set_pinned(&r.agent, &child.id, &json!({"pinned": true})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::CONFLICT, "conflict", "the session of a subagent is not in the list: it cannot be pinned"));
}

#[tokio::test]
async fn the_images_notice_is_remembered_in_the_meta_once() {
    let mut r = rig().await;
    assert!(!r.agent.store().meta(&r.id).await.unwrap().images_notice_shown);
    assert!(set_images_notice(&r.agent, &r.id).await.ok().unwrap().images_notice_shown);
    assert_eq!(events(&mut r), 1);
    assert!(set_images_notice(&r.agent, &r.id).await.ok().unwrap().images_notice_shown);
    assert_eq!(events(&mut r), 0, "no event the second time");
    assert!(r.agent.store().meta(&r.id).await.unwrap().images_notice_shown, "stored");
    let e = refusal(set_images_notice(&r.agent, "no-such").await);
    assert_eq!((e.0, e.1), (StatusCode::NOT_FOUND, "not_found"));
}

#[tokio::test]
async fn a_create_with_a_folder_checks_it_first_and_stores_it_canonical() {
    let r = rig().await;
    let store = r.agent.store();
    let (rest, folder) = split_create(&json!({"title": "t", "model": "a::b", "project": r.folder.path().to_string_lossy()})).await.ok().unwrap();
    assert_eq!(rest, json!({"title": "t", "model": "a::b"}), "the keys of model_api are left");
    let folder = folder.unwrap();
    let meta = create_in(store, store.create("t").await.unwrap(), folder.clone()).await.ok().unwrap();
    assert_eq!(store.meta(&meta.id).await.unwrap().project, Some(canonical(r.folder.path())));
    assert_eq!(split_create(&json!({"project": null})).await.ok().unwrap().1, None);
    assert_eq!(split_create(&json!({})).await.ok().unwrap().1, None);
    assert_eq!(refusal(split_create(&json!({"project": "relative"})).await), bad("path must be absolute, or start with ~/"));
    assert_eq!(refusal(split_create(&json!({"project": "/nowhere/x"})).await), bad("not a directory: /nowhere/x"));
    assert_eq!(refusal(split_create(&json!({"project": 4})).await), bad("expected {path: a folder, or null to detach}"));
}

#[tokio::test]
async fn the_projects_are_the_folders_under_the_roots_that_hold_a_marker() {
    let root = tempfile::tempdir().unwrap();
    let make = |name: &str, marker: Option<&str>, agents: bool| {
        let dir = root.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        if let Some(m) = marker {
            std::fs::write(dir.join(m), "").unwrap();
        }
        if agents {
            std::fs::write(dir.join("AGENTS.md"), "rules").unwrap();
        }
    };
    make("alpha", Some("Cargo.toml"), true);
    make("beta", Some("package.json"), false);
    make("plain", None, true);
    make("gamma", Some("Makefile"), false);
    std::fs::create_dir(root.path().join("gamma/AGENTS.md")).unwrap();
    let mut settings = Settings::defaults();
    settings.patch(json!({"desktop": {"projectRoots": [root.path().to_string_lossy()]}})).unwrap();
    let got = projects(&settings).await;
    let base = root.path().to_string_lossy().to_string();
    assert_eq!(got["roots"], json!([base.clone()]));
    assert_eq!(
        got["projects"],
        json!([
            {"path": format!("{base}/alpha"), "name": "alpha", "agentsMd": true},
            {"path": format!("{base}/beta"), "name": "beta", "agentsMd": false},
            {"path": format!("{base}/gamma"), "name": "gamma", "agentsMd": false},
        ]),
        "no marker, no project; a folder named AGENTS.md is not the file"
    );
    // No roots: the built-in ones, expanded.
    let empty = projects(&Settings::defaults()).await;
    let roots = empty["roots"].as_array().unwrap();
    assert_eq!(roots.len(), xdg::DEFAULT_PROJECT_ROOTS.len());
    assert!(roots.iter().all(|r| r.as_str().unwrap().starts_with('/')), "{roots:?}");
}

#[tokio::test]
async fn the_list_of_a_folder_has_its_top_level_sessions_only() {
    let r = rig().await;
    let store = r.agent.store();
    let folder = canonical(r.folder.path());
    let mut a = store.create("a").await.unwrap();
    a.project = Some(folder.clone());
    store.save_meta(&a).await.unwrap();
    let other = store.create("other").await.unwrap();
    let mut child = store.create_with("kid", SessionOrigin::Subagent, Some(a.id.clone())).await.unwrap();
    child.project = Some(folder.clone());
    store.save_meta(&child).await.unwrap();
    let all = store.list().await.unwrap();
    let titles = |v: Vec<(SessionMeta, usize)>| v.into_iter().map(|(m, n)| (m.title, n)).collect::<Vec<_>>();
    assert_eq!(titles(crate::subagents::listed(all.clone(), None, Some(&folder))), [("a".to_string(), 1)], "the child is not listed, its parent counts it");
    assert_eq!(titles(crate::subagents::listed(all.clone(), None, Some("/nowhere"))), []);
    let plain = titles(crate::subagents::listed(all.clone(), None, None));
    assert!(plain.iter().any(|(t, _)| *t == other.title) && plain.len() == 3, "{plain:?}");
    // The folder is canonicalized when it is a directory: a link to it finds the same sessions.
    let link = r.folder.path().join("..").join(r.folder.path().file_name().unwrap());
    assert_eq!(in_project(all, &link.to_string_lossy()).len(), 2, "the parent and the child, before the visible filter");
}

#[test]
fn a_listing_takes_a_parent_or_a_project_not_both() {
    assert_eq!(refusal(list_filters(Some("p"), Some("/tmp"))), bad("give parent or project, not both"));
    assert!(list_filters(Some("p"), None).is_ok() && list_filters(None, Some("/tmp")).is_ok() && list_filters(None, None).is_ok());
}

#[test]
fn the_title_is_its_words_on_one_line_of_1_to_120_characters() {
    assert_eq!(title_of(&json!({"title": "  Free   the\n disk "})), Ok("Free the disk".into()));
    assert_eq!(title_of(&json!({"title": "a".repeat(120)})), Ok("a".repeat(120)));
    assert_eq!(title_of(&json!({"title": "a".repeat(121)})), Err("the title has 121 characters: at most 120".into()));
    assert_eq!(title_of(&json!({"title": " \n "})), Err("the title is empty".into()));
    for body in [json!({}), json!({"title": 5}), json!({"title": "x", "extra": 1}), json!("x"), json!(null)] {
        assert_eq!(title_of(&body), Err("expected {title: 1 to 120 characters}".to_string()), "{body}");
    }
}

#[tokio::test]
async fn a_rename_is_stored_announced_once_and_kept_against_the_automatic_titles() {
    let mut r = rig().await;
    let meta = set_title(&r.agent, &r.id, &json!({"title": "  My own   name "})).await.ok().unwrap();
    assert_eq!((meta.title.as_str(), meta.renamed), ("My own name", true));
    assert_eq!(events(&mut r), 1);
    set_title(&r.agent, &r.id, &json!({"title": "My own name"})).await.ok().unwrap();
    assert_eq!(events(&mut r), 0, "the same title again changes nothing");
    let stored = r.agent.store().meta(&r.id).await.unwrap();
    assert_eq!((stored.title.as_str(), stored.renamed), ("My own name", true));
    assert_eq!(refusal(set_title(&r.agent, &r.id, &json!({"title": ""})).await), bad("the title is empty"));
    let e = refusal(set_title(&r.agent, "no-such", &json!({"title": "x"})).await);
    assert_eq!((e.0, e.1), (StatusCode::NOT_FOUND, "not_found"));
    let child = r.agent.store().create_with("job", SessionOrigin::Subagent, Some(r.id.clone())).await.unwrap();
    let e = refusal(set_title(&r.agent, &child.id, &json!({"title": "x"})).await);
    assert_eq!((e.0, e.1), (StatusCode::CONFLICT, "conflict"));
}

#[tokio::test]
async fn archive_and_unarchive_with_their_refusals_and_the_event_only_on_a_change() {
    let mut r = rig().await;
    for body in [json!({}), json!({"archived": "yes"}), json!({"archived": true, "pinned": true}), json!(true)] {
        assert_eq!(refusal(set_archived(&r.agent, &r.id, &body).await), bad("expected {archived: true or false}"), "{body}");
    }
    assert!(set_archived(&r.agent, &r.id, &json!({"archived": true})).await.ok().unwrap().archived);
    assert_eq!(events(&mut r), 1);
    set_archived(&r.agent, &r.id, &json!({"archived": true})).await.ok().unwrap();
    assert_eq!(events(&mut r), 0);
    assert!(r.agent.store().meta(&r.id).await.unwrap().archived, "stored");
    assert!(!set_archived(&r.agent, &r.id, &json!({"archived": false})).await.ok().unwrap().archived);
    assert_eq!(events(&mut r), 1);
    let e = refusal(set_archived(&r.agent, "no-such", &json!({"archived": true})).await);
    assert_eq!((e.0, e.1), (StatusCode::NOT_FOUND, "not_found"));
    let child = r.agent.store().create_with("job", SessionOrigin::Subagent, Some(r.id.clone())).await.unwrap();
    let e = refusal(set_archived(&r.agent, &child.id, &json!({"archived": true})).await);
    assert_eq!((e.0, e.1), (StatusCode::CONFLICT, "conflict"));
}

#[tokio::test]
async fn a_running_conversation_cannot_be_archived_or_deleted_but_can_be_brought_back() {
    let r = rig_with(Arc::new(Hang)).await;
    set_archived(&r.agent, &r.id, &json!({"archived": true})).await.ok().unwrap();
    // The handle stays alive: a turn whose handle is dropped is cancelled.
    let _turn = r.agent.send_with(&r.id, "hello".into(), kagent_core::protocol::Via::Text, Default::default()).await.unwrap();
    for _ in 0..100 {
        if r.agent.is_running(&r.id).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(r.agent.is_running(&r.id).await);
    let reason = "the conversation is running: stop it first";
    let e = refusal(set_archived(&r.agent, &r.id, &json!({"archived": true})).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::CONFLICT, "conflict", reason));
    let e = refusal(refuse_running(&r.agent, &r.id).await);
    assert_eq!((e.0, e.1, e.2.as_str()), (StatusCode::CONFLICT, "conflict", reason), "delete is refused the same way");
    assert!(!set_archived(&r.agent, &r.id, &json!({"archived": false})).await.ok().unwrap().archived, "a running conversation may come back from the archive");
    assert!(r.agent.store().meta(&r.id).await.is_ok(), "nothing was deleted");
    // A running subagent keeps its parent from being deleted.
    r.agent.cancel(&r.id).await;
    let idle = rig().await;
    assert!(refuse_running(&idle.agent, &idle.id).await.is_ok());
}

#[tokio::test]
async fn deleting_a_session_removes_its_files_and_its_images_folder() {
    let r = rig().await;
    let images = r.agent.store().images_dir(&r.id);
    tokio::fs::create_dir_all(&images).await.unwrap();
    tokio::fs::write(images.join("a.png"), b"png").await.unwrap();
    r.agent.delete_session(&r.id).await.unwrap();
    assert!(r.agent.store().meta(&r.id).await.is_err());
    assert!(!images.exists(), "the images folder goes with the conversation");
}
