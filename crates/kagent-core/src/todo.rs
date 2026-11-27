//! The todo list of a session (plan-todo.md section 1; tools.md section 19): the items,
//! how a list is normalised, and the tools `todo_write` and `todo_read`. The list itself
//! is `SessionMeta::todo`: it lives in the session's state and is saved with its meta by
//! `Agent::set_todo`, which the daemon's route calls too.

use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::tool::{CallAssessment, Tool, ToolContext, ToolContract, ToolOutput};

/// At most this many items in one list (tools.md section 19).
pub const MAX_ITEMS: usize = 50;
/// At most this many characters in the content of one item.
pub const MAX_CONTENT_CHARS: usize = 200;
/// What both tools answer while the session has no list.
pub const NO_LIST: &str = "no todo list";

/// `pending`, `in_progress` or `completed` (api.md `TodoItem`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TodoPriority {
    High,
    #[default]
    Medium,
    Low,
}

/// One item of a list as it is stored and sent (config.md section 11, api.md `TodoItem`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

/// One item as `todo_write` and `PUT /api/sessions/:id/todo` take it: `status` and
/// `priority` may be left out (or null). A field that is not one of the three is an error.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TodoItemInput {
    pub content: String,
    #[serde(default)]
    pub status: Option<TodoStatus>,
    #[serde(default)]
    pub priority: Option<TodoPriority>,
}

/// Why a list was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoError(pub String);

impl std::fmt::Display for TodoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TodoError {}

/// The list as it is stored: content trimmed, a status that is missing `pending` and a
/// priority that is missing `medium`, and at most one item `in_progress` (the first;
/// each later one becomes pending). More than `MAX_ITEMS` items, an item without content
/// or one over `MAX_CONTENT_CHARS` characters is refused as a whole.
pub fn normalise(items: Vec<TodoItemInput>) -> Result<Vec<TodoItem>, TodoError> {
    if items.len() > MAX_ITEMS {
        return Err(TodoError(format!("the todo list has {} items; at most {MAX_ITEMS} are kept", items.len())));
    }
    let mut out = Vec::with_capacity(items.len());
    let mut in_progress = false;
    for (i, item) in items.into_iter().enumerate() {
        let content = item.content.trim();
        let chars = content.chars().count();
        if chars == 0 {
            return Err(TodoError(format!("item {} of the todo list has no content", i + 1)));
        }
        if chars > MAX_CONTENT_CHARS {
            return Err(TodoError(format!("item {} of the todo list has {chars} characters; at most {MAX_CONTENT_CHARS} are kept", i + 1)));
        }
        let status = match item.status.unwrap_or_default() {
            TodoStatus::InProgress if in_progress => TodoStatus::Pending,
            TodoStatus::InProgress => {
                in_progress = true;
                TodoStatus::InProgress
            }
            other => other,
        };
        out.push(TodoItem { content: content.to_string(), status, priority: item.priority.unwrap_or_default() });
    }
    Ok(out)
}

fn mark(status: TodoStatus) -> char {
    match status {
        TodoStatus::Completed => 'x',
        TodoStatus::InProgress => '>',
        TodoStatus::Pending => ' ',
    }
}

/// The list as `todo_write` and `todo_read` answer it (tools.md section 19): `[1] [x]
/// content` lines, `x` done, `>` in progress and a space open; only a high item ends in
/// ` (high)`. `NO_LIST` while there is none.
pub fn render(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return NO_LIST.to_string();
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let high = if item.priority == TodoPriority::High { " (high)" } else { "" };
            format!("[{}] [{}] {}{}", i + 1, mark(item.status), item.content, high)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How the tools reach the sessions. The daemon creates it before its registry (the agent
/// does not exist yet), gives it to both tools, and attaches the agent once it exists.
/// A `Weak`, so an agent that is dropped takes its tools with it.
#[derive(Clone, Default)]
pub struct TodoAccess(Arc<OnceLock<Weak<Agent>>>);

impl TodoAccess {
    /// The agent whose sessions the tools write into. The first one stays.
    pub fn attach(&self, agent: &Arc<Agent>) {
        let _ = self.0.set(Arc::downgrade(agent));
    }

    fn agent(&self) -> anyhow::Result<Arc<Agent>> {
        self.0.get().and_then(Weak::upgrade).ok_or_else(|| anyhow::anyhow!("the todo list is not available: the agent is not attached"))
    }
}

/// A failure of `Agent::set_todo` or `Agent::todo`, said so that the model knows what to do.
fn explain(e: &anyhow::Error, session_id: &str) -> String {
    if e.is::<TodoError>() {
        return format!("{e:#}");
    }
    if e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) {
        return format!("there is no session '{session_id}' to keep a todo list in");
    }
    format!("the todo list could not be stored: {e:#}")
}

const WRITE_DESCRIPTION: &str = "Create or update the todo list of this conversation. Use it for work with three or \
     more steps (a diagnosis in several parts, a plan the owner asked for); not for a single answer or a \
     question that needs one or two tool calls. Send the whole list every time: at most 50 items, each with a \
     short content (at most 200 characters), a status (pending, in_progress or completed) and a priority \
     (high, medium or low). Keep one item in_progress, the step you are working on now; mark an item \
     completed as soon as its work is done, and update the list together with starting the next step. The \
     owner sees the list in the chat. The list you send replaces the whole list: an item you leave out is \
     gone. When the owner asks you to add an item, send only the new items with mode \"add\": they are put \
     after the items already there.";

const READ_DESCRIPTION: &str = "Read the todo list of this conversation: one line per item, [x] done, [>] in \
     progress, [ ] open. Use it when you no longer know what is open, for example after the conversation \
     was summarized.";

/// `todo_write`: replaces the list of the session the call runs in.
pub struct TodoWrite {
    contract: ToolContract,
    access: TodoAccess,
}

impl TodoWrite {
    pub fn new(access: TodoAccess) -> Self {
        Self {
            // Read-only (tools.md section 19): the list is the agent's own bookkeeping, so
            // it needs no approval and works in read-only mode. `normalise` checks the
            // limits; the schema states them for the model.
            contract: ToolContract::read_only(
                "todo_write",
                WRITE_DESCRIPTION,
                json!({"type":"object","required":["items"],"properties":{
                    "items":{"type":"array","maxItems":50,"items":{"type":"object","required":["content"],"properties":{
                        "content":{"type":"string","maxLength":200,"description":"one step, at most 200 characters"},
                        "status":{"type":"string","enum":["pending","in_progress","completed"],"description":"default: pending"},
                        "priority":{"type":"string","enum":["high","medium","low"],"description":"default: medium"}
                    }}},
                    "mode":{"type":"string","enum":["replace","add"],"description":"replace (default): the items are the whole list; add: they are put after the items already there"}
                }}),
            ),
            access,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteArgs {
    items: Vec<TodoItemInput>,
    #[serde(default)]
    mode: WriteMode,
}

#[derive(Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum WriteMode {
    #[default]
    Replace,
    Add,
}

impl From<&TodoItem> for TodoItemInput {
    fn from(i: &TodoItem) -> Self {
        Self { content: i.content.clone(), status: Some(i.status), priority: Some(i.priority) }
    }
}

/// The open items of `before` that `after` no longer has (by content): a replace that
/// drops them says so, since a model that meant to add an item often sends only it.
fn dropped_open(before: &[TodoItem], after: &[TodoItem]) -> Vec<String> {
    before
        .iter()
        .filter(|b| b.status != TodoStatus::Completed && !after.iter().any(|a| a.content == b.content))
        .map(|b| b.content.clone())
        .collect()
}

#[async_trait]
impl Tool for TodoWrite {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    /// The card of the call: how many items, not the list.
    fn assess(&self, input: &Value) -> CallAssessment {
        let summary = match input.get("items").and_then(Value::as_array).map(Vec::len) {
            Some(0) => "clear the todo list".to_string(),
            Some(n) => format!("todo list: {n} items"),
            None => "todo list".to_string(),
        };
        CallAssessment::from_contract(&self.contract, summary.clone(), summary)
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        // Not `tools::parse`: input `null` must not read as an empty list, which would clear this one.
        let args = serde_json::from_value::<WriteArgs>(input).map_err(|e| anyhow::anyhow!("items: {e}"))?;
        let agent = self.access.agent()?;
        let before = match agent.todo(&ctx.session_id).await {
            Ok(items) => items,
            Err(e) => return Ok(ToolOutput::error(explain(&e, &ctx.session_id))),
        };
        let items = match args.mode {
            WriteMode::Replace => args.items,
            WriteMode::Add => before.iter().map(TodoItemInput::from).chain(args.items).collect(),
        };
        let stored = agent.set_todo(&ctx.session_id, items).await.map_err(|e| explain(&e, &ctx.session_id));
        Ok(match stored {
            Ok(items) => {
                // An empty list is a clear the model asked for: nothing to warn about.
                let dropped = if items.is_empty() { Vec::new() } else { dropped_open(&before, &items) };
                let mut out = render(&items);
                if !dropped.is_empty() {
                    out.push_str(&format!(
                        "\n\nRemoved {} open item(s) the list had: {}. If that was not meant, send them again with mode \"add\".",
                        dropped.len(),
                        dropped.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ")
                    ));
                }
                ToolOutput::ok(out)
            }
            Err(reason) => ToolOutput::error(reason),
        })
    }
}

/// `todo_read`: the list of the session the call runs in.
pub struct TodoRead {
    contract: ToolContract,
    access: TodoAccess,
}

impl TodoRead {
    pub fn new(access: TodoAccess) -> Self {
        Self {
            contract: ToolContract::read_only("todo_read", READ_DESCRIPTION, json!({"type":"object","properties":{}})),
            access,
        }
    }
}

#[async_trait]
impl Tool for TodoRead {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn call(&self, _input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let agent = self.access.agent()?;
        let stored = agent.todo(&ctx.session_id).await.map_err(|e| explain(&e, &ctx.session_id));
        Ok(match stored {
            Ok(items) => ToolOutput::ok(render(&items)),
            Err(reason) => ToolOutput::error(reason),
        })
    }
}

#[cfg(test)]
mod tests;
