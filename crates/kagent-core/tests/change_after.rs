//! diff.md section 3: a file edit stores the text it wrote beside the backup of the text before, for the diff view.

use std::sync::Arc;
use std::time::Duration;

use kagent_core::journal::{Change, ChangeJournal};
use kagent_core::tool::{Tool, ToolContext, ToolServices};
use kagent_core::tools::change::{EditFile, WriteFile};
use serde_json::json;

#[tokio::test]
async fn an_edit_and_a_write_store_the_text_they_wrote() {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
    let work = tempfile::tempdir_in(&home).unwrap();
    let journal_dir = tempfile::tempdir().unwrap();
    let journal = Arc::new(ChangeJournal::new(journal_dir.path()).unwrap());
    let services = Arc::new(ToolServices::new(journal.clone(), Arc::new(kagent_core::tools::privop::PrivClient::new())));
    let mut ctx = ToolContext::standalone(Duration::from_secs(5), 10_000);
    ctx.session_id = "change-after-test".into();
    ctx.services = Some(services);
    let file = work.path().join("x.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let path = file.display().to_string();
    kagent_core::tools::seen::remember(&ctx.session_id, &path);
    EditFile::new().call(json!({"path": path, "old": "beta", "new": "gamma"}), ctx.clone()).await.unwrap();
    kagent_core::tools::seen::remember(&ctx.session_id, &path);
    WriteFile::new().call(json!({"path": path, "content": "whole new\n"}), ctx.clone()).await.unwrap();
    let changes = journal.list(10);
    assert_eq!(changes.len(), 2);
    let text = |c: &Change, side: &str| {
        let v = if side == "after" { &c.op["afterBackup"] } else { &c.undo.as_ref().unwrap()["backup"] };
        std::fs::read_to_string(v.as_str().unwrap_or_else(|| panic!("{side} in {c:?}"))).unwrap()
    };
    let (write, edit) = (&changes[0], &changes[1]);
    assert_eq!((write.tool.as_str(), edit.tool.as_str()), ("write_file", "edit_file"));
    assert_eq!((text(edit, "before"), text(edit, "after")), ("alpha\nbeta\n".to_string(), "alpha\ngamma\n".to_string()));
    assert_eq!((text(write, "before"), text(write, "after")), ("alpha\ngamma\n".to_string(), "whole new\n".to_string()));
}
