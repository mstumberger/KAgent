//! `agent_tell {id | name, text, interrupt?}` (tools.md section 21, steering.md section 5): the main agent corrects or
//! interrupts one of its own subagents for the owner ("tell the implementer to leave the tests alone"). It asks like a
//! mutating tool, and always out loud from voice. Only the main agent has it: a subagent is refused it (the depth rule), and
//! the tool finds only the children of the session that calls it, so a worker cannot message another worker, and nothing
//! here reaches the owner's conversation.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::agents::AgentAccess;
use crate::agent::children::{ChildView, TellError};
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

pub const DESCRIPTION: &str = "Tell one of your running subagents something for the owner, for example 'leave the tests alone'. It reads the message at its next step and keeps going; with interrupt true it stops what it is doing now (the current request or command is cancelled) and resumes with the message first. Give the agent's id (agent_list) or part of its job's name. The message is the owner's words, not yours, and it can never approve anything for the agent. It asks the owner first.";

pub struct AgentTell {
    contract: ToolContract,
    access: AgentAccess,
}

impl AgentTell {
    pub fn new(access: AgentAccess) -> Self {
        let contract = ToolContract::read_only(
            "agent_tell",
            DESCRIPTION,
            json!({"type":"object","required":["text"],"properties":{
                "id":{"type":"string","description":"the agent id (from agent_run or agent_list)"},
                "name":{"type":"string","description":"part of the name of its job, when you have no id"},
                "text":{"type":"string","description":"what to tell it, 1 to 2000 characters"},
                "interrupt":{"type":"boolean","description":"stop what it is doing now and resume with this first (default false)"}
            }}),
        )
        .mutating(SideEffect::None, Risk::Low);
        Self { contract, access }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Args {
    id: Option<String>,
    name: Option<String>,
    text: String,
    interrupt: bool,
}

/// The child an input names among `children` (those of the calling session): by id, else by a part of its job's name.
fn pick(children: &[ChildView], id: Option<&str>, name: Option<&str>) -> Result<ChildView, ToolOutput> {
    if let Some(id) = id.map(str::trim).filter(|i| !i.is_empty()) {
        return children.iter().find(|c| c.id == id).cloned().ok_or_else(|| ToolOutput::not_done(&format!("no subagent '{id}' belongs to this session"), "call agent_list to see yours"));
    }
    let Some(name) = name.map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()) else {
        return Err(ToolOutput::not_done("agent_tell needs the agent's id or part of its name", "pass id (from agent_list) or name"));
    };
    let running: Vec<&ChildView> = children.iter().filter(|c| c.title.to_lowercase().contains(&name)).collect();
    match running.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(ToolOutput::not_done(&format!("no subagent of this session has '{name}' in its name"), "call agent_list to see yours")),
        many => Err(ToolOutput::not_done(&format!("{} subagents match '{name}': {}", many.len(), many.iter().map(|c| format!("{} ({})", c.title, c.id)).collect::<Vec<_>>().join(", ")), "pass id")),
    }
}

#[async_trait]
impl Tool for AgentTell {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let who = input.get("id").or_else(|| input.get("name")).and_then(Value::as_str).unwrap_or("a subagent");
        let verb = if input.get("interrupt").and_then(Value::as_bool).unwrap_or(false) { "interrupt" } else { "tell" };
        let a = CallAssessment::from_contract(&self.contract, format!("{verb} subagent {who}: \"{}\"", crate::util::truncate_str(input.get("text").and_then(Value::as_str).unwrap_or_default(), 80)), format!("agent_tell {verb}"));
        // Out loud from voice in every mode, like the other changes that speak for the owner.
        CallAssessment { always_ask: true, ..a }
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: Args = match serde_json::from_value(input) {
            Ok(a) => a,
            Err(e) => return Ok(ToolOutput::error(format!("agent_tell needs id or name, and text: {e}"))),
        };
        let agent = match self.access.agent() {
            Ok(agent) => agent,
            Err(e) => return Ok(ToolOutput::error(e.to_string())),
        };
        let child = match pick(&agent.children_of(&ctx.session_id), args.id.as_deref(), args.name.as_deref()) {
            Ok(c) => c,
            Err(out) => return Ok(out),
        };
        Ok(match agent.tell_child(Some(&ctx.session_id), &child.id, &args.text, args.interrupt).await {
            Ok(_) if args.interrupt => ToolOutput::ok(format!("Interrupted \"{}\" and told it: {}. It resumes with that first.", child.title, crate::util::truncate_str(args.text.trim(), 120))),
            Ok(_) => ToolOutput::ok(format!("Told \"{}\": it will see it at its next step.", child.title)),
            Err(TellError::Ended(status)) => ToolOutput::not_done(&format!("the subagent already ended ({})", status.label()), "nothing to tell it; start a new one with agent_run if the work is not done"),
            Err(TellError::NotStarted) => ToolOutput::not_done(&format!("\"{}\" is still waiting for a free slot and has not started", child.title), "wait for it to start (agent_list), then tell it"),
            Err(e) => ToolOutput::not_done(&e.to_string(), "fix that and call again"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::children::ChildStatus;

    fn view(id: &str, title: &str) -> ChildView {
        ChildView::new(id.into(), title.into(), "general".into(), ChildStatus::Running)
    }

    #[test]
    fn a_child_is_picked_by_id_or_by_a_part_of_its_name() {
        let kids = vec![view("a1", "Implementer for KA-12"), view("b2", "Reviewer")];
        assert_eq!(pick(&kids, Some("b2"), None).unwrap().title, "Reviewer");
        assert_eq!(pick(&kids, None, Some("implement")).unwrap().id, "a1");
        let none = pick(&kids, Some("zz"), None).unwrap_err();
        assert!(none.content.contains("no subagent 'zz' belongs to this session"), "{}", none.content);
        let two = pick(&[view("a1", "Fix A"), view("b2", "Fix B")], None, Some("fix")).unwrap_err();
        assert!(two.content.contains("2 subagents match 'fix'") && two.content.contains("pass id"), "{}", two.content);
        assert!(pick(&kids, None, None).unwrap_err().content.contains("needs the agent's id"));
        assert!(pick(&kids, None, Some("zzz")).unwrap_err().content.contains("has 'zzz' in its name"));
    }
}
