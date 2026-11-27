//! `calendar_change` (calendar.md section 3): add, move and delete an event. The question the
//! owner answers (the card, the spoken read-back) is built from the event as Google stores it
//! (`prime` reads it first), never from the model's words. Every change is journaled with the
//! way back, and `Calendar` is the undo handler of those records.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use kagent_core::journal::{Change, ChangeJournal, ChangeStatus};
use kagent_core::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput, UndoHandler};
use serde_json::{json, Value};

use super::model::{CalInfo, Event};
use super::plan::{self, Params};
use super::tools::{event_line, failed, ready, CalendarCell, UNTRUSTED};
use super::{CalError, Calendar};

/// What the module read before a question: the calendars, and the events of the calls in flight.
#[derive(Default)]
pub struct Memo {
    pub calendars: Vec<CalInfo>,
    pub events: HashMap<String, (CalInfo, Event)>,
}

pub struct CalendarChange {
    contract: ToolContract,
    cell: CalendarCell,
}

impl CalendarChange {
    pub fn new(cell: CalendarCell) -> Self {
        let mut contract = ToolContract::read_only(
            "calendar_change",
            "Add, move or delete an event in the owner's Google Calendar (only calendars the owner switched on). add needs title and start, then end or durationMin (default 1 hour); update and delete need the id from calendar_events. To add an event follow the skill calendar-event: ask one question at a time (when, how long, where, calendar, reminder, notes), use place_search for a place, set guests only when the owner named people and gave their email, and set meet only when they asked for a call. The owner confirms every change; delete is destructive. Only one event is changed, never a whole series.",
            json!({"type":"object","required":["action"],"properties":{
                "action":{"type":"string","enum":["add","update","delete"]},
                "id":{"type":"string","description":"the event's id from calendar_events (update, delete)"},
                "calendar":{"type":"string","description":"calendar name or id (add: default is the owner's default calendar)"},
                "title":{"type":"string"},
                "start":{"type":"string","description":"2026-11-28T12:00 in the machine's time zone, or a date with allDay"},
                "end":{"type":"string","description":"2026-11-28T13:00; or give durationMin"},
                "durationMin":{"type":"integer","description":"minutes, instead of end"},
                "allDay":{"type":"boolean"},
                "location":{"type":"object","properties":{"name":{"type":"string"},"address":{"type":"string"},"mapsUrl":{"type":"string","description":"the mapsUrl of place_search"}}},
                "notes":{"type":"string"},
                "reminderMin":{"type":"integer","description":"popup reminder minutes before; null: Google's default"},
                "meet":{"type":"boolean","description":"add a Google Meet link; only when the owner asked for a call"},
                "guests":{"type":"array","items":{"type":"string"},"description":"email addresses; Google mails them an invitation"}
            }}),
        )
        .mutating(SideEffect::Network, Risk::Medium);
        // Asked in every permission mode, and by voice always (voice-control.md section 4).
        contract.always_ask = true;
        Self { contract: contract.timeout(40_000, 90_000), cell }
    }

    fn action(input: &Value) -> &str {
        input.get("action").and_then(Value::as_str).unwrap_or("")
    }

    /// The sentence of the card, and whether the call deletes.
    fn sentence(&self, input: &Value) -> (String, bool) {
        let action = Self::action(input);
        let Some(cal) = self.cell.get() else { return (format!("calendar {action}"), action == "delete") };
        let params = plan::read(input).unwrap_or_default();
        let stored = input.get("id").and_then(Value::as_str).and_then(|id| cal.memo.lock().unwrap().events.get(id).map(|(_, e)| e.clone()));
        match (action, stored) {
            ("add", _) => {
                let when = plan::times(cal, &params, None).ok().flatten();
                let name = cal.calendar_name(input.get("calendar").and_then(Value::as_str));
                (when.map(|w| plan::summary_add(&params, &w, cal, &name)).unwrap_or_else(|| format!("add \"{}\"", params.title.clone().unwrap_or_default())), false)
            }
            ("update", Some(old)) => {
                let when = plan::times(cal, &params, Some(&old)).ok().flatten();
                (plan::summary_update(&params, &old, &when, cal), false)
            }
            ("delete", Some(old)) => (plan::summary_delete(&old, cal), true),
            ("update", None) => (format!("change event {}", input.get("id").and_then(Value::as_str).unwrap_or("?")), false),
            ("delete", None) => (format!("delete event {}", input.get("id").and_then(Value::as_str).unwrap_or("?")), true),
            _ => ("calendar change".to_string(), false),
        }
    }
}

impl Calendar {
    /// The name of a calendar for a sentence: the one named, or the default.
    pub fn calendar_name(&self, wanted: Option<&str>) -> String {
        let wanted = wanted.map(str::trim).filter(|w| !w.is_empty()).map(String::from).unwrap_or_else(|| self.config().default_calendar);
        let memo = self.memo.lock().unwrap();
        memo.calendars.iter().find(|c| c.id == wanted || (wanted == "primary" && c.primary) || c.name.eq_ignore_ascii_case(&wanted)).map(|c| c.name.clone()).unwrap_or(wanted)
    }

    /// An event by id: in the calendar named, or in the first switched-on calendar that has it.
    pub async fn find_event(&self, calendar: Option<&str>, id: &str) -> Result<(CalInfo, Event), CalError> {
        let list = match calendar.map(str::trim).filter(|c| !c.is_empty()) {
            Some(name) => vec![self.on_calendar(name).await?],
            None => self.switched_on().await?,
        };
        for c in &list {
            match self.get_event(c, id).await {
                Ok(e) => {
                    self.memo.lock().unwrap().events.insert(id.to_string(), (c.clone(), e.clone()));
                    return Ok((c.clone(), e));
                }
                Err(CalError::Refused(_)) => continue,
                Err(other) => return Err(other),
            }
        }
        Err(CalError::Refused(format!("no event with id {id} in the calendars that are switched on: read the ids again with calendar_events")))
    }

    fn require_instance(event: &Event) -> Result<(), CalError> {
        if event.raw.get("recurrence").is_some() {
            return Err(CalError::Refused(format!("\"{}\" is a whole series: change the series in Google Calendar (KAgent changes one event at a time, by the id of that event from calendar_events)", event.title)));
        }
        Ok(())
    }

    /// The calendar an event is added to, which must be switched on and writable.
    async fn target(&self, wanted: Option<&str>) -> Result<CalInfo, CalError> {
        let name = wanted.map(str::trim).filter(|w| !w.is_empty()).map(String::from).unwrap_or_else(|| self.config().default_calendar);
        let calendar = self.on_calendar(&name).await?;
        if !calendar.writable {
            return Err(CalError::Refused(format!("calendar \"{}\" is read-only: choose another one, or ask the owner to change the default in Settings -> Calendar", calendar.name)));
        }
        Ok(calendar)
    }

    fn journal(&self, ctx: &ToolContext, summary: &str, op: Value, undo: Option<Value>, after: &Event) -> Option<String> {
        let services = ctx.services.as_deref()?;
        let change = Change {
            id: ChangeJournal::new_id(),
            ts: chrono::Utc::now(),
            session_id: ctx.session_id.clone(),
            call_id: ctx.call_id.clone(),
            tool: "calendar_change".into(),
            summary: summary.into(),
            op,
            undo,
            privileged: false,
            status: ChangeStatus::Applied,
            before: None,
            after: Some(format!("{} ({})", after.title, after.link)),
            undone_at: None,
            note: None,
        };
        match services.journal.save(&change) {
            Ok(()) => Some(change.id),
            Err(e) => {
                tracing::warn!("calendar: the change could not be journaled: {e:#}");
                None
            }
        }
    }

    fn report(&self, done: &str, e: &Event, calendar: &CalInfo, change: Option<String>) -> String {
        let mut out = format!("{UNTRUSTED}\ndone: {done}\nstored event: {}\nlink: {}\n", event_line(self, e, &calendar.name), e.link);
        match change {
            Some(id) => out.push_str(&format!("change id: {id} (undo available: undo_change)\n")),
            None => out.push_str("not journaled: there is no undo for this one\n"),
        }
        out.push_str("Tell the owner what was stored, from the stored event above.");
        out
    }

    pub async fn run_change(&self, ctx: &ToolContext, input: &Value) -> Result<String, CalError> {
        let params = plan::read(input).map_err(CalError::Refused)?;
        let id = input.get("id").and_then(Value::as_str).map(str::trim).filter(|i| !i.is_empty());
        let wanted = input.get("calendar").and_then(Value::as_str);
        match CalendarChange::action(input) {
            "add" => self.add(ctx, &params, wanted).await,
            "update" => self.update(ctx, &params, id.ok_or_else(|| CalError::Refused("update needs the id of the event: read it with calendar_events".into()))?, wanted).await,
            "delete" => self.delete(ctx, id.ok_or_else(|| CalError::Refused("delete needs the id of the event: read it with calendar_events".into()))?, wanted).await,
            other => Err(CalError::Refused(format!("action must be add, update or delete, not \"{other}\""))),
        }
    }

    async fn add(&self, ctx: &ToolContext, p: &Params, wanted: Option<&str>) -> Result<String, CalError> {
        if p.title.is_none() {
            return Err(CalError::Refused("add needs a title".into()));
        }
        let when = plan::times(self, p, None).map_err(CalError::Refused)?.expect("a new event has times");
        let calendar = self.target(wanted).await?;
        let meet_id = uuid::Uuid::new_v4().to_string();
        let body = plan::add_body(p, &when, &meet_id);
        let stored = self.insert_event(&calendar, &body, !p.guests.is_empty(), p.meet).await?;
        let summary = plan::summary_add(p, &when, self, &calendar.name);
        let op = json!({"kind": "user", "action": "calendar", "op": "add", "calendar": calendar.id, "eventId": stored.id});
        let undo = json!({"kind": "user", "action": "calendar", "step": "delete", "calendar": calendar.id, "id": stored.id});
        let change = self.journal(ctx, &summary, op, Some(undo), &stored);
        Ok(self.report(&summary, &stored, &calendar, change))
    }

    async fn update(&self, ctx: &ToolContext, p: &Params, id: &str, wanted: Option<&str>) -> Result<String, CalError> {
        let (calendar, old) = self.find_event(wanted, id).await?;
        Self::require_instance(&old)?;
        let when = plan::times(self, p, Some(&old)).map_err(CalError::Refused)?;
        let patch = plan::patch(p, &old, when, &uuid::Uuid::new_v4().to_string());
        if patch.body.as_object().is_none_or(|b| b.is_empty()) {
            return Err(CalError::Refused("nothing to change: name at least one of title, start, end, durationMin, allDay, location, notes, reminderMin, meet or guests".into()));
        }
        let stored = self.patch_event(&calendar, id, &patch.body, patch.invite, patch.meet).await?;
        let summary = plan::summary_update(p, &old, &when, self);
        let op = json!({"kind": "user", "action": "calendar", "op": "update", "calendar": calendar.id, "eventId": id});
        let undo = json!({"kind": "user", "action": "calendar", "step": "patch", "calendar": calendar.id, "id": id, "fields": patch.undo});
        let change = self.journal(ctx, &summary, op, Some(undo), &stored);
        Ok(self.report(&summary, &stored, &calendar, change))
    }

    async fn delete(&self, ctx: &ToolContext, id: &str, wanted: Option<&str>) -> Result<String, CalError> {
        let (calendar, old) = self.find_event(wanted, id).await?;
        Self::require_instance(&old)?;
        self.delete_event(&calendar, id, false).await?;
        let summary = plan::summary_delete(&old, self);
        let op = json!({"kind": "user", "action": "calendar", "op": "delete", "calendar": calendar.id, "eventId": id});
        let undo = json!({"kind": "user", "action": "calendar", "step": "insert", "calendar": calendar.id, "event": plan::restore_body(&old.raw)});
        let change = self.journal(ctx, &summary, op, Some(undo), &old);
        let mut out = format!("done: {summary}\n");
        match change {
            Some(c) => out.push_str(&format!("change id: {c} (undo available: undo_change brings it back as a new event)\n")),
            None => out.push_str("not journaled: there is no undo for this one\n"),
        }
        if old.meet.is_some() || !old.guests.is_empty() {
            out.push_str("Note: guests were not told, and an undo cannot bring back a Meet link.\n");
        }
        Ok(out)
    }
}

#[async_trait]
impl Tool for CalendarChange {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    async fn prime(&self, input: &Value) {
        let Some(cal) = self.cell.get() else { return };
        if cal.ready().is_err() {
            return;
        }
        // The calendars (for the name in the sentence) and the event the call is about.
        let _ = cal.switched_on().await.map(|on| cal.memo.lock().unwrap().calendars = on);
        if let (true, Some(id)) = (matches!(Self::action(input), "update" | "delete"), input.get("id").and_then(Value::as_str)) {
            let _ = cal.find_event(input.get("calendar").and_then(Value::as_str), id.trim()).await;
        }
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let (summary, delete) = self.sentence(input);
        let a = CallAssessment::from_contract(&self.contract, summary, format!("calendar_change {}", Self::action(input)));
        if delete { a.destructive() } else { a }
    }

    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let cal = match ready(&self.cell) {
            Ok(cal) => cal,
            Err(out) => return Ok(out),
        };
        Ok(match cal.run_change(&ctx, &input).await {
            Ok(text) => ToolOutput::ok(text),
            Err(e) => failed(e),
        })
    }
}

/// The undo of a journaled calendar change (`undo_change`): the way back each record holds. Its
/// `step` says which (not `op`: a record with an `op` is a root-helper request).
#[async_trait]
impl UndoHandler for Calendar {
    fn action(&self) -> &'static str {
        "calendar"
    }

    async fn undo(&self, undo: &Value) -> (bool, String) {
        match self.undo_op(undo).await {
            Ok(done) => (true, done),
            Err(e) => (false, e.to_string()),
        }
    }
}

impl Calendar {
    async fn undo_op(&self, undo: &Value) -> Result<String, CalError> {
        self.ready()?;
        let calendar = self.on_calendar(undo["calendar"].as_str().unwrap_or_default()).await?;
        let id = undo["id"].as_str().unwrap_or_default();
        match undo["step"].as_str() {
            Some("delete") => {
                self.delete_event(&calendar, id, false).await?;
                Ok(format!("deleted the event {id} again"))
            }
            Some("insert") => {
                let e = self.insert_event(&calendar, &undo["event"], false, false).await?;
                Ok(format!("brought the event back as a new one ({}): {}", e.id, e.link))
            }
            Some("patch") => {
                let e = self.patch_event(&calendar, id, &undo["fields"], false, false).await?;
                Ok(format!("wrote the old values back into \"{}\"", e.title))
            }
            _ => Err(CalError::Refused("this record has no calendar undo".into())),
        }
    }
}

/// Register the module as the undo handler of `calendar` records.
pub fn register_undo(cal: &Arc<Calendar>, services: &kagent_core::tool::ToolServices) {
    services.undo.write().unwrap().push(cal.clone());
}
