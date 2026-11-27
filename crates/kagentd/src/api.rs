//! HTTP API handlers (docs/specs/api.md section 3, phase-1 subset).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::SendOutcome;
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{AgentEvent, Via};
use kagent_core::tools;

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

mod providers;
pub use providers::{
    add_provider, delete_provider, fetch_models, list_models, list_providers, provider_order, provider_templates, test_provider, update_provider,
};
pub(crate) use providers::named_models;

fn ok_json(v: Value) -> Response {
    (StatusCode::OK, Json(v)).into_response()
}

// ------------------------------------------------------------------ state / settings

pub async fn get_state(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    let sessions = st.agent.store().list().await.map_err(internal)?;
    let mut running = Vec::new();
    for s in &sessions {
        if st.agent.is_running(&s.id).await {
            running.push(s.id.clone());
        }
    }
    let chat_valid = providers::chat_model_valid(&st.providers, &st.settings.read().unwrap().chat_model);
    Ok(ok_json(json!({
        "version": st.version,
        "settings": *st.settings.read().unwrap(),
        "pendingApprovals": st.agent.broker().pending(),
        "running": running,
        "chatModelValid": chat_valid,
        "privHelper": st.services.priv_client.status(),
        "platform": {"desktop": kagent_core::platform::desktop().name(), "wayland": kagent_core::platform::wayland(), "packages": kagent_core::platform::package_manager().name()},
        "alertsOpen": st.alerts.open_count(),
    })))
}

pub async fn put_settings(State(st): State<Arc<AppState>>, Json(patch): Json<Value>) -> ApiResult<Response> {
    // A model must have the role of the job it is given (models.md section 4).
    providers::check_model_roles(&st.providers, &patch)?;
    let wants_autostart = patch.get("startup").and_then(|v| v.get("autostart")).and_then(Value::as_bool);
    {
        let mut s = st.settings.write().unwrap();
        s.patch(patch).map_err(bad_request)?;
        st.agent.permissions().set_mode(s.permission_mode);
        st.agent.permissions().set_rules(s.rules.clone());
        st.agent.permissions().set_overrides(crate::settings::parse_overrides(&s.tool_overrides));
        st.agent.set_tool_groups(Some(s.tool_groups.clone()));
        *st.services.desktop.write().unwrap() = s.desktop_prefs();
        if let Some(q) = s.extra.get("watchers").and_then(|w| w.get("quietHours")).and_then(Value::as_str) {
            st.alerts.set_quiet_hours(q);
        }
        s.save(&crate::paths::Paths::resolve().settings()).map_err(internal)?;
        if let Some(want) = wants_autostart {
            if want != s.startup.autostart {
                return Err(bad_request("startup.autostart patch did not apply"));
            }
        }
    }
    // At most `subagents.maxConcurrent` subagents run at once (agents.md section 2).
    let max_children = st.settings.read().unwrap().subagents_config().max_concurrent;
    st.agent.set_max_concurrent_children(max_children as usize).await;
    // Apply the systemd user unit change outside the settings lock (spawns
    // systemctl; slow paths must never hold the lock).
    if let Some(want) = wants_autostart {
        tokio::task::spawn_blocking(move || crate::startup::apply_requested(want));
    }
    let settings = st.settings.read().unwrap().clone();
    let _ = st.events.send(AgentEvent::SettingsChanged {
        settings: serde_json::to_value(&settings).unwrap_or(Value::Null),
    });
    Ok(ok_json(serde_json::to_value(settings).unwrap_or(Value::Null)))
}

fn internal(e: anyhow::Error) -> HttpError {
    HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", format!("{e:#}"))
}

// ------------------------------------------------------------------ tools / sessions

/// Groups exposed over MCP (config.md: mcpToolGroups; empty = this default).
pub fn mcp_groups(st: &AppState) -> Vec<String> {
    let g = st.settings.read().unwrap().mcp_tool_groups.clone();
    if g.is_empty() {
        ["system", "kernel", "services", "files", "hardware"].iter().map(|s| s.to_string()).collect()
    } else {
        g
    }
}

pub async fn list_tools(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    let groups = st.settings.read().unwrap().tool_groups.clone();
    let mcp = mcp_groups(&st);
    let tools: Vec<Value> = st
        .agent
        .tools()
        .contracts()
        .iter()
            .map(|c| {
                let group = tools::group_of(&c.name);
                json!({
                    "contract": c,
                    "group": group,
                    // An empty toolGroups means every group.
                    "enabled": groups.is_empty() || groups.iter().any(|g| g == group),
                    "mcp": mcp.iter().any(|g| g == group),
                })
            })
            .collect();
    Ok(ok_json(json!({"tools": tools})))
}

/// `GET /api/sessions?parent=<id>` lists a session's children; without `parent`,
/// subagent sessions are not listed.
pub async fn list_sessions(
    State(st): State<Arc<AppState>>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> ApiResult<Response> {
    let parent = q.get("parent").map(String::as_str).filter(|p| !p.is_empty());
    let mut listed = crate::subagents::listed(st.agent.store().list().await.map_err(internal)?, parent);
    for (m, _) in &mut listed {
        m.message_count = st.agent.store().load_messages(&m.id).await.map(|v| v.len()).unwrap_or(0);
    }
    // Each with `agents`, how many subagents it started (the row of a parent says "3 agents").
    let sessions: Vec<Value> = listed.iter().map(|(m, n)| crate::subagents::with_agents(m, *n)).collect();
    Ok(ok_json(json!({"sessions": sessions})))
}

pub async fn create_session(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let title = body.get("title").and_then(Value::as_str).unwrap_or("");
    let meta = st.agent.store().create(title).await.map_err(internal)?;
    Ok((StatusCode::CREATED, Json(meta)).into_response())
}

pub async fn get_session(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let meta = st.agent.store().meta(&id).await.map_err(|e| not_found(format!("{e:#}")))?;
    let messages = st.agent.messages(&id).await.map_err(internal)?;
    Ok(ok_json(json!({"meta": meta, "messages": messages})))
}

pub async fn delete_session(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    // Cancels and deletes the subagents of the session too (agents.md section 2).
    let gone = st.widgets.family(st.agent.store(), &id).await;
    st.agent.delete_session(&id).await.map_err(internal)?;
    // Their inline widgets go with them (widgets.md section 5).
    st.widgets.remove_all(&gone).await;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn post_message(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let text = body
        .get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| bad_request("text required"))?
        .to_string();
    let via = match body.get("via").and_then(Value::as_str) {
        Some("voice") => Via::Voice,
        _ => Via::Text,
    };
    // `/plan <request>` starts the turn in plan mode and loses the prefix (plan-todo.md section 2).
    let (text, plan) = crate::plan_api::split_plan(text, body.get("plan").and_then(Value::as_bool));
    if text.trim().is_empty() {
        return Err(bad_request("text required"));
    }
    let meta = st.agent.store().meta(&id).await.map_err(|e| not_found(format!("{e:#}")))?;
    if meta.origin == kagent_core::session::SessionOrigin::Subagent {
        return Err(HttpError(StatusCode::CONFLICT, "conflict", "the session of a subagent is read-only: talk to the session that started it".into()));
    }
    // A message that starts with `/<skill>` gives its turn that skill's body (skills.md section 6).
    let mut opts = st.skills.turn_opts(&text);
    opts.plan = plan;
    let outcome = st.agent.send_with(&id, text, via, opts).await.map_err(internal)?;
    let body = match outcome {
        SendOutcome::Started { turn_id, .. } => json!({"status": "started", "turnId": turn_id}),
        SendOutcome::Queued => json!({"status": "queued"}),
    };
    Ok(ok_json(body))
}

pub async fn cancel_session(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let cancelled = st.agent.cancel(&id).await;
    Ok(ok_json(json!({"cancelled": cancelled})))
}

pub async fn resolve_approval(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let decision = body.get("decision").and_then(Value::as_str).ok_or_else(|| bad_request("decision required"))?;
    let d = match decision {
        "allowOnce" => ApprovalDecision::AllowOnce,
        "allowSession" => ApprovalDecision::AllowSession,
        "allowAlways" => ApprovalDecision::AllowAlways,
        "deny" => ApprovalDecision::Deny { reason: body.get("reason").and_then(Value::as_str).map(str::to_string) },
        other => return Err(bad_request(format!("unknown decision '{other}'"))),
    };
    if st.agent.broker().resolve(&id, d, kagent_core::permission::ApprovalVia::Click) {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(not_found("approval already resolved or expired"))
    }
}

pub async fn stop_all(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    // Count tool process groups alive now; cancellation kills each group.
    let processes = kagent_core::tools::exec::running_process_groups();
    let cancelled = st.agent.stop_all().await;
    Ok(ok_json(json!({"cancelledTurns": cancelled, "killedProcesses": processes})))
}

/// GET /api/doctor: model-free health check (System view, plasmoid).
pub async fn get_doctor() -> ApiResult<Response> {
    let ctx = kagent_core::tool::ToolContext::standalone(std::time::Duration::from_secs(30), 24_000);
    Ok(ok_json(kagent_core::tools::timeline::checkup(&ctx).await))
}

pub async fn get_system() -> ApiResult<Response> {
    let snapshot = kagent_core::tools::sys::system_snapshot().await;
    Ok(ok_json(snapshot))
}

// ------------------------------------------------------------------ audit (S6)

pub async fn get_audit(State(st): State<Arc<AppState>>, axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>) -> ApiResult<Response> {
    let path = crate::paths::Paths::resolve().data.join("audit.jsonl");
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(100).min(1000);
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let records: Vec<Value> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(limit)
        .collect();
    let _ = st;
    Ok(ok_json(json!({"records": records})))
}

pub async fn verify_audit() -> ApiResult<Response> {
    let path = crate::paths::Paths::resolve().data.join("audit.jsonl");
    match kagent_core::audit::verify(&path) {
        Ok(count) => Ok(ok_json(json!({"ok": true, "count": count}))),
        Err((line, reason)) => Ok(ok_json(json!({"ok": false, "brokenAt": line, "reason": reason}))),
    }
}

// ---------------------------------------------------------------- changes (S8)

pub async fn list_changes(
    State(st): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> ApiResult<Response> {
    let limit = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50usize).min(500);
    Ok(ok_json(serde_json::to_value(st.services.journal.list(limit)).unwrap_or(Value::Null)))
}

/// Undo goes through the normal tool path (approval, audit, events) as `undo_change`.
pub async fn undo_change(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    st.services.journal.get(&id).map_err(|_| not_found(format!("no change {id}")))?;
    let call = kagent_core::protocol::ToolCall {
        id: format!("ui_{}", uuid::Uuid::new_v4().simple()),
        name: "undo_change".into(),
        input: json!({"change_id": id}),
    };
    let privilege = st.settings.read().unwrap().privilege_method();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (msg, status) = st.agent.execute_call("ui", "ui", &call, &cancel, privilege).await;
    let change = st.services.journal.get(&id).map_err(internal)?;
    Ok(ok_json(json!({"status": status, "output": msg.content, "change": change})))
}

// ---------------------------------------------------------------- direct tool calls (MCP, S12)

/// Run one tool through the full agent path (validation, permission, approval card,
/// audit, journal). Used by `kagentd mcp`; MCP-disabled groups are refused here.
pub async fn call_tool(
    State(st): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let group = tools::group_of(&name);
    let session = body.get("sessionId").and_then(Value::as_str).unwrap_or("mcp").to_string();
    if session == "mcp" && !mcp_groups(&st).iter().any(|g| g == group) {
        return Err(HttpError(StatusCode::FORBIDDEN, "not_allowed", format!("tool group '{group}' is not exposed over MCP (settings.mcpToolGroups)")));
    }
    let call = kagent_core::protocol::ToolCall {
        id: format!("{session}_{}", uuid::Uuid::new_v4().simple()),
        name,
        input: body.get("input").cloned().unwrap_or(json!({})),
    };
    let privilege = st.settings.read().unwrap().privilege_method();
    let started = std::time::Instant::now();
    let (msg, status) = st.agent.execute_call(&session, &session, &call, &CancellationToken::new(), privilege).await;
    Ok(ok_json(json!({"status": status, "output": msg.content, "durationMs": started.elapsed().as_millis() as u64})))
}
