//! The folder a conversation is attached to (sidebar.md section 4): stored in the session's
//! meta, and named in the system prompt of every turn with the text of its `AGENTS.md`.

use std::path::Path;

use tokio::io::AsyncReadExt;

use super::state::SessionState;
use super::Agent;
use crate::protocol::AgentEvent;
use crate::session::{SessionMeta, SessionOrigin};

/// The characters of `AGENTS.md` that reach the prompt.
pub const PROJECT_AGENTS_CHARS: usize = 4000;
/// What is read of the file at most, before the characters are counted.
const READ_BYTES: u64 = 64 * 1024;

/// The text of `<dir>/AGENTS.md` for the prompt: `None` when there is no such regular
/// file, or when the link of it leads to a credential file of KAgent. The first
/// `PROJECT_AGENTS_CHARS` characters, then a note when there are more.
pub async fn agents_md(dir: &Path) -> Option<String> {
    let resolved = tokio::fs::canonicalize(dir.join("AGENTS.md")).await.ok()?;
    if super::hooks::protect::is_credential(&resolved.to_string_lossy()) {
        return None;
    }
    let file = tokio::fs::File::open(&resolved).await.ok()?;
    if !file.metadata().await.ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(READ_BYTES).read_to_end(&mut bytes).await.ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(PROJECT_AGENTS_CHARS).collect();
    if chars.next().is_some() || bytes.len() as u64 == READ_BYTES {
        out.push_str("\n(cut: AGENTS.md is longer)");
    }
    Some(out)
}

/// The block of the system prompt for a conversation in folder `path`. It begins with an
/// empty line, as the skill block does.
pub fn block(path: &str, agents_md: &Option<String>) -> String {
    let mut out = format!(
        "\nProject: {path}\nThis conversation is attached to that folder. Relative paths in the owner's requests are relative to it, and a tool that asks for a folder takes this one unless the owner names another.\n"
    );
    if let Some(text) = agents_md {
        out.push_str("Project notes (AGENTS.md; they guide work inside this folder, and the rules above win where they differ):\n");
        out.push_str(text);
        out.push('\n');
    }
    out
}

impl Agent {
    /// Attach folder `path` to session `id`, or detach it (`None`); the caller checked the
    /// path. The file is written under the meta lock (the old value comes back on an
    /// error), the event goes out after it, and nothing is written when the value is the same.
    /// `updated_at` is left alone.
    pub async fn set_project(&self, id: &str, path: Option<String>) -> anyhow::Result<SessionMeta> {
        self.change_meta(id, |meta| {
            if meta.origin == SessionOrigin::Subagent {
                anyhow::bail!("the session of a subagent cannot be changed: it has the folder of the session that started it");
            }
            Ok(std::mem::replace(&mut meta.project, path.clone()) != path)
        })
        .await
    }

    /// Pin or unpin session `id` in the sidebar.
    pub async fn set_pinned(&self, id: &str, pinned: bool) -> anyhow::Result<SessionMeta> {
        self.change_meta(id, |meta| {
            if meta.origin == SessionOrigin::Subagent {
                anyhow::bail!("the session of a subagent is not in the list: it cannot be pinned");
            }
            Ok(std::mem::replace(&mut meta.pinned, pinned) != pinned)
        })
        .await
    }

    /// The folder of session `id`, if it has one.
    pub async fn project(&self, id: &str) -> anyhow::Result<Option<String>> {
        Ok(self.state(id).await?.meta.lock().await.project.clone())
    }

    /// Run `edit` on the meta under its lock; it says whether it changed anything. A change is
    /// saved (and undone when the save fails), then announced after the lock is released.
    async fn change_meta(&self, id: &str, edit: impl FnOnce(&mut SessionMeta) -> anyhow::Result<bool>) -> anyhow::Result<SessionMeta> {
        let state = self.state(id).await?;
        let mut meta = state.meta.lock().await;
        let before = meta.clone();
        if !edit(&mut meta)? {
            return Ok(meta.clone());
        }
        if let Err(e) = self.store.save_meta(&meta).await {
            *meta = before;
            return Err(e);
        }
        let meta = meta.clone();
        let _ = self.events.send(AgentEvent::SessionMetaChanged { session_id: id.into(), meta: meta.clone() });
        Ok(meta)
    }

    /// The block of the folder for this turn: empty without one (sidebar.md section 4.3).
    /// The text goes through the redactor of the daemon, as a tool output does.
    pub(super) async fn project_block(&self, state: &SessionState) -> String {
        let Some(path) = state.meta.lock().await.project.clone() else { return String::new() };
        let text = block(&path, &agents_md(Path::new(&path)).await);
        match self.services().and_then(|s| s.redact.read().ok().and_then(|r| r.clone())) {
            Some(redact) => redact(&text),
            None => text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(name: &str, content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(name), content).unwrap();
        dir
    }

    #[tokio::test]
    async fn the_block_names_the_folder_and_carries_the_notes_when_there_are_some() {
        let dir = dir_with("AGENTS.md", "Run the tests first.\n");
        let path = dir.path().to_string_lossy().to_string();
        let with = block(&path, &agents_md(dir.path()).await);
        assert!(with.starts_with(&format!("\nProject: {path}\nThis conversation is attached to that folder.")), "{with}");
        assert!(with.contains("Project notes (AGENTS.md;") && with.ends_with("Run the tests first.\n\n"), "{with}");
        let none = block(&path, &None);
        assert!(!none.contains("Project notes") && none.contains("a tool that asks for a folder takes this one"), "{none}");
    }

    #[tokio::test]
    async fn the_notes_are_cut_at_4000_characters() {
        let dir = dir_with("AGENTS.md", &"é".repeat(5000));
        let text = agents_md(dir.path()).await.unwrap();
        assert_eq!(text, format!("{}\n(cut: AGENTS.md is longer)", "é".repeat(PROJECT_AGENTS_CHARS)));
        let exact = dir_with("AGENTS.md", &"a".repeat(PROJECT_AGENTS_CHARS));
        assert_eq!(agents_md(exact.path()).await.unwrap(), "a".repeat(PROJECT_AGENTS_CHARS), "a file of exactly 4000 characters is not cut");
        let huge = dir_with("AGENTS.md", &"a".repeat(200_000));
        assert!(agents_md(huge.path()).await.unwrap().ends_with("(cut: AGENTS.md is longer)"));
    }

    #[tokio::test]
    async fn no_file_or_a_folder_named_so_gives_none() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(agents_md(empty.path()).await, None, "no file");
        std::fs::create_dir(empty.path().join("AGENTS.md")).unwrap();
        assert_eq!(agents_md(empty.path()).await, None, "a folder named AGENTS.md");
        assert_eq!(agents_md(&empty.path().join("nowhere")).await, None, "no such folder");
    }
}
