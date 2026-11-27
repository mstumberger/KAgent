//! Google's Calendar API (calendar.md sections 3 and 5): the calendar list and the events.
//! Every call goes through `api`, which adds the access token, tries again once on a 401
//! and turns a failure into words that say what to do. The switched-on rule is enforced
//! here, at the lowest level: a calendar the owner did not switch on is not asked, and a
//! read-only one is not written, whoever calls.

use chrono::{DateTime, FixedOffset};
use reqwest::Method;
use serde_json::{json, Value};

use super::model::{CalInfo, Event};
use super::oauth::urlencode;
use super::{CalError, Calendar};

/// Pages of events read for one calendar and one question.
const MAX_PAGES: usize = 10;

/// What `api` answers: Google's JSON (`Null` for an empty body).
pub type ApiResult = Result<Value, CalError>;

impl Calendar {
    /// One call to the Calendar API. `path` starts with `/calendar/v3`.
    pub(crate) async fn api(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> ApiResult {
        let mut retried = false;
        loop {
            let token = self.access_token().await?;
            let mut request = self.http.request(method.clone(), format!("{}{path}", self.hosts.api)).bearer_auth(&token).query(query);
            if let Some(body) = body {
                request = request.json(body);
            }
            let response = request.send().await.map_err(|e| CalError::Upstream(format!("Google Calendar did not answer: {}", super::oauth::scrub(&e.without_url().to_string(), &[&token]))))?;
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if status.as_u16() == 401 && !retried {
                // The access token ended early or was revoked: ask for a new one, once.
                retried = true;
                self.forget_access();
                continue;
            }
            if status.is_success() {
                return Ok(serde_json::from_str(&text).unwrap_or(Value::Null));
            }
            return Err(self.failure(status.as_u16(), &text, &token));
        }
    }

    /// Google's refusal in words, without the token.
    fn failure(&self, status: u16, body: &str, token: &str) -> CalError {
        let said = serde_json::from_str::<Value>(body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(String::from)).unwrap_or_default();
        let said = super::oauth::scrub(&said, &[token]);
        match status {
            401 => CalError::NotConnected("Google no longer accepts the access: the owner opens Settings -> Calendar and presses Reconnect.".into()),
            403 if said.contains("not been used") || said.contains("disabled") => CalError::Setup(format!("the Google Calendar API is not enabled in the owner's Google Cloud project ({said}): enable it in the console, then try again")),
            403 => CalError::Upstream(format!("Google refused this ({said}); the account may not be allowed to do it")),
            404 => CalError::Refused("Google has no such calendar or event (it may have been deleted or moved)".into()),
            410 => CalError::Refused("that event was already deleted".into()),
            429 => CalError::Upstream("Google says too many requests: wait a minute and try again".into()),
            s if s >= 500 => CalError::Upstream(format!("Google Calendar had an error ({s}): try again in a moment")),
            s => CalError::Upstream(format!("Google Calendar answered {s}: {said}")),
        }
    }

    /// The calendars of the account, each with the switch of the owner not yet applied.
    /// Also learns the address of the account (the id of the primary calendar).
    pub async fn account_calendars(&self) -> Result<Vec<CalInfo>, CalError> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("maxResults", "250".to_string())];
            query.extend(page.take().map(|p| ("pageToken", p)));
            let answer = self.api(Method::GET, "/calendar/v3/users/me/calendarList", &query, None).await?;
            out.extend(answer.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(CalInfo::from_json));
            match answer.get("nextPageToken").and_then(Value::as_str) {
                Some(next) => page = Some(next.to_string()),
                None => break,
            }
        }
        if let Some(primary) = out.iter().find(|c| c.primary) {
            self.set_account(&primary.id);
        }
        Ok(out)
    }

    /// The calendars the owner switched on (`calendar.calendars`; `primary` names the primary
    /// one), in the order of the account. Every other calendar does not exist for the agent.
    pub async fn switched_on(&self) -> Result<Vec<CalInfo>, CalError> {
        let on = self.config().calendars;
        let all = self.account_calendars().await?;
        let found: Vec<CalInfo> = all.into_iter().filter(|c| on.iter().any(|id| id == &c.id || (id == "primary" && c.primary))).collect();
        if found.is_empty() {
            return Err(CalError::Setup("no calendar is switched on that exists in this account: the owner chooses them in Settings -> Calendar".into()));
        }
        Ok(found)
    }

    /// A calendar named by the model or the settings (an id, a name, or `primary`), if it is switched on.
    pub async fn on_calendar(&self, wanted: &str) -> Result<CalInfo, CalError> {
        let on = self.switched_on().await?;
        let wanted = wanted.trim();
        let key = wanted.to_lowercase();
        on.iter()
            .find(|c| c.id == wanted || (wanted == "primary" && c.primary) || c.name.to_lowercase() == key)
            .cloned()
            .ok_or_else(|| CalError::Refused(format!("calendar \"{wanted}\" is not switched on for KAgent, so it does not exist for me. Switched on: {}", on.iter().map(|c| format!("\"{}\"", c.name)).collect::<Vec<_>>().join(", "))))
    }

    /// The events of one switched-on calendar between two instants (Google's `timeMin`, `timeMax`),
    /// every page, in the order of the start.
    pub async fn list_events(&self, calendar: &CalInfo, from: DateTime<FixedOffset>, to: DateTime<FixedOffset>, query: Option<&str>) -> Result<Vec<Event>, CalError> {
        self.require_on(calendar).await?;
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        let path = format!("/calendar/v3/calendars/{}/events", urlencode(&calendar.id));
        for _ in 0..MAX_PAGES {
            let mut q = vec![
                ("singleEvents", "true".to_string()),
                ("orderBy", "startTime".to_string()),
                ("maxResults", "250".to_string()),
                ("timeMin", from.to_rfc3339()),
                ("timeMax", to.to_rfc3339()),
            ];
            q.extend(query.filter(|t| !t.is_empty()).map(|t| ("q", t.to_string())));
            q.extend(page.take().map(|p| ("pageToken", p)));
            let answer = self.api(Method::GET, &path, &q, None).await?;
            out.extend(answer.get("items").and_then(Value::as_array).into_iter().flatten().filter(|e| e["status"] != "cancelled").filter_map(|e| Event::from_json(&calendar.id, e)));
            match answer.get("nextPageToken").and_then(Value::as_str) {
                Some(next) => page = Some(next.to_string()),
                None => break,
            }
        }
        Ok(out)
    }

    /// One event, as Google stores it.
    pub async fn get_event(&self, calendar: &CalInfo, id: &str) -> Result<Event, CalError> {
        self.require_on(calendar).await?;
        let path = format!("/calendar/v3/calendars/{}/events/{}", urlencode(&calendar.id), urlencode(id));
        let v = self.api(Method::GET, &path, &[], None).await?;
        Event::from_json(&calendar.id, &v).filter(|_| v["status"] != "cancelled").ok_or_else(|| CalError::Refused(format!("event {id} does not exist any more")))
    }

    /// `events.insert`; Google invites the guests by mail only when `invite`.
    pub async fn insert_event(&self, calendar: &CalInfo, body: &Value, invite: bool, meet: bool) -> Result<Event, CalError> {
        self.require_writable(calendar).await?;
        let path = format!("/calendar/v3/calendars/{}/events", urlencode(&calendar.id));
        let v = self.api(Method::POST, &path, &write_query(invite, meet), Some(body)).await?;
        Event::from_json(&calendar.id, &v).ok_or_else(|| CalError::Upstream("Google answered without the stored event".into()))
    }

    /// `events.patch`: the fields named in `body`.
    pub async fn patch_event(&self, calendar: &CalInfo, id: &str, body: &Value, invite: bool, meet: bool) -> Result<Event, CalError> {
        self.require_writable(calendar).await?;
        let path = format!("/calendar/v3/calendars/{}/events/{}", urlencode(&calendar.id), urlencode(id));
        let v = self.api(Method::PATCH, &path, &write_query(invite, meet), Some(body)).await?;
        Event::from_json(&calendar.id, &v).ok_or_else(|| CalError::Upstream("Google answered without the stored event".into()))
    }

    /// `events.delete`; the guests are told only when `invite`.
    pub async fn delete_event(&self, calendar: &CalInfo, id: &str, invite: bool) -> Result<(), CalError> {
        self.require_writable(calendar).await?;
        let path = format!("/calendar/v3/calendars/{}/events/{}", urlencode(&calendar.id), urlencode(id));
        self.api(Method::DELETE, &path, &[("sendUpdates", if invite { "all" } else { "none" }.to_string())], None).await.map(|_| ())
    }

    /// A `CalInfo` that did not come from `switched_on` is checked again here.
    async fn require_on(&self, calendar: &CalInfo) -> Result<(), CalError> {
        let on = self.config().calendars;
        if on.iter().any(|id| id == &calendar.id || (id == "primary" && calendar.primary)) {
            Ok(())
        } else {
            Err(CalError::Refused(format!("calendar \"{}\" is not switched on for KAgent", calendar.name)))
        }
    }

    async fn require_writable(&self, calendar: &CalInfo) -> Result<(), CalError> {
        self.require_on(calendar).await?;
        if calendar.writable {
            Ok(())
        } else {
            Err(CalError::Refused(format!("calendar \"{}\" is read-only: nothing can be added to it, moved or deleted there", calendar.name)))
        }
    }
}

fn write_query(invite: bool, meet: bool) -> Vec<(&'static str, String)> {
    let mut q = vec![("sendUpdates", if invite { "all" } else { "none" }.to_string())];
    if meet {
        q.push(("conferenceDataVersion", "1".to_string()));
    }
    q
}

/// The body of a new Meet link (calendar.md section 3: `meet: true`).
pub fn meet_request(id: &str) -> Value {
    json!({"createRequest": {"requestId": id, "conferenceSolutionKey": {"type": "hangoutsMeet"}}})
}
