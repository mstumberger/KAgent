//! HTTP server: router, Host check, bearer auth, static web/dist.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post, put};
use axum::Router;
use serde_json::json;

use crate::api;
use crate::ws;
use crate::AppState;

/// Constant-time token compare (no early exit on mismatch).
pub fn token_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn error_body(code: &str, message: impl Into<String>) -> String {
    json!({"error": {"code": code, "message": message.into()}}).to_string()
}

pub struct HttpError(pub StatusCode, pub &'static str, pub String);

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.0,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            error_body(self.1, self.2),
        )
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, HttpError>;

pub fn bad_request(msg: impl Into<String>) -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_input", msg.into())
}
pub fn not_found(msg: impl Into<String>) -> HttpError {
    HttpError(StatusCode::NOT_FOUND, "not_found", msg.into())
}

async fn host_check(req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h == "127.0.0.1:7878" || h == "localhost:7878");
    if !ok {
        return (
            StatusCode::MISDIRECTED_REQUEST,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            error_body("misdirected", "Host header must be 127.0.0.1:7878 or localhost:7878"),
        )
            .into_response();
    }
    next.run(req).await
}

async fn bearer_auth(state: axum::extract::State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let token = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    match token {
        Some(t) if token_eq(t, &state.token) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            error_body("unauthorized", "missing or wrong bearer token"),
        )
            .into_response(),
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let api_routes = Router::new()
        .route("/state", get(api::get_state))
        .route("/settings", put(api::put_settings))
        .route("/providers/templates", get(api::provider_templates))
        .route("/providers", get(api::list_providers).post(api::add_provider))
        .route("/providers/order", put(api::provider_order))
        .route("/providers/{id}", put(api::update_provider).delete(api::delete_provider))
        .route("/providers/{id}/fetch-models", post(api::fetch_models))
        .route("/providers/{id}/test", post(api::test_provider))
        .route("/models", get(api::list_models))
        .route("/tools", get(api::list_tools))
        .route("/tools/{name}/call", post(api::call_tool))
        .route("/sessions", get(api::list_sessions).post(api::create_session))
        .route("/sessions/{id}", get(api::get_session).delete(api::delete_session))
        .route("/sessions/{id}/messages", post(api::post_message))
        .route("/sessions/{id}/cancel", post(api::cancel_session))
        .route("/sessions/{id}/compact", post(crate::compact_api::compact_session))
        .route("/sessions/{id}/todo", put(crate::todo_api::put_todo))
        .route("/sessions/{id}/plan", post(crate::plan_api::post_plan))
        .route("/sessions/{id}/model", put(crate::model_api::put_model))
        .route("/sessions/{id}/project", put(crate::project_api::put_project))
        .route("/sessions/{id}/pin", put(crate::project_api::put_pin))
        .route("/projects", get(crate::project_api::get_projects))
        .route("/approvals/{id}", post(api::resolve_approval))
        .route("/stop-all", post(api::stop_all))
        .route("/system", get(api::get_system))
        .route("/doctor", get(api::get_doctor))
        .route("/audit", get(api::get_audit))
        .route("/audit/verify", get(api::verify_audit))
        .route("/alerts", get(crate::watch::http::list))
        .route("/alerts/{id}/ack", post(crate::watch::http::ack))
        .route("/alerts/{id}/investigate", post(crate::watch::http::investigate))
        .route("/net", get(crate::net::tools::http::status))
        .route("/net/router", put(crate::net::tools::http::configure))
        .route("/net/devices", get(crate::net::tools::http::devices))
        .route("/net/top", get(crate::net::tools::http::top))
        .route("/net/unblock", post(crate::net::tools::http::unblock))
        .route("/sentinel", get(crate::sentinel::http::status))
        .route("/sentinel/honeytokens", post(crate::sentinel::http::honeytokens))
        .route("/sentinel/rebaseline", post(crate::sentinel::http::rebaseline))
        .route("/changes", get(api::list_changes))
        .route("/changes/{id}/undo", post(api::undo_change))
        .route("/tasks", get(crate::tasks::http::list).post(crate::tasks::http::add))
        .route("/tasks/{id}", put(crate::tasks::http::update).delete(crate::tasks::http::delete))
        .route("/tasks/{id}/run", post(crate::tasks::http::run))
        .route("/tasks/{id}/runs", get(crate::tasks::http::runs))
        .route("/tasks/{id}/cancel", post(crate::tasks::http::cancel))
        .route("/tasks/templates", get(crate::tasks::http::templates))
        .route("/tasks/templates/{id}", post(crate::tasks::http::add_template))
        .route("/tasks/preview", get(crate::tasks::http::preview))
        .route("/skills", get(crate::skills_api::list).post(crate::skills_api::create_skill))
        .route("/skills/rescan", post(crate::skills_api::rescan))
        .route("/skills/{name}", get(crate::skills_api::get_one).put(crate::skills_api::update_skill).delete(crate::skills_api::delete_skill))
        .route("/skills/{name}/enabled", put(crate::skills_api::set_enabled))
        .route("/skills/import-sources", get(crate::skills_sources_api::import_sources))
        .route("/skills/import", post(crate::skills_sources_api::import))
        .route("/skills/sources", get(crate::skills_sources_api::list_sources).post(crate::skills_sources_api::add_source))
        .route("/skills/sources/{id}", delete(crate::skills_sources_api::remove_source))
        .route("/skills/sources/{id}/sync", post(crate::skills_sources_api::sync_source))
        .route("/memory", get(crate::memory::http::list).post(crate::memory::http::create))
        .route("/memory/search", get(crate::memory::http::search))
        .route("/memory/{name}", put(crate::memory::http::update).delete(crate::memory::http::delete))
        .route("/memory/{name}/forget", post(crate::memory::http::forget))
        .route("/ui/report", post(crate::ui::http::report))
        .route("/ui/window", post(crate::ui::http::window))
        .route("/ui/frame", post(crate::ui::http::frame))
        .route("/ui/snapshot/{id}", post(crate::ui::http::snapshot))
        .route("/voice/levels", get(crate::voice_levels::http::levels))
        .route("/voice/demo", post(crate::voice_levels::http::demo))
        .route("/browser/status", get(crate::browser::http::status))
        .route("/browser/attach/connect", post(crate::browser::http::connect))
        .route("/browser/attach/disconnect", post(crate::browser::http::disconnect))
        .route("/telegram", get(crate::bots::http::get).put(crate::bots::http::put))
        .route("/telegram/token", post(crate::bots::http::set_token).delete(crate::bots::http::delete_token))
        .route("/telegram/pairing-code", post(crate::bots::http::pairing_code))
        .route("/telegram/pairing", delete(crate::bots::http::unpair))
        .route("/telegram/test", post(crate::bots::http::test_message))
        .route("/calendar", get(crate::calendar::http::get))
        .route("/calendar/client", put(crate::calendar::http::put_client))
        .route("/calendar/connect", post(crate::calendar::http::connect))
        .route("/calendar/disconnect", post(crate::calendar::http::disconnect))
        .route("/calendar/maps-key", put(crate::calendar::http::put_maps_key))
        .route("/calendar/maps-test", post(crate::calendar::http::maps_test))
        .route("/calendar/report", post(crate::calendar::http::report))
        .route("/web/brave-key", get(crate::web::http::get_key).put(crate::web::http::put_key).delete(crate::web::http::delete_key))
        .route("/agents", get(crate::subagents::http::list))
        .route("/agents/{name}", put(crate::subagents::http::put_profile).delete(crate::subagents::http::delete))
        .route("/sessions/{id}/subagents/{child}/stop", post(crate::subagents::http::stop))
        .route("/hooks", get(crate::hooks::http::list).post(crate::hooks::http::create))
        .route("/hooks/{id}", put(crate::hooks::http::update).delete(crate::hooks::http::delete))
        .route("/hooks/{id}/test", post(crate::hooks::http::test))
        .route("/widgets/{session}/{id}", get(crate::widgets::get_widget))
        .layer(middleware::from_fn_with_state(state.clone(), bearer_auth));

    // The one calendar route without the bearer token: Google sends the owner's browser here
    // (calendar.md section 1). It takes a known `state` and a loopback peer only.
    let app = Router::new()
        .route("/api/calendar/oauth/callback", get(crate::calendar::http::callback))
        .nest("/api", api_routes)
        .route("/ws", any(ws::ws_handler))
        .route("/live", any(crate::live::ws_live));

    // Static web UI when web/dist exists (SPA fallback to index.html, no auth: the app
    // reads the token from the URL fragment). Added before the layers below so the Host
    // check covers static files too (DNS rebinding).
    let dist = crate::paths::Paths::resolve().web_dist();
    let app = if dist.is_dir() {
        let index = dist.join("index.html");
        let serve_dir = tower_http::services::ServeDir::new(&dist)
            .not_found_service(tower_http::services::ServeFile::new(index));
        app.fallback_service(serve_dir)
    } else {
        app
    };
    app.layer(middleware::from_fn(host_check))
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state)
}

pub async fn serve(state: Arc<AppState>) -> anyhow::Result<()> {
    let app = router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:7878").await?;
    {
        let s = state.settings.read().unwrap();
        crate::startup::greet(&s.startup.greeting, &s.persona);
    }
    let url = format!("http://127.0.0.1:7878/#token={}", state.token);
    println!("kagentd {} listening on http://127.0.0.1:7878", state.version);
    // The token is printed to a terminal only: under systemd stdout is the journal, which the
    // agent can read (token-guard.md).
    if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        println!("open {url}");
    } else {
        println!("open http://127.0.0.1:7878/ (the token is in KAgent's config folder, file `token`)");
    }
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).with_graceful_shutdown(shutdown_signal()).await?;
    Ok(())
}

/// Ctrl+C or SIGTERM (systemd stop): finish in-flight requests, then return.
async fn shutdown_signal() {
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = term => {},
    }
    tracing::info!("shutdown requested");
}
