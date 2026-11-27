//! The Calendar API of the fake Google (`fake.rs`): calendarList and events, with paging,
//! a time range, `q`, Meet requests, the `sendUpdates` the caller asked for (kept in the
//! request log) and a 410 for an event that was deleted.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, NaiveDate};
use serde_json::{json, Value};

use super::fake::{authorised, Shared};

pub const OWNER: &str = "owner@example.com";
pub const FAMILY: &str = "family@group.calendar.google.com";
pub const HOLIDAYS: &str = "sl.slovenian#holiday@group.v.calendar.google.com";
pub const WORK: &str = "work@example.com";

pub fn calendars() -> Vec<Value> {
    vec![
        json!({"id": OWNER, "summary": OWNER, "summaryOverride": "Personal", "primary": true, "accessRole": "owner", "backgroundColor": "#9fe1e7"}),
        json!({"id": FAMILY, "summary": "Family", "accessRole": "writer", "backgroundColor": "#f6bf26"}),
        json!({"id": HOLIDAYS, "summary": "Holidays in Slovenia", "accessRole": "reader", "backgroundColor": "#0b8043"}),
        json!({"id": WORK, "summary": "Work", "accessRole": "owner", "backgroundColor": "#d50000"}),
    ]
}

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/calendar/v3/users/me/calendarList", get(calendar_list))
        .route("/calendar/v3/calendars/{cal}/events", get(list).post(insert))
        .route("/calendar/v3/calendars/{cal}/events/{id}", get(one).patch(patch).delete(remove))
}

fn problem(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error": {"code": status.as_u16(), "message": message, "status": status.canonical_reason()}}))).into_response()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|h| h.strip_prefix("Bearer ")).map(String::from)
}

fn unauthorised() -> Response {
    problem(StatusCode::UNAUTHORIZED, "Invalid Credentials")
}

fn real_id(cal: &str) -> String {
    if cal == "primary" { OWNER.to_string() } else { cal.to_string() }
}

/// When an event starts, as a number that sorts (a date counts from its midnight, UTC).
fn start_key(e: &Value) -> i64 {
    instant(&e["start"]).unwrap_or(0)
}

fn instant(v: &Value) -> Option<i64> {
    match v.get("dateTime").and_then(Value::as_str) {
        Some(t) => DateTime::parse_from_rfc3339(t).ok().map(|t| t.timestamp()),
        None => v.get("date").and_then(Value::as_str).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()),
    }
}

async fn calendar_list(State(state): State<Shared>, headers: HeaderMap) -> Response {
    let f = state.lock().unwrap();
    if !authorised(&f, &bearer(&headers)) {
        return unauthorised();
    }
    let all = &f.calendars;
    // Two pages, to show that the client reads them all.
    Json(json!({"items": all})).into_response()
}

async fn list(State(state): State<Shared>, Path(cal): Path<String>, Query(q): Query<HashMap<String, String>>, headers: HeaderMap) -> Response {
    let f = state.lock().unwrap();
    if !authorised(&f, &bearer(&headers)) {
        return unauthorised();
    }
    let id = real_id(&cal);
    if !f.calendars.iter().any(|c| c["id"] == id.as_str()) {
        return problem(StatusCode::NOT_FOUND, "Not Found");
    }
    let parse = |k: &str| q.get(k).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.timestamp());
    let (min, max) = (parse("timeMin").unwrap_or(i64::MIN), parse("timeMax").unwrap_or(i64::MAX));
    let needle = q.get("q").map(|t| t.to_lowercase());
    let mut found: Vec<&Value> = f
        .events
        .iter()
        .filter(|(c, e)| *c == id && e["status"] != "cancelled")
        .map(|(_, e)| e)
        .filter(|e| {
            let (s, t) = (instant(&e["start"]).unwrap_or(0), instant(&e["end"]).unwrap_or(0));
            s < max && t > min
        })
        .filter(|e| needle.as_ref().is_none_or(|n| ["summary", "location", "description"].iter().any(|k| e[*k].as_str().is_some_and(|t| t.to_lowercase().contains(n)))))
        .collect();
    found.sort_by_key(|e| start_key(e));
    let from: usize = q.get("pageToken").and_then(|t| t.parse().ok()).unwrap_or(0);
    let size = if f.page_size == 0 { found.len().max(1) } else { f.page_size };
    let page: Vec<&Value> = found.iter().skip(from).take(size).copied().collect();
    let mut body = json!({"items": page, "summary": id});
    if from + size < found.len() {
        body["nextPageToken"] = json!((from + size).to_string());
    }
    Json(body).into_response()
}

async fn one(State(state): State<Shared>, Path((cal, id)): Path<(String, String)>, headers: HeaderMap) -> Response {
    let f = state.lock().unwrap();
    if !authorised(&f, &bearer(&headers)) {
        return unauthorised();
    }
    match f.events.iter().find(|(c, e)| *c == real_id(&cal) && e["id"] == id.as_str()) {
        Some((_, e)) if e["status"] == "cancelled" => problem(StatusCode::GONE, "Resource has been deleted"),
        Some((_, e)) => Json(e).into_response(),
        None => problem(StatusCode::NOT_FOUND, "Not Found"),
    }
}

async fn insert(State(state): State<Shared>, Path(cal): Path<String>, Query(q): Query<HashMap<String, String>>, headers: HeaderMap, Json(mut body): Json<Value>) -> Response {
    let mut f = state.lock().unwrap();
    if !authorised(&f, &bearer(&headers)) {
        return unauthorised();
    }
    let id = real_id(&cal);
    match f.calendars.iter().find(|c| c["id"] == id.as_str()) {
        None => return problem(StatusCode::NOT_FOUND, "Not Found"),
        Some(c) if !matches!(c["accessRole"].as_str(), Some("owner" | "writer")) => return problem(StatusCode::FORBIDDEN, "You need to have writer access to this calendar."),
        _ => {}
    }
    f.next_event += 1;
    let event_id = format!("evt{}", f.next_event);
    body["id"] = json!(event_id);
    body["status"] = json!("confirmed");
    body["htmlLink"] = json!(format!("https://calendar.google.com/calendar/event?eid={event_id}"));
    if q.get("conferenceDataVersion").map(String::as_str) == Some("1") && body.pointer("/conferenceData/createRequest").is_some() {
        body["hangoutLink"] = json!("https://meet.google.com/fak-eeve-nts");
        body["conferenceData"] = json!({"entryPoints": [{"entryPointType": "video", "uri": "https://meet.google.com/fak-eeve-nts"}]});
    }
    f.events.push((id, body.clone()));
    Json(body).into_response()
}

async fn patch(State(state): State<Shared>, Path((cal, id)): Path<(String, String)>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let mut f = state.lock().unwrap();
    if !authorised(&f, &bearer(&headers)) {
        return unauthorised();
    }
    let cal_id = real_id(&cal);
    let Some((_, event)) = f.events.iter_mut().find(|(c, e)| *c == cal_id && e["id"] == id.as_str()) else { return problem(StatusCode::NOT_FOUND, "Not Found") };
    if event["status"] == "cancelled" {
        return problem(StatusCode::GONE, "Resource has been deleted");
    }
    for (k, v) in body.as_object().into_iter().flatten() {
        if v.is_null() {
            event.as_object_mut().unwrap().remove(k);
        } else {
            event[k] = v.clone();
        }
    }
    Json(event.clone()).into_response()
}

async fn remove(State(state): State<Shared>, Path((cal, id)): Path<(String, String)>, headers: HeaderMap) -> Response {
    let mut f = state.lock().unwrap();
    if !authorised(&f, &bearer(&headers)) {
        return unauthorised();
    }
    let cal_id = real_id(&cal);
    let Some((_, event)) = f.events.iter_mut().find(|(c, e)| *c == cal_id && e["id"] == id.as_str()) else { return problem(StatusCode::NOT_FOUND, "Not Found") };
    if event["status"] == "cancelled" {
        return problem(StatusCode::GONE, "Resource has been deleted");
    }
    event["status"] = json!("cancelled");
    StatusCode::NO_CONTENT.into_response()
}
