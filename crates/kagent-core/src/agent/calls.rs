//! Tool execution for the agent loop: which tools are on offer, grouping, validation,
//! permission, the call itself, audit and events. Also the entry point for voice and
//! MCP callers.

use std::time::{Duration, Instant};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::hooks::HookRefusal;
use super::Agent;
use crate::permission::{AllowRule, ApprovalDecision, ApprovalKind, ApprovalRequest, Decision};
use crate::protocol::{AgentEvent, Message, ToolCall, ToolCallStatus};
use crate::scheduler::parallel_groups;
use crate::tool::{CallAssessment, PrivilegeMethod, ToolContext};
use crate::util::{budget_output, normalize_input, truncate_str, validate_schema};

impl Agent {
    /// Restrict which tool groups are advertised to the model (tool budget).
    pub fn set_tool_groups(&self, groups: Option<Vec<String>>) {
        *self.tool_groups.lock().unwrap() = groups;
    }

    /// Whether a tool may run: it must belong to an enabled group (an empty or unset
    /// list means all groups). Hidden tools are also refused, so a model cannot call a
    /// disabled tool it learned about earlier in the conversation.
    pub fn tool_enabled(&self, name: &str) -> bool {
        match self.tool_groups.lock().unwrap().as_ref() {
            Some(groups) if !groups.is_empty() => groups.iter().any(|g| g == crate::tools::group_of(name)),
            _ => true,
        }
    }

    /// Tool specs the model may see (enabled groups only). Used by voice and MCP.
    pub fn advertised_specs(&self) -> Vec<crate::tool::ToolSpec> {
        self.advertised_tools()
    }

    /// No session named: every enabled tool, whatever a restricted session would lose.
    pub(super) fn advertised_tools(&self) -> Vec<crate::tool::ToolSpec> {
        self.tools_for("")
    }

    /// Run a batch of calls: assess, group, then execute group by group.
    pub async fn execute_calls(
        &self,
        session_id: &str,
        turn_id: &str,
        calls: &[ToolCall],
        cancel: &CancellationToken,
        privilege: PrivilegeMethod,
    ) -> Vec<Message> {
        let safe: Vec<bool> = calls
            .iter()
            .map(|c| self.tools.get(&c.name).map(|t| t.assess(&c.input).concurrency_safe).unwrap_or(true))
            .collect();
        let mut results: Vec<Option<Message>> = vec![None; calls.len()];
        for group in parallel_groups(&safe) {
            let futures = group.iter().map(|&i| self.execute_call(session_id, turn_id, &calls[i], cancel, privilege));
            let outs = futures::future::join_all(futures).await;
            for (&i, (msg, _)) in group.iter().zip(outs) {
                results[i] = Some(msg);
            }
        }
        results.into_iter().map(|m| m.expect("every call has a result")).collect()
    }

    /// Execute one call through validation, permission and the tool itself. Always
    /// returns a tool-result message. Also used by the live voice bridge.
    pub async fn execute_call(
        &self,
        session_id: &str,
        turn_id: &str,
        call: &ToolCall,
        cancel: &CancellationToken,
        privilege: PrivilegeMethod,
    ) -> (Message, ToolCallStatus) {
        let started = Instant::now();
        let Some(tool) = self.tools.get(&call.name) else {
            let msg = format!("unknown tool '{}'. Available: {}", call.name, self.tool_names_for(session_id));
            return self.finish(session_id, turn_id, call, ToolCallStatus::Error, msg, started, "unknown tool".into());
        };
        if !self.tool_enabled(&call.name) {
            let msg = format!(
                "tool '{}' is disabled (group '{}' is off in settings). Available: {}",
                call.name,
                crate::tools::group_of(&call.name),
                self.tool_names_for(session_id)
            );
            return self.finish(session_id, turn_id, call, ToolCallStatus::Denied, msg, started, "disabled tool".into());
        }
        let contract = tool.contract().clone();
        let input = normalize_input(&contract.input_schema, call.input.clone());
        let call = ToolCall { input, ..call.clone() };
        // A subagent's summary carries the name of its job (its approval card).
        let assessment = self.named(session_id, tool.assess(&call.input));
        // What a subagent's profile takes (agents.md section 2): enforced here, when the
        // call runs, so a tool the model remembered from elsewhere is refused too.
        if let Some(reason) = self.child_refusal(session_id, &call.name, &assessment) {
            return self.finish(session_id, turn_id, &call, ToolCallStatus::Denied, format!("not executed: {reason}"), started, assessment.summary);
        }
        let _ = self.events.send(AgentEvent::ToolCallStarted {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
            summary: assessment.summary.clone(),
        });
        if cancel.is_cancelled() {
            return self.finish(session_id, turn_id, &call, ToolCallStatus::Cancelled, "cancelled by user before running".into(), started, assessment.summary);
        }
        let issues = validate_schema(&contract.input_schema, &call.input);
        if !issues.is_empty() {
            let msg = format!("invalid input for {}: {}. Fix the arguments and call again.", call.name, issues.join("; "));
            return self.finish(session_id, turn_id, &call, ToolCallStatus::Error, msg, started, assessment.summary);
        }

        // Hooks (hooks.md sections 1 and 4): `PreToolUse` after the schema check and before
        // `authorize`. It may end the call, ask for a card (`hook_ask`) or replace the input.
        let (mut call, mut assessment) = (call, assessment);
        let mut hook_ask = None;
        if let Err(refusal) = self.pre_tool_hook(session_id, &mut call, &mut assessment, &mut hook_ask).await {
            let (status, msg) = match refusal {
                HookRefusal::Denied(reason) => {
                    let status = if cancel.is_cancelled() { ToolCallStatus::Cancelled } else { ToolCallStatus::Denied };
                    (status, format!("not executed: {reason}"))
                }
                HookRefusal::Invalid(msg) => (ToolCallStatus::Error, msg),
            };
            return self.finish(session_id, turn_id, &call, status, msg, started, assessment.summary);
        }

        let requested_timeout = call.input.get("timeout_ms").and_then(Value::as_u64);
        let timeout_ms = requested_timeout.unwrap_or(contract.default_timeout_ms).min(contract.max_timeout_ms);
        let ctx = ToolContext {
            cancel: cancel.child_token(),
            timeout: Duration::from_millis(timeout_ms),
            privilege,
            max_output_bytes: contract.max_output_bytes,
            session_id: session_id.to_string(),
            call_id: call.id.clone(),
            services: self.services.clone(),
        };
        // Dry run for the approval card, only when we are going to ask (the engine, or a hook).
        let preview = if hook_ask.is_some() || matches!(self.permissions.evaluate(session_id, &call.name, &assessment), Decision::Ask { .. }) {
            tokio::time::timeout(Duration::from_secs(20), tool.preview(&call.input, &ctx)).await.ok().flatten()
        } else {
            None
        };

        match self.authorize(session_id, &call, &assessment, preview, cancel, hook_ask).await {
            Ok(()) => {}
            Err(reason) => {
                let status = if cancel.is_cancelled() { ToolCallStatus::Cancelled } else { ToolCallStatus::Denied };
                return self.finish(session_id, turn_id, &call, status, format!("not executed: {reason}"), started, assessment.summary);
            }
        }

        let run = tool.call(call.input.clone(), ctx.clone());
        let outcome = tokio::select! {
            r = run => Some(r),
            _ = tokio::time::sleep(Duration::from_millis(timeout_ms + 2_000)) => None,
            _ = cancel.cancelled() => { ctx.cancel.cancel(); None }
        };
        let (status, content) = match outcome {
            Some(Ok(out)) if !out.is_error => (ToolCallStatus::Ok, out.content),
            Some(Ok(out)) => (ToolCallStatus::Error, out.content),
            Some(Err(e)) => (ToolCallStatus::Error, format!("error: {e:#}")),
            None if cancel.is_cancelled() => (ToolCallStatus::Cancelled, "cancelled by user".into()),
            None => (ToolCallStatus::Error, format!("timed out after {timeout_ms} ms")),
        };
        let content = budget_output(&content, contract.max_output_bytes);
        let (is_error, duration_ms) = (status != ToolCallStatus::Ok, started.elapsed().as_millis() as u64);
        let done = self.finish(session_id, turn_id, &call, status, content.clone(), started, assessment.summary);
        // Hooks (hooks.md section 1): `PostToolUse` after the tool returned.
        self.post_tool_hook(session_id, &call, &content, is_error, duration_ms).await;
        done
    }

    /// Ask the broker for `call` (hooks.md section 3 for `hook_ask`: what the engine
    /// would allow gets its card too, and a hook is never satisfied by a remembered rule).
    async fn authorize(
        &self,
        session_id: &str,
        call: &ToolCall,
        assessment: &CallAssessment,
        preview: Option<String>,
        cancel: &CancellationToken,
        hook_ask: Option<String>,
    ) -> Result<(), String> {
        // A hook's `ask` gets its card where the engine would allow the call (hooks.md
        // section 3). The other way round a hook relaxes nothing: a denial stays one.
        let reason = match self.permissions.evaluate(session_id, &call.name, assessment) {
            Decision::Allow { .. } => match hook_ask.as_deref() {
                Some(reason) => format!("a PreToolUse hook asks for your confirmation: {reason}"),
                None => return Ok(()),
            },
            Decision::Deny { reason } => return Err(reason),
            Decision::Ask { reason } => match hook_ask.as_deref() {
                Some(hook) => format!("{reason}; a PreToolUse hook asks too: {hook}"),
                None => reason,
            },
        };
        let request = ApprovalRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.into(),
            call_id: call.id.clone(),
            tool: call.name.clone(),
            kind: ApprovalKind::Tool,
            input: call.input.clone(),
            summary: assessment.summary.clone(),
            reason,
            risk: assessment.risk,
            privileged: assessment.privileged,
            destructive: assessment.destructive,
            // A hook that asks is asked every time: there is nothing to remember.
            can_remember: !assessment.always_ask && !assessment.privileged && !assessment.destructive && hook_ask.is_none(),
            rule_key: assessment.rule_key.clone(),
            preview,
        };
        let can_remember = request.can_remember;
        let (request_id, request_summary) = (request.request_id.clone(), request.summary.clone());
        let decision = self.broker.request(request, cancel).await;
        if let Some(audit) = &self.audit {
            let data = serde_json::json!({
                "requestId": request_id,
                "tool": call.name,
                "summary": request_summary,
                "decision": decision.label(),
            });
            if let Err(e) = audit.append(crate::audit::AuditKind::Approval, session_id, data) {
                tracing::warn!("audit append failed: {e:#}");
            }
        }
        match decision {
            ApprovalDecision::AllowOnce => Ok(()),
            ApprovalDecision::AllowSession => {
                if can_remember {
                    self.permissions.add_session_rule(session_id, AllowRule {
                        tool: call.name.clone(),
                        prefix: assessment.rule_key.clone(),
                    });
                }
                Ok(())
            }
            ApprovalDecision::AllowAlways => {
                if can_remember {
                    self.permissions.add_rule(AllowRule {
                        tool: call.name.clone(),
                        prefix: assessment.rule_key.clone(),
                    });
                    if let Some(sink) = &self.on_rules_changed {
                        sink(self.permissions.rules());
                    }
                }
                Ok(())
            }
            ApprovalDecision::Deny { reason } => {
                Err(reason.unwrap_or_else(|| "the user denied this action".into()))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        session_id: &str,
        turn_id: &str,
        call: &ToolCall,
        status: ToolCallStatus,
        content: String,
        started: Instant,
        summary: String,
    ) -> (Message, ToolCallStatus) {
        if let Some(audit) = &self.audit {
            let output_sha256: String = {
                use sha2::Digest;
                let mut h = sha2::Sha256::new();
                h.update(content.as_bytes());
                h.finalize().iter().map(|b| format!("{b:02x}")).collect()
            };
            let data = serde_json::json!({
                "callId": call.id,
                "turnId": turn_id,
                "tool": call.name,
                "input": crate::audit::redact_input(&call.input),
                "status": format!("{status:?}").to_lowercase(),
                "summary": summary,
                "durationMs": started.elapsed().as_millis() as u64,
                "outputSha256": output_sha256,
            });
            if let Err(e) = audit.append(crate::audit::AuditKind::ToolCall, session_id, data) {
                tracing::warn!("audit append failed: {e:#}");
            }
        }
        let _ = self.events.send(AgentEvent::ToolCallFinished {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            call_id: call.id.clone(),
            name: call.name.clone(),
            status,
            output: truncate_str(&content, 8_000).to_string(),
            duration_ms: started.elapsed().as_millis() as u64,
        });
        let is_error = status != ToolCallStatus::Ok;
        (Message::tool_result(call, content, is_error), status)
    }

}
