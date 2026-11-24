//! Change layer end to end without root: write/edit/undo of a home file through the
//! journal, and a clear error when the root helper is missing.

use std::sync::Arc;
use std::time::Duration;

use kagent_core::journal::{ChangeJournal, ChangeStatus};
use kagent_core::tool::{ToolContext, ToolServices};
use kagent_core::tools::builtin_registry;
use kagent_core::tools::privop::PrivClient;
use serde_json::json;

fn ctx(journal: Arc<ChangeJournal>) -> ToolContext {
    let mut c = ToolContext::standalone(Duration::from_secs(20), 24_000);
    c.session_id = "test".into();
    c.call_id = "c1".into();
    c.services = Some(Arc::new(ToolServices::new(journal, Arc::new(PrivClient::with_path("/nonexistent/kagent-priv")))));
    c
}

#[tokio::test]
async fn write_edit_undo_roundtrip_in_home() {
    let jdir = tempfile::tempdir().unwrap();
    let journal = Arc::new(ChangeJournal::new(jdir.path()).unwrap());
    let ctx = ctx(journal.clone());
    let home = std::env::var("HOME").unwrap();
    let dir = format!("{home}/.cache/kagent-test-{}", uuid::Uuid::new_v4().simple());
    std::fs::create_dir_all(&dir).unwrap();
    let file = format!("{dir}/note.txt");
    let reg = builtin_registry();

    let w = reg.get("write_file").unwrap().call(json!({"path": file, "content": "hello\nhello again\n", "mode": "create"}), ctx.clone()).await.unwrap();
    assert!(!w.is_error, "{}", w.content);
    let e = reg.get("edit_file").unwrap().call(json!({"path": file, "old": "hello", "new": "x"}), ctx.clone()).await.unwrap();
    assert!(e.is_error && e.content.contains("2 times"), "{}", e.content);
    let e = reg.get("edit_file").unwrap().call(json!({"path": file, "old": "hello again", "new": "bye"}), ctx.clone()).await.unwrap();
    assert!(!e.is_error && e.content.contains("+bye"), "{}", e.content);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\nbye\n");

    let changes = journal.list(10);
    assert_eq!(changes.len(), 2);
    // Undo newest first: the edit, then the creation.
    for c in &changes {
        let out = reg.get("undo_change").unwrap().call(json!({"change_id": c.id}), ctx.clone()).await.unwrap();
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(journal.get(&c.id).unwrap().status, ChangeStatus::Undone);
        if c.tool == "edit_file" {
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\nhello again\n");
        }
    }
    assert!(!std::path::Path::new(&file).exists(), "created file removed by undo");
    // Undoing twice is refused.
    let again = reg.get("undo_change").unwrap().call(json!({"change_id": changes[0].id}), ctx.clone()).await.unwrap();
    assert!(again.is_error);
    std::fs::remove_dir_all(&dir).unwrap();

    // Outside $HOME is refused.
    let out = reg.get("write_file").unwrap().call(json!({"path": "/etc/kagent-test", "content": "x"}), ctx.clone()).await.unwrap();
    assert!(out.is_error);
}

#[tokio::test]
async fn missing_root_helper_gives_install_hint() {
    let jdir = tempfile::tempdir().unwrap();
    let ctx = ctx(Arc::new(ChangeJournal::new(jdir.path()).unwrap()));
    let reg = builtin_registry();
    let err = reg.get("sysctl").unwrap().call(json!({"action": "set", "key": "vm.swappiness", "value": "10"}), ctx).await.unwrap_err();
    assert!(format!("{err:#}").contains("kagentd sudoers"), "{err:#}");
}
