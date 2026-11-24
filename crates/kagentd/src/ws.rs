//! WebSocket /ws?token=: streams AgentEvents to UI clients.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use futures::SinkExt;
use serde_json::json;

use crate::server::token_eq;
use crate::AppState;

pub async fn ws_handler(
    State(st): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = params.get("token").map(String::as_str).unwrap_or("");
    if !token_eq(token, &st.token) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| client_loop(st, socket))
}

async fn client_loop(st: Arc<AppState>, mut socket: WebSocket) {
    // Hello: version + current pending approvals so a reconnecting UI catches up.
    let hello = json!({
        "type": "hello",
        "version": st.version,
        "pendingApprovals": st.agent.broker().pending(),
    });
    if socket.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }

    let mut rx = st.events.subscribe();
    let mut last_seen = Instant::now();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(20));
    ping_interval.tick().await; // first tick fires immediately; skip it
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    let Ok(text) = serde_json::to_string(&ev) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let notice = json!({"type": "notice", "level": "warn", "text": format!("{n} events dropped, reload the session")});
                    if socket.send(Message::Text(notice.to_string().into())).await.is_err() {
                        return;
                    }
                }
                Err(_) => return, // broadcast closed: daemon is shutting down
            },
            _ = ping_interval.tick() => {
                if last_seen.elapsed() > Duration::from_secs(60) {
                    let _ = socket.close().await;
                    return;
                }
                if socket.send(Message::Ping(vec![].into())).await.is_err() {
                    return;
                }
            }
            frame = socket.recv() => match frame {
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Ping(_))) => last_seen = Instant::now(),
                Some(Ok(Message::Close(_))) | None => return,
                Some(Ok(_)) => {} // clients only listen
                Some(Err(_)) => return,
            },
        }
    }
}
