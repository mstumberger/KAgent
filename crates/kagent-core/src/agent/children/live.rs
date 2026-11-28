//! What a running subagent is doing, as the daemon tells it (steering.md section 2): its last action, the files its tool
//! calls named, and the owner's messages to it with their receipts. The data is kept on the child's `ChildView`, so
//! `subagentUpdated` carries it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::protocol::ToolCallStatus;

/// Files kept per worker.
pub const MAX_FILES: usize = 50;
/// The owner's messages kept per worker (the newest).
pub const MAX_STEERING: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionStatus {
    Running,
    Done,
    Error,
    Refused,
}

impl ActionStatus {
    /// The status of a finished call: `done` for what worked, `refused` for a call that was denied or cancelled,
    /// `error` for the rest.
    pub fn of(status: ToolCallStatus) -> Self {
        match status {
            ToolCallStatus::Ok | ToolCallStatus::Shown | ToolCallStatus::Partial => Self::Done,
            ToolCallStatus::Denied | ToolCallStatus::Cancelled | ToolCallStatus::Skipped => Self::Refused,
            ToolCallStatus::Error | ToolCallStatus::NotDone => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastAction {
    pub tool: String,
    pub summary: String,
    pub status: ActionStatus,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TouchedFile {
    pub path: String,
    pub changed: bool,
}

/// The tools whose `path` input is a file the worker touches, and whether the call changes it.
fn touch_of(tool: &str) -> Option<bool> {
    match tool {
        "write_file" | "edit_file" => Some(true),
        "read_file" | "list_dir" | "search_files" | "open_in_editor" => Some(false),
        _ => None,
    }
}

/// The file a call names, with whether it changes it.
pub fn touched(tool: &str, input: &Value) -> Option<(String, bool)> {
    let changed = touch_of(tool)?;
    let path = input.get("path")?.as_str()?.trim();
    (!path.is_empty()).then(|| (path.to_string(), changed))
}

/// Add a touch: in order of first sight, a path that was read and is later changed becomes changed, at most `MAX_FILES`.
pub fn add_touch(files: &mut Vec<TouchedFile>, path: String, changed: bool) {
    if let Some(f) = files.iter_mut().find(|f| f.path == path) {
        f.changed |= changed;
    } else if files.len() < MAX_FILES {
        files.push(TouchedFile { path, changed });
    }
}

/// Where the owner's message is: waiting for the worker's next model request, or read by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerState {
    Queued,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Steering {
    pub id: String,
    pub text: String,
    pub interrupt: bool,
    pub state: SteerState,
    pub at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_file_tools_name_a_file() {
        assert_eq!(touched("edit_file", &json!({"path": "/a"})), Some(("/a".into(), true)));
        assert_eq!(touched("read_file", &json!({"path": " /b "})), Some(("/b".into(), false)));
        assert_eq!(touched("shell", &json!({"command": "rm /c", "path": "/c"})), None);
        assert_eq!(touched("read_file", &json!({})), None);
        assert_eq!(touched("read_file", &json!({"path": ""})), None);
    }

    #[test]
    fn a_path_read_then_changed_is_changed_and_the_list_is_capped() {
        let mut files = Vec::new();
        add_touch(&mut files, "/a".into(), false);
        add_touch(&mut files, "/b".into(), false);
        add_touch(&mut files, "/a".into(), true);
        add_touch(&mut files, "/a".into(), false);
        assert_eq!(files, vec![TouchedFile { path: "/a".into(), changed: true }, TouchedFile { path: "/b".into(), changed: false }]);
        for i in 0..80 {
            add_touch(&mut files, format!("/f{i}"), false);
        }
        assert_eq!(files.len(), MAX_FILES);
    }

    #[test]
    fn statuses_map_to_done_error_refused() {
        assert_eq!(ActionStatus::of(ToolCallStatus::Ok), ActionStatus::Done);
        assert_eq!(ActionStatus::of(ToolCallStatus::Partial), ActionStatus::Done);
        assert_eq!(ActionStatus::of(ToolCallStatus::Denied), ActionStatus::Refused);
        assert_eq!(ActionStatus::of(ToolCallStatus::Cancelled), ActionStatus::Refused);
        assert_eq!(ActionStatus::of(ToolCallStatus::NotDone), ActionStatus::Error);
        assert_eq!(ActionStatus::of(ToolCallStatus::Error), ActionStatus::Error);
    }
}
