//! The calendar routes (calendar.md section 7). The work is in `Calendar`; these handlers
//! parse, answer, and name the status of a refusal. The callback of Google is the one route
//! without the bearer token: it takes a known `state` and a loopback peer only.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::{CalError, Calendar};
use crate::server::{bad_request, ApiResult, HttpError};
use crate::AppState;

fn ok_json(v: Value) -> Response {
    (StatusCode::OK, Json(v)).into_response()
}

/// A refusal of the module as the status of an answer.
pub fn http_error(e: CalError) -> HttpError {
    match e {
        CalError::Setup(m) | CalError::Refused(m) => bad_request(m),
        CalError::NotConnected(m) => HttpError(StatusCode::CONFLICT, "not_connected", m),
        CalError::Upstream(m) => HttpError(StatusCode::BAD_GATEWAY, "calendar_upstream", m),
    }
}

impl Calendar {
    /// `GET /api/calendar`: the state, the account's calendars with the owner's switches, and
    /// whether the secrets exist (never the secrets).
    pub async fn view(&self) -> Value {
        let mut calendars = Vec::new();
        let mut failed = None;
        if self.state() == super::ConnState::Connected {
            match self.account_calendars().await {
                Ok(list) => {
                    let on = self.config().calendars;
                    calendars = list.iter().map(|c| json!({"id": c.id, "name": c.name, "color": c.color, "writable": c.writable, "on": on.iter().any(|id| id == &c.id || (id == "primary" && c.primary))})).collect();
                }
                Err(e) => failed = Some(e.to_string()),
            }
        }
        // The state after the call: a call that found the grant gone has changed it.
        let (state, error) = self.state_and_error();
        let error = failed.or(error);
        let last = self.saved.lock().unwrap().last_report.clone();
        json!({
            "state": state,
            "account": self.account(),
            "calendars": calendars,
            "hasClientSecret": self.has_client_secret(),
            "hasMapsKey": self.has_maps_key().await,
            "lastReport": last,
            "error": error,
        })
    }

    /// `POST /api/calendar/maps-test`: one search for "Ljubljana", on the map and nowhere else.
    pub async fn maps_test(&self) -> Value {
        let Some(key) = self.maps_key().await else { return json!({"ok": false, "error": "no Maps key is set"}) };
        match self.places_search(&key, "Ljubljana", None).await {
            Ok(found) => match found.first() {
                Some(p) => json!({"ok": true, "place": {"name": p.name, "address": p.address}}),
                None => json!({"ok": false, "error": "Google Maps knew the key but found nothing for \"Ljubljana\""}),
            },
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        }
    }
}

pub async fn get(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.calendar.view().await))
}

/// `PUT /api/calendar/client` `{clientId, clientSecret?}`: an empty secret keeps the stored one.
pub async fn put_client(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let id = body.get("clientId").and_then(Value::as_str).ok_or_else(|| bad_request("clientId required"))?;
    let secret = body.get("clientSecret").and_then(Value::as_str);
    st.calendar.set_client(id, secret).await.map_err(http_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /api/calendar/connect`: `{url}`; the page opens it in the browser.
pub async fn connect(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(json!({"url": st.calendar.connect().map_err(http_error)?})))
}

pub async fn disconnect(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    st.calendar.disconnect().await.map_err(http_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `PUT /api/calendar/maps-key` `{key}`: `null` forgets it.
pub async fn put_maps_key(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let key = match body.get("key") {
        Some(Value::String(k)) => Some(k.as_str()),
        Some(Value::Null) => None,
        _ => return Err(bad_request("key must be text, or null to forget it")),
    };
    st.calendar.set_maps_key(key).await.map_err(http_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn maps_test(State(st): State<Arc<AppState>>) -> ApiResult<Response> {
    Ok(ok_json(st.calendar.maps_test().await))
}

/// `POST /api/calendar/report` `{deliver}`: the text, and where it went.
pub async fn report(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    let deliver = body.get("deliver").and_then(Value::as_bool).unwrap_or(false);
    let (text, sent) = st.calendar.report_now(deliver).await.map_err(http_error)?;
    Ok(ok_json(json!({"text": text, "sent": sent})))
}

#[derive(serde::Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// `GET /api/calendar/oauth/callback`: no bearer token (Google sends the owner's browser here),
/// so it takes a loopback peer and a known `state` only.
pub async fn callback(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Query(q): Query<CallbackQuery>) -> Response {
    page(callback_page(&st.calendar, peer.ip(), q).await)
}

pub async fn callback_page(cal: &Calendar, peer: IpAddr, q: CallbackQuery) -> (StatusCode, String) {
    if !peer.is_loopback() {
        return (StatusCode::FORBIDDEN, message("Refused", "This address answers on this machine only."));
    }
    if let Some(error) = q.error {
        return (StatusCode::BAD_REQUEST, message("Not connected", &format!("Google did not connect KAgent ({error}). You can close this tab and try again in Settings -> Calendar.")));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return (StatusCode::BAD_REQUEST, message("Not connected", "This link is not a sign-in answer from Google."));
    };
    match cal.callback(&code, &state).await {
        Ok(()) => (StatusCode::OK, message("Connected", "Google Calendar is connected. You can close this tab.")),
        Err(e) => (StatusCode::BAD_REQUEST, message("Not connected", &e.to_string())),
    }
}

fn page((status, body): (StatusCode, String)) -> Response {
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")], body).into_response()
}

/// A small page; the text is escaped.
fn message(title: &str, text: &str) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>KAgent: {t}</title>\
         <style>body{{font:16px system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 1rem;color:#222;background:#fafafa}}@media(prefers-color-scheme:dark){{body{{color:#ddd;background:#181818}}}}</style></head>\
         <body><h1>{t}</h1><p>{m}</p></body></html>",
        t = escape(title),
        m = escape(text)
    )
}
