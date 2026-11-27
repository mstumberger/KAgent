//! Plan mode (plan-todo.md section 2; tools.md section 19): the rule the permission engine
//! applies to a planning session, the tools `plan_enter` and `plan_exit`, the plan file and
//! the two blocks of the prompt. The flag itself is `SessionMeta::planning`: it lives in the
//! session's state and is changed through `Agent::set_planning` and `Agent::approve_plan`,
//! which the tools call.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::permission::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use crate::tool::{CallAssessment, Risk, Tool, ToolContext, ToolContract, ToolOutput};

/// The calls a planning session may make although they are not read-only (tools.md section
/// 19): the agent's own bookkeeping and its research. The memory and session tools come
/// with S25. `agent_run` and `agent_stop` since 2026-11-27: `start_child` refuses any
/// subagent of a planning session that is not read-only (`PLAN_SUBAGENT_DENIED`).
pub const PLAN_ALLOWED: [&str; 10] = ["todo_write", "todo_read", "plan_enter", "plan_exit", "load_skill", "memory_search", "memory_get", "session_search", "agent_run", "agent_stop"];
/// Why a planning session may not start a subagent that is not read-only.
pub const PLAN_SUBAGENT_DENIED: &str = "plan mode: only a read-only subagent (profile explore) may run until the plan is approved";
/// What a planning session answers to a call that is neither read-only nor in `PLAN_ALLOWED`.
pub const PLAN_DENIED: &str = "plan mode: research only until the plan is approved";
/// At most this many bytes in a plan (tools.md section 19: 12 KB).
pub const MAX_PLAN_BYTES: usize = 12 * 1024;
/// The tool and the summary of the card `plan_exit` raises (tools.md section 19).
pub const PLAN_TOOL: &str = "plan";
pub const PLAN_SUMMARY: &str = "Approve plan";
/// The result of an approved `plan_exit`.
pub const PLAN_APPROVED: &str = "Plan approved. Carry it out step by step with the normal permissions.";
/// Why a plan card that still waits is denied when the owner cancels plan mode.
pub const PLAN_CANCELLED: &str = "the user cancelled plan mode";
/// The same when another plan of the conversation was approved meanwhile.
pub const PLAN_SUPERSEDED: &str = "another plan of this conversation was approved";
const NOT_PLANNING: &str = "this conversation is not in plan mode, so there is no plan to approve: call plan_enter first, or answer without plan_exit";
const ALREADY_PLANNING: &str = "this conversation is already in plan mode: research with read-only tools, then call plan_exit with the plan";
const ENTERED: &str = "Plan mode is on for this conversation: research with read-only tools only; every other call is refused from now on. When you know enough, write the plan as Markdown and call plan_exit with it.";
const PLAN_REASON: &str = "Plan mode: the agent researched read-only and changes nothing until you approve this plan.";

/// What every model request of a planning session carries. The spec gives no text for plan
/// mode; without it a turn started with `/plan` gives the model no hint until a call of
/// its own is refused.
pub const PLAN_PROMPT: &str = "\nPLAN MODE is on for this conversation: nothing may change until the user approves a \
    plan. Research with read-only tools only (every other call is refused). Then hand the plan to the \
    user with ONE call: plan_exit, with the whole plan as Markdown in its `plan` argument. Never write \
    the plan as answer text: the user gets no plan and nothing can be approved until plan_exit was \
    called. After the user's answer: approved, carry the plan out step by step under the normal \
    permissions; rejected, revise the plan and call plan_exit again.\n";

/// The plan step of `PermissionEngine::evaluate`, between read-only mode and the destructive
/// step: the denial for a call that is not research in a planning session, `None` when the
/// call goes on to the other steps. A read-only call is always research.
pub fn plan_denial(planning: bool, tool: &str, call: &CallAssessment) -> Option<&'static str> {
    (planning && !call.read_only && !PLAN_ALLOWED.contains(&tool)).then_some(PLAN_DENIED)
}

/// The block of an approved plan (plan-todo.md section 2), in every later request of the
/// session for as long as `approvedPlan` is set.
pub fn approved_prompt(plan: &str) -> String {
    format!("\nApproved plan, follow it:\n{plan}\n")
}

/// The plan an owner approved (config.md section 11, `approvedPlan`): where it was saved, its
/// text and when it was approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedPlan {
    pub path: String,
    pub text: String,
    /// Missing in a meta written before plans expired: such a plan counts as expired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// How long an approved plan stays in the prompt at most (plan-todo.md section 2).
pub const PLAN_TTL: chrono::TimeDelta = chrono::TimeDelta::hours(24);

impl ApprovedPlan {
    /// Whether the plan has run out of time at `now`: 24 h after its approval.
    pub fn expired_at(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.approved_at.is_none_or(|t| now - t >= PLAN_TTL)
    }
}

/// Whether a todo list stored after the approval ends the plan: it has items and every one
/// is completed (plan-todo.md section 2).
pub fn todo_done(items: &[crate::todo::TodoItem]) -> bool {
    !items.is_empty() && items.iter().all(|i| i.status == crate::todo::TodoStatus::Completed)
}

/// Write an approved plan to `<dir>/<sessionId>-<n>.md`, `n` being the first number the
/// session has not used yet, in one step.
pub async fn save(dir: &Path, session_id: &str, plan: &str) -> anyhow::Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let name: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let (mut n, mut path) = (1u32, dir.join(format!("{name}-1.md")));
    while path.exists() {
        n += 1;
        path = dir.join(format!("{name}-{n}.md"));
    }
    crate::session::write_atomic(&path, plan.as_bytes()).await?;
    Ok(path)
}

/// How the tools reach the sessions, as `todo::TodoAccess` does: the daemon creates it
/// before its registry, gives it to both tools, and attaches the agent once it exists.
#[derive(Clone, Default)]
pub struct PlanAccess(Arc<OnceLock<Weak<Agent>>>);

impl PlanAccess {
    /// The agent whose sessions the tools plan in. The first one stays.
    pub fn attach(&self, agent: &Arc<Agent>) {
        let _ = self.0.set(Arc::downgrade(agent));
    }

    fn agent(&self) -> anyhow::Result<Arc<Agent>> {
        self.0.get().and_then(Weak::upgrade).ok_or_else(|| anyhow::anyhow!("plan mode is not available: the agent is not attached"))
    }
}

/// A failure of the agent's session methods, said so that the model knows what to do.
fn explain(e: &anyhow::Error, session_id: &str) -> String {
    if e.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) {
        return format!("there is no session '{session_id}' to plan in");
    }
    format!("plan mode could not be changed: {e:#}")
}

const ENTER_DESCRIPTION: &str = "Enter plan mode for this conversation. Use it when the user asks to plan first, \
    wants to see a plan before anything changes, or asks for a change in several steps that you should not \
    start on your own. From then on you research only: every call that is not read-only is refused. When you \
    know enough, write the plan and call plan_exit with it. Already in plan mode: call plan_exit, not this again.";

const EXIT_DESCRIPTION: &str = "End plan mode by handing the user your plan (a plan written as answer text is \
    not shown and cannot be approved). plan: the whole plan as Markdown, at most 12 KB: what you found, \
    the steps in order with the tool each uses and what it changes, how to undo it, and the risks. The \
    user sees one card, Plan. Approved: plan mode is off, and you carry the plan out step by step with \
    the normal permissions. Rejected with a reason: you stay in plan mode; revise the plan and call \
    plan_exit again. Call it only in plan mode, when the research is done.";

/// `plan_enter`: the session plans from this call on.
pub struct PlanEnter {
    contract: ToolContract,
    access: PlanAccess,
}

impl PlanEnter {
    pub fn new(access: PlanAccess) -> Self {
        Self {
            contract: ToolContract::read_only(
                "plan_enter",
                ENTER_DESCRIPTION,
                json!({"type":"object","properties":{"reason":{"type":"string","description":"optional: what is to be planned"}}}),
            ),
            access,
        }
    }
}

#[async_trait]
impl Tool for PlanEnter {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, _input: &Value) -> CallAssessment {
        CallAssessment::from_contract(&self.contract, "enter plan mode".into(), "plan_enter".into())
    }

    async fn call(&self, _input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let agent = self.access.agent()?;
        Ok(match agent.set_planning(&ctx.session_id, true).await {
            Ok(true) => ToolOutput::ok(ENTERED),
            Ok(false) => ToolOutput::error(ALREADY_PLANNING),
            Err(e) => ToolOutput::error(explain(&e, &ctx.session_id)),
        })
    }
}

#[derive(Deserialize)]
struct ExitArgs {
    /// One text, or its lines (as `skill_save.body`, tools.md section 17): Ollama drops a
    /// tool call whose long Markdown argument it cannot parse (S20).
    #[serde(deserialize_with = "text_or_lines")]
    plan: String,
}

fn text_or_lines<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    use serde::de::Error;
    match Value::deserialize(d)? {
        Value::String(text) => Ok(text),
        Value::Array(items) => items
            .into_iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(|| D::Error::custom("plan: every line must be text")))
            .collect::<Result<Vec<_>, _>>()
            .map(|lines| lines.join("\n")),
        other => Err(D::Error::custom(format!("plan: expected text or an array of lines, got {other}"))),
    }
}

/// `plan_exit`: shows the plan to the owner and waits for the answer.
pub struct PlanExit {
    contract: ToolContract,
    access: PlanAccess,
    /// `<data>/plans`, where an approved plan is saved.
    dir: PathBuf,
}

impl PlanExit {
    pub fn new(access: PlanAccess, dir: impl Into<PathBuf>) -> Self {
        Self {
            contract: ToolContract::read_only(
                "plan_exit",
                EXIT_DESCRIPTION,
                json!({"type":"object","required":["plan"],"properties":{
                    "plan":{"description":"the whole plan as Markdown; at most 12 KB","anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}
                }}),
            )
            // The card waits for the owner: longer than the 15 s of a read-only call, and
            // than the approval timeout (its default is 600 s).
            .timeout(3_600_000, 3_600_000),
            access,
            dir: dir.into(),
        }
    }
}

#[async_trait]
impl Tool for PlanExit {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, _input: &Value) -> CallAssessment {
        CallAssessment::from_contract(&self.contract, "present the plan".into(), PLAN_TOOL.into())
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        // `validate_schema` does not look into `anyOf`: the form is checked here.
        let Ok(args) = serde_json::from_value::<ExitArgs>(input) else {
            return Ok(ToolOutput::error("plan is required: the whole plan as Markdown, one text or one line per item"));
        };
        let plan = args.plan.trim();
        if plan.is_empty() {
            return Ok(ToolOutput::error("the plan is empty: write it as Markdown and call plan_exit again"));
        }
        if plan.len() > MAX_PLAN_BYTES {
            return Ok(ToolOutput::error(format!("the plan has {} bytes; at most {MAX_PLAN_BYTES} are kept: shorten it", plan.len())));
        }
        let agent = self.access.agent()?;
        let sid = ctx.session_id.clone();
        let planning = match agent.planning(&sid).await {
            Ok(planning) => planning,
            Err(e) => return Ok(ToolOutput::error(explain(&e, &sid))),
        };
        if !planning {
            return Ok(ToolOutput::error(NOT_PLANNING));
        }
        let request = ApprovalRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: sid.clone(),
            call_id: ctx.call_id.clone(),
            tool: PLAN_TOOL.into(),
            input: json!({ "plan": plan }),
            summary: PLAN_SUMMARY.into(),
            reason: PLAN_REASON.into(),
            risk: Risk::Low,
            privileged: false,
            destructive: false,
            can_remember: false,
            rule_key: PLAN_TOOL.into(),
            kind: ApprovalKind::Plan,
            preview: Some(plan.to_string()),
        };
        let request_id = request.request_id.clone();
        let (decision, via) = agent.broker().request_via(request, &ctx.cancel).await;
        if let Some(audit) = &agent.audit {
            let data = json!({ "requestId": request_id, "tool": PLAN_TOOL, "summary": PLAN_SUMMARY, "decision": decision.label(), "via": via.label() });
            if let Err(e) = audit.append(crate::audit::AuditKind::Approval, &sid, data) {
                tracing::warn!("audit append failed: {e:#}");
            }
        }
        Ok(match decision {
            ApprovalDecision::Deny { reason } => {
                let reason = reason.unwrap_or_else(|| "the user denied this plan".into());
                if agent.planning(&sid).await.unwrap_or(false) {
                    ToolOutput::ok(format!("Plan rejected: {reason}. Revise the plan and call plan_exit again."))
                } else {
                    ToolOutput::ok(format!("Plan not approved: {reason}. Plan mode is off: go on as the user asks, under the normal permissions."))
                }
            }
            // Never rememberable: any allowance approves this plan and adds no rule.
            _ => match agent.approve_plan(&sid, &self.dir, plan).await {
                Ok(stored) => ToolOutput::ok(format!("{PLAN_APPROVED} It is saved as {}.", stored.path)),
                Err(e) => ToolOutput::error(format!("{PLAN_APPROVED} The plan could not be saved: {e:#}")),
            },
        })
    }
}

#[cfg(test)]
mod tests;
