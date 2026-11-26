//! HTTP API handlers (docs/specs/api.md section 3, phase-1 subset).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use kagent_core::agent::SendOutcome;
use kagent_core::model::ModelClient as _;
use kagent_core::permission::ApprovalDecision;
use kagent_core::protocol::{AgentEvent, Via};
use kagent_core::tools;

use crate::server::{bad_request, not_found, ApiResult, HttpError};
use crate::AppState;

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
    let chat_valid = st.providers.resolve(&st.settings.read().unwrap().chat_model).is_ok();
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
    // A live (speech-to-speech) model cannot answer text turns, and the reverse.
    for (key, want_live) in [("chatModel", false), ("fastModel", false), ("fallbackModel", false), ("liveModel", true)] {
        let Some(r) = patch.get(key).and_then(Value::as_str).filter(|r| !r.is_empty()) else { continue };
        if let Ok(m) = st.providers.resolve(r) {
            let live = m.config.supports_live || (!m.config.roles.is_empty() && m.config.roles.iter().all(|x| x == "live")) || m.model.id.contains("-live") || m.model.id.contains("native-audio");
            if live != want_live {
                return Err(bad_request(if want_live {
                    format!("{r} is not a live voice model; pick a Gemini *-live model")
                } else {
                    format!("{r} is a live voice model and cannot answer text chat; pick a text model (e.g. a Gemini flash model) and set it as the Voice tab's live model instead")
                }));
            }
        }
    }
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

// ------------------------------------------------------------------ providers

pub async fn provider_templates(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(json!({"templates": st.providers.templates()})))
}

pub async fn list_providers(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(json!({"providers": st.providers.masked()})))
}

pub async fn add_provider(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let template_id = body.get("templateId").and_then(Value::as_str).ok_or_else(|| bad_request("templateId required"))?;
    let name = body.get("name").and_then(Value::as_str).map(str::to_string);
    let base_url = body.get("baseUrl").and_then(Value::as_str).map(str::to_string);
    let api_key = body.get("apiKey").and_then(Value::as_str).map(str::to_string);
    let headers: BTreeMap<String, String> = body
        .get("headers")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
        .unwrap_or_default();
    let p = st
        .providers
        .add_from_template(template_id, name, base_url, api_key, headers)
        .map_err(|e| HttpError(StatusCode::BAD_REQUEST, "invalid_input", format!("{e:#}")))?;
    Ok((StatusCode::CREATED, Json(serde_json::to_value(p).unwrap_or(Value::Null))).into_response())
}

pub async fn provider_order(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let order: Vec<String> = body
        .get("order")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .ok_or_else(|| bad_request("order array required"))?;
    st.providers.set_order(order).map_err(internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn update_provider(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let p = st
        .providers
        .update(&id, |p| {
            if let Some(v) = body.get("name").and_then(Value::as_str) {
                p.name = v.to_string();
            }
            if let Some(v) = body.get("baseUrl").and_then(Value::as_str) {
                p.api.base_url = v.to_string();
            }
            if let Some(v) = body.get("enabled").and_then(Value::as_bool) {
                p.enabled = v;
            }
            // "" = keep the stored key; null = clear it.
            match body.get("apiKey") {
                Some(Value::Null) => p.api_key = String::new(),
                Some(Value::String(s)) if !s.is_empty() => p.api_key = s.clone(),
                _ => {}
            }
            if let Some(h) = body.get("headers").and_then(Value::as_object) {
                p.api.headers = h.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect();
            }
            if let Some(models) = body.get("models").and_then(Value::as_array) {
                for m in models {
                    let Some(mid) = m.get("id").and_then(Value::as_str) else { continue };
                    if let Some(entry) = p.models.iter_mut().find(|e| e.id == mid) {
                        if let Some(e) = m.get("enabled").and_then(Value::as_bool) {
                            entry.enabled = e;
                        }
                        if let Some(roles) = m.get("roles").and_then(Value::as_array) {
                            entry.roles = roles.iter().filter_map(Value::as_str).map(str::to_string).collect();
                        }
                    }
                }
            }
        })
        .map_err(|e| not_found(format!("{e:#}")))?;
    Ok(ok_json(serde_json::to_value(p).unwrap_or(Value::Null)))
}

pub async fn delete_provider(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let had_chat = st
        .settings
        .read()
        .unwrap()
        .chat_model
        .starts_with(&format!("{id}::"));
    st.providers.delete(&id).map_err(|e| not_found(format!("{e:#}")))?;
    if had_chat {
        let mut s = st.settings.write().unwrap();
        s.chat_model = String::new();
        s.save(&crate::paths::Paths::resolve().settings()).map_err(internal)?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn fetch_models(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> ApiResult<Response> {
    let provider = st.providers.provider(&id).ok_or_else(|| not_found(format!("unknown provider {id}")))?;
    match kagent_providers::discovery::fetch_models(&provider).await {
        Ok(ids) => {
            let merged = kagent_providers::discovery::merge_models(&provider, ids);
            let updated = st.providers.update(&id, |p| {
                *p = merged;
                p.models_fetched_at = Some(chrono::Utc::now().to_rfc3339());
            });
            match updated {
                Ok(p) => Ok(ok_json(serde_json::to_value(p).unwrap_or(Value::Null))),
                Err(e) => Err(internal(e)),
            }
        }
        Err(e) => {
            st.providers.set_error(&id, &format!("{e:#}"));
            Err(HttpError(
                StatusCode::BAD_GATEWAY,
                "provider_error",
                format!("{e:#}"),
            ))
        }
    }
}

pub async fn test_provider(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> ApiResult<Response> {
    let model_id = body.get("modelId").and_then(Value::as_str).map(str::to_string);
    let provider = st.providers.provider(&id).ok_or_else(|| not_found(format!("unknown provider {id}")))?;
    let mid = model_id
        .or_else(|| provider.models.first().map(|m| m.id.clone()))
        .ok_or_else(|| bad_request("provider has no models; pass modelId"))?;
    let start = std::time::Instant::now();
    let client = kagent_providers::ProvidersClient::new(st.providers.clone());
    let cancel = CancellationToken::new();
    let request = kagent_core::model::ModelRequest {
        model_ref: format!("{id}::{mid}"),
        system: String::new(),
        messages: vec![kagent_core::protocol::Message::user("Reply with exactly: OK")],
        tools: vec![],
        options: Default::default(),
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), client.complete(request, tx, cancel)).await;
    let latency = start.elapsed().as_millis() as u64;
    let body = match result {
        Ok(Ok(r)) => json!({"ok": true, "latencyMs": latency, "reply": r.text}),
        Ok(Err(e)) => {
            st.providers.set_error(&id, &format!("{e:#}"));
            json!({"ok": false, "latencyMs": latency, "error": format!("{e:#}")})
        }
        Err(_) => json!({"ok": false, "latencyMs": latency, "error": "timed out after 30 s"}),
    };
    Ok(ok_json(body))
}

pub async fn list_models(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    let mut out: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for role in ["chat", "fast", "live", "vision"] {
        out.insert(role, Vec::new());
    }
    for (pid, model, config) in st.providers.enabled_models() {
        let view = json!({
            "id": model.id,
            "name": model.name,
            "ref": format!("{pid}::{}", model.id),
            "config": config,
        });
        let roles = if model.roles.is_empty() { vec!["chat".to_string()] } else { model.roles.clone() };
        for role in ["chat", "fast", "live", "vision"] {
            if roles.iter().any(|r| r == role) || (role == "chat" && roles.iter().any(|r| r == "chat")) {
                out.get_mut(role).unwrap().push(view.clone());
            }
        }
    }
    Ok(ok_json(Value::Object(out.into_iter().map(|(k, v)| (k.to_string(), Value::Array(v))).collect())))
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
    let mut metas = kagent_core::session::SessionStore::visible(st.agent.store().list().await.map_err(internal)?, parent);
    for m in &mut metas {
        m.message_count = st.agent.store().load_messages(&m.id).await.map(|v| v.len()).unwrap_or(0);
    }
    Ok(ok_json(json!({"sessions": metas})))
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
    st.agent.cancel(&id).await;
    st.agent.forget(&id);
    st.agent.store().delete(&id).await.map_err(internal)?;
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
    st.agent.store().meta(&id).await.map_err(|e| not_found(format!("{e:#}")))?;
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
    if st.agent.broker().resolve(&id, d) {
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
