//! The routes of the tickets (tickets.md section 4, api.md section 12). The work is in functions over the store,
//! tested without a server; the handlers wrap them.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use super::{ops::Listing, By, Create, Filter, State as TState, TicketError, Tickets, Update};
use crate::server::{ApiResult, HttpError};
use crate::AppState;

impl From<TicketError> for HttpError {
    fn from(e: TicketError) -> Self {
        match e {
            TicketError::Invalid(m) => HttpError(StatusCode::BAD_REQUEST, "invalid_input", m),
            TicketError::NotFound(m) => HttpError(StatusCode::NOT_FOUND, "not_found", m),
            TicketError::Limit(m) => HttpError(StatusCode::CONFLICT, "conflict", m),
            TicketError::Failed(m) => HttpError(StatusCode::INTERNAL_SERVER_ERROR, "internal", m),
        }
    }
}

fn shape_error(what: &str, e: serde_json::Error) -> HttpError {
    HttpError(StatusCode::BAD_REQUEST, "invalid_input", format!("{what}: {e}"))
}

/// The filter of `GET /api/tickets` from its query.
pub fn filter_of(q: &HashMap<String, String>) -> Result<Filter, TicketError> {
    let state = match q.get("state").map(String::as_str).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(serde_json::from_value::<TState>(Value::String(s.into())).map_err(|_| {
            super::invalid(format!("state: one of {}", TState::ALL.iter().map(|s| s.word()).collect::<Vec<_>>().join(", ")))
        })?),
    };
    let get = |k: &str| q.get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    Ok(Filter { project: get("project"), state, label: get("label"), text: get("q") })
}

pub fn listing_json(l: &Listing) -> Value {
    json!({"projects": l.projects, "tickets": l.tickets})
}

/// `POST /api/tickets`: the folder must exist (the check of sidebar.md section 3); it is stored canonical.
pub async fn create(tickets: &Arc<Tickets>, body: &Value) -> Result<super::Ticket, HttpError> {
    let mut input: Create = serde_json::from_value(body.clone()).map_err(|e| shape_error("a ticket is {title, description?, state?, priority?, labels?, parent?, links?, assignee?, doneWhen?, project?}", e))?;
    if let Some(p) = input.project.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        input.project = Some(crate::project_api::check(p).await?.to_string_lossy().to_string());
    } else {
        input.project = None;
    }
    Ok(tickets.create(input, By::Owner)?)
}

pub fn change(tickets: &Tickets, key: &str, body: &Value) -> Result<super::Ticket, HttpError> {
    let input: Update = serde_json::from_value(body.clone()).map_err(|e| shape_error("expected any of title, description, state, priority, labels, parent, links, assignee, doneWhen, note", e))?;
    Ok(tickets.update(key, input, By::Owner)?.0)
}

pub async fn get(State(st): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> ApiResult<Response> {
    let filter = filter_of(&q)?;
    Ok(Json(listing_json(&st.tickets.list(&filter))).into_response())
}

pub async fn post(State(st): State<Arc<AppState>>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok((StatusCode::CREATED, Json(create(&st.tickets, &body).await?)).into_response())
}

pub async fn put(State(st): State<Arc<AppState>>, Path(key): Path<String>, Json(body): Json<Value>) -> ApiResult<Response> {
    Ok(Json(change(&st.tickets, &key, &body)?).into_response())
}

pub async fn delete(State(st): State<Arc<AppState>>, Path(key): Path<String>) -> ApiResult<Response> {
    st.tickets.delete(&key)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
