//! `tickets {action: add | update | list | close, ...}` (tickets.md section 5): the agent's way to the project's list.
//! `list` is read-only; the other actions change a ticket and ask like a mutating tool, and always out loud from voice.
//! Results are worded per `agent-loop.md` section B.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use serde::Deserialize;
use serde_json::{json, Value};

use super::store::project_name;
use super::{By, Create, Filter, Priority, State, Ticket, TicketError, Tickets, Update};

pub type TicketsCell = Arc<OnceLock<Arc<Tickets>>>;

/// Lines `list` writes before it says how many more there are.
const LIST_LINES: usize = 30;

pub const DESCRIPTION: &str = "The owner's list of tickets for the project of this conversation (or the Inbox when it has none): add, update, list, close. Use it when the owner says 'add to the todo: ...', 'add a ticket', 'remind me to ...' or 'what is on my list' and no multi-step job is running; todo_write is only the working list of a multi-step job you are doing now. Actions: add {title, description?, state?, priority?, labels?, done_when?, parent?}; update {key, title?, description?, state?, priority?, labels?, done_when?, parent?, note?}; list {state?, label?, text?, all_projects?}; close {key, note?} marks it done. State is backlog, todo, in_progress, in_review, done or dropped; priority none, low, medium, high or urgent.";

pub struct TicketsTool {
    contract: ToolContract,
    cell: TicketsCell,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Input {
    action: String,
    key: Option<String>,
    title: Option<String>,
    description: Option<String>,
    state: Option<State>,
    priority: Option<Priority>,
    labels: Option<Vec<String>>,
    done_when: Option<Vec<String>>,
    parent: Option<String>,
    note: Option<String>,
    text: Option<String>,
    label: Option<String>,
    all_projects: bool,
}

impl TicketsTool {
    pub fn new(cell: TicketsCell) -> Self {
        let contract = ToolContract::read_only(
            "tickets",
            DESCRIPTION,
            json!({"type":"object","required":["action"],"properties":{
                "action":{"type":"string","enum":["add","update","list","close"]},
                "key":{"type":"string","description":"The ticket key, like KA-12 (update, close)."},
                "title":{"type":"string"},
                "description":{"type":"string","description":"Markdown."},
                "state":{"type":"string","enum":["backlog","todo","in_progress","in_review","done","dropped"]},
                "priority":{"type":"string","enum":["none","low","medium","high","urgent"]},
                "labels":{"type":"array","items":{"type":"string"}},
                "done_when":{"type":"array","items":{"type":"string"},"description":"Lines that say when the ticket is done."},
                "parent":{"type":"string","description":"The key of the parent ticket in the same project."},
                "note":{"type":"string"},
                "text":{"type":"string","description":"list: text in the key, title or description."},
                "label":{"type":"string","description":"list: one label."},
                "all_projects":{"type":"boolean","description":"list: every project, not only this conversation's."}
            }}),
        )
        .mutating(SideEffect::Workspace, Risk::Low);
        Self { contract, cell }
    }
}

fn input_of(value: Value) -> Result<Input, String> {
    kagent_core::tools::parse::<Input>(value).map_err(|e| format!("the input is not valid: {e}"))
}

fn unknown_key(key: &str) -> ToolOutput {
    ToolOutput::not_done(&format!("no ticket {key}"), "call tickets list to see the keys")
}

fn refusal(e: TicketError, what: &str) -> ToolOutput {
    match e {
        TicketError::NotFound(m) => ToolOutput::not_done(&m, "call tickets list to see the keys"),
        TicketError::Invalid(m) | TicketError::Limit(m) => ToolOutput::not_done(&format!("{what}: {m}"), "fix that and call again"),
        TicketError::Failed(m) => ToolOutput::error(m),
    }
}

fn line(t: &Ticket) -> String {
    let labels = if t.labels.is_empty() { String::new() } else { format!(" ({})", t.labels.join(", ")) };
    format!("{} [{}, {}] {}{labels}", t.key, t.state.word(), t.priority.word(), t.title)
}

/// The answer of `list` for these tickets.
pub fn list_text(tickets: &[Ticket], scope: &str) -> String {
    if tickets.is_empty() {
        return format!("No tickets in {scope}.");
    }
    let mut out: Vec<String> = tickets.iter().take(LIST_LINES).map(line).collect();
    if tickets.len() > LIST_LINES {
        out.push(format!("({} more: narrow it with state, label or text)", tickets.len() - LIST_LINES));
    }
    format!("{} in {scope}:\n{}", if tickets.len() == 1 { "1 ticket".to_string() } else { format!("{} tickets", tickets.len()) }, out.join("\n"))
}

impl TicketsTool {
    async fn project(&self, hub: &Tickets, session: &str) -> Option<String> {
        let agent = hub.agent()?;
        agent.project(session).await.ok().flatten()
    }

    fn add(&self, hub: &Tickets, project: Option<String>, i: Input) -> ToolOutput {
        let Some(title) = i.title.clone().filter(|t| !t.trim().is_empty()) else {
            return ToolOutput::not_done("add needs a title", "pass title");
        };
        let create = Create {
            project: project.clone(),
            title,
            description: i.description.unwrap_or_default(),
            state: i.state,
            priority: i.priority,
            labels: i.labels.unwrap_or_default(),
            parent: i.parent,
            links: Vec::new(),
            assignee: None,
            done_when: i.done_when.unwrap_or_default(),
        };
        match hub.create(create, By::Agent) {
            Ok(t) => ToolOutput::ok(format!("Added {} \"{}\" ({}, {}) to {}.", t.key, t.title, t.state.word(), t.priority.word(), project_name(project.as_deref()))),
            Err(e) => refusal(e, "the ticket was not added"),
        }
    }

    fn update(&self, hub: &Tickets, project: Option<&str>, i: Input, close: bool) -> ToolOutput {
        let Some(key) = i.key.as_deref().map(str::trim).filter(|k| !k.is_empty()) else {
            return ToolOutput::not_done("this action needs a key", "pass key (call tickets list to see them)");
        };
        // Only the calling project's tickets: another project's key is not found from here.
        match hub.get(key) {
            Ok(t) if t.project.as_deref() != project => return unknown_key(key),
            Err(_) => return unknown_key(key),
            Ok(t) if close && t.state == State::Done => return ToolOutput::not_done(&format!("{key} is already done"), "nothing to do"),
            Ok(_) => {}
        }
        let update = Update {
            title: i.title,
            description: i.description,
            state: if close { Some(State::Done) } else { i.state },
            priority: i.priority,
            labels: i.labels,
            done_when: i.done_when,
            parent: i.parent.map(|p| Some(p).filter(|p| !p.is_empty())),
            note: i.note,
            ..Default::default()
        };
        match hub.update(key, update, By::Agent) {
            Ok((_, said)) if said.is_empty() => ToolOutput::not_done(&format!("nothing to change on {key}"), "pass a field to change"),
            Ok((_, said)) if close => ToolOutput::ok(format!("{key} is done.{}", if said.iter().any(|s| s == "note added") { " Note added." } else { "" })),
            Ok((_, said)) => ToolOutput::ok(format!("{key} updated: {}.", said.join("; "))),
            Err(e) => refusal(e, &format!("{key} was not changed")),
        }
    }

    fn list(&self, hub: &Tickets, project: Option<String>, i: Input) -> ToolOutput {
        let scope = if i.all_projects { "all projects".to_string() } else { project_name(project.as_deref()) };
        let filter = Filter {
            project: if i.all_projects { None } else { Some(project.unwrap_or_else(|| "inbox".into())) },
            state: i.state,
            label: i.label.filter(|l| !l.trim().is_empty()),
            text: i.text.filter(|l| !l.trim().is_empty()),
        };
        ToolOutput::ok(list_text(&hub.list(&filter).tickets, &scope))
    }
}

#[async_trait]
impl Tool for TicketsTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let i = input_of(input.clone()).unwrap_or_default();
        let what = match i.action.as_str() {
            "add" => format!("add the ticket \"{}\"", i.title.as_deref().unwrap_or("")),
            "update" => format!("change ticket {}", i.key.as_deref().unwrap_or("?")),
            "close" => format!("close ticket {}", i.key.as_deref().unwrap_or("?")),
            _ => "list the tickets".to_string(),
        };
        let a = CallAssessment::from_contract(&self.contract, what, format!("tickets {}", i.action));
        match i.action.as_str() {
            "list" => a.as_read_only(),
            // Asked out loud from voice in every mode (permission.rs `voice_asks`), like the calendar changes.
            _ => CallAssessment { always_ask: true, ..a },
        }
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let Some(hub) = self.cell.get() else { return Ok(ToolOutput::error("the tickets module is not running")) };
        let i = match input_of(input) {
            Ok(i) => i,
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        let project = self.project(hub, &ctx.session_id).await;
        Ok(match i.action.as_str() {
            "add" => self.add(hub, project, i),
            "update" => self.update(hub, project.as_deref(), i, false),
            "close" => self.update(hub, project.as_deref(), i, true),
            "list" => self.list(hub, project, i),
            other => ToolOutput::not_done(&format!("unknown action {other:?}"), "use add, update, list or close"),
        })
    }
}
