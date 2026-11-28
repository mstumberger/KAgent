//! The morning report, running part (calendar.md section 6): the timer inside the module,
//! the text for a day, and its delivery. The text is `report.rs`; this file reads the events
//! and the tasks, asks for the drive, and sends it. No model is asked.

use chrono::{Duration, NaiveDate, Timelike};

use super::model::{overlap, Event};
use super::report::{self, Clash, Data, Day, Due, Item, Lang, Leave, Next, TaskLine};
use super::state::{self, LastReport, Spoken};
use super::tools::collect;
use super::{CalError, Calendar, ConnState};
use crate::notify::{Channel, Notification, Urgency};

/// How often the timer looks at the clock.
const TICK: std::time::Duration = std::time::Duration::from_secs(20);
/// A first event before this hour (or the only one) is told the evening before.
const EARLY_HOUR: u32 = 10;

/// The reminders and tasks of KAgent that run on `day`, by time.
pub fn tasks_due(store: &crate::tasks::TaskStore, day: NaiveDate) -> Vec<TaskLine> {
    let mut due: Vec<(chrono::DateTime<chrono::Local>, String)> = store.list().into_iter().filter(|t| t.enabled && !t.done).filter_map(|t| t.next_run.filter(|n| n.date_naive() == day).map(|n| (n, t.title))).collect();
    due.sort();
    due.into_iter().map(|(at, title)| TaskLine { at: Some(format!("{:02}:{:02}", at.hour(), at.minute())), title }).collect()
}

impl Calendar {
    /// The owner wrote this: when it is Slovenian or English, `persona.language: auto` follows it.
    pub fn hear(&self, text: &str) {
        let Some(lang) = report::language_of(text) else { return };
        let mut saved = self.saved.lock().unwrap();
        if saved.last_language != lang.name() {
            saved.last_language = lang.name().into();
            state::save(&self.data_dir, &saved);
        }
    }

    fn lang(&self) -> Lang {
        let setting = self.settings.read().unwrap().persona.language.clone();
        Lang::of(&setting, &self.saved.lock().unwrap().last_language)
    }

    fn hhmm(&self, e: &Event) -> String {
        let t = e.starts(&self.zone);
        format!("{:02}:{:02}", t.hour(), t.minute())
    }

    /// The report of a day, as text. Reads Google; changes nothing.
    pub async fn compose(&self, day: Day) -> Result<String, CalError> {
        self.ready()?;
        let cfg = self.config();
        let date = match day {
            Day::Today => self.today(),
            Day::Tomorrow => self.today() + Duration::days(1),
        };
        let (_, events) = collect(self, self.day_start(date), self.day_start(date + Duration::days(1)), None, None).await?;
        let items = events.iter().map(|e| Item { at: (!e.all_day()).then(|| self.hhmm(e)), title: e.title.clone(), place: e.location.clone() }).collect();
        let mut clashes = Vec::new();
        for (n, a) in events.iter().enumerate() {
            for b in &events[n + 1..] {
                if let Some((from, to)) = overlap(a, b, &self.zone) {
                    clashes.push(Clash { a: a.title.clone(), b: b.title.clone(), from: format!("{:02}:{:02}", from.hour(), from.minute()), to: format!("{:02}:{:02}", to.hour(), to.minute()) });
                }
            }
        }
        let today = day == Day::Today;
        let leave = if today && cfg.report.leave_by { self.leave_line(&events).await } else { None };
        let tomorrow = if today && cfg.report.tomorrow { self.tomorrow_line(date + Duration::days(1)).await } else { None };
        let tasks = if cfg.report.tasks { (self.tasks)(date) } else { Vec::new() };
        let address = self.settings.read().unwrap().persona.address.clone();
        Ok(report::text(&Data { lang: self.lang(), address, date, day, items, clashes, leave, tomorrow, tasks }))
    }

    /// "Leave by 08:05": the first event that is still to come and has a place, from home. Needs a Maps key and `home`.
    async fn leave_line(&self, events: &[Event]) -> Option<Leave> {
        let first = events.iter().find(|e| !e.all_day() && !e.location.trim().is_empty() && e.starts(&self.zone).to_utc() > self.now())?;
        let home = self.home_lat_lng().await?;
        let key = self.maps_key().await?;
        let place = self.places_search(&key, &first.location, None).await.ok()?.into_iter().next()?.lat_lng?;
        let start = first.starts(&self.zone);
        let drive = self.drive(home, place, start.to_utc()).await.map_err(|e| tracing::warn!("calendar: no travel time for the report: {e}")).ok()?;
        let margin = i64::from(self.config().travel_margin_min);
        let leave = start - Duration::seconds(drive.with_traffic) - Duration::minutes(margin);
        Some(Leave { title: first.title.clone(), at: format!("{:02}:{:02}", leave.hour(), leave.minute()), drive_min: (drive.with_traffic as f64 / 60.0).round() as i64, margin_min: margin })
    }

    /// The first event of tomorrow, when it starts before 10:00 or is the only one.
    async fn tomorrow_line(&self, date: NaiveDate) -> Option<Next> {
        let (_, events) = collect(self, self.day_start(date), self.day_start(date + Duration::days(1)), None, None).await.ok()?;
        let first = events.iter().find(|e| !e.all_day()).or_else(|| events.first())?;
        let only = events.len() == 1;
        let early = !first.all_day() && first.starts(&self.zone).hour() < EARLY_HOUR;
        (early || only).then(|| Next { title: first.title.clone(), at: (!first.all_day()).then(|| self.hhmm(first)), only })
    }

    /// Send a report to the channels that are on, and keep it for the page. Returns where it went.
    pub async fn deliver(&self, text: &str) -> Vec<String> {
        let cfg = self.config().report;
        let mut sent = Vec::new();
        if cfg.telegram && (self.paired)() {
            if let Some(notifier) = &self.notifier {
                let note = Notification { title: "Morning report".into(), body: text.to_string(), urgency: Urgency::Normal, channels: vec![Channel::Telegram] };
                if notifier.send(note).await.is_ok() {
                    sent.push("telegram".to_string());
                }
            }
        }
        let now = self.now();
        let today = now.with_timezone(&self.zone_at(now)).date_naive().to_string();
        let mut saved = self.saved.lock().unwrap();
        if cfg.spoken {
            saved.spoken = Some(Spoken { day: today, text: text.to_string() });
            sent.push("spoken".into());
        }
        saved.last_report = Some(LastReport { at: now.to_rfc3339(), text: text.to_string(), sent: sent.clone() });
        state::save(&self.data_dir, &saved);
        sent
    }

    /// `POST /api/calendar/report`: the text, and with `deliver` also sent to the channels that are on.
    pub async fn report_now(&self, deliver: bool) -> Result<(String, Vec<String>), CalError> {
        let text = self.compose(Day::Today).await?;
        let sent = if deliver { self.deliver(&text).await } else { Vec::new() };
        Ok((text, sent))
    }

    /// The report that waits for a voice session: given once, and only on the day it was made.
    pub fn take_spoken(&self) -> Option<String> {
        let now = self.now();
        let today = now.with_timezone(&self.zone_at(now)).date_naive().to_string();
        let mut saved = self.saved.lock().unwrap();
        let waiting = saved.spoken.take()?;
        state::save(&self.data_dir, &saved);
        (waiting.day == today).then_some(waiting.text)
    }

    /// One look at the clock: deliver today's report when it is due (calendar.md section 6).
    pub async fn tick(&self) {
        let now = self.now();
        let local = now.with_timezone(&self.zone_at(now)).naive_local();
        let verdict = {
            let mut saved = self.saved.lock().unwrap();
            let verdict = report::due(&self.config().report, local, &saved.report_day);
            if verdict != Due::No {
                // Marked first: whatever happens next, today's report is not tried again and again.
                saved.report_day = local.date().to_string();
                state::save(&self.data_dir, &saved);
            }
            verdict
        };
        if verdict != Due::Yes || !self.config().enabled {
            return;
        }
        if self.state() != ConnState::Connected {
            let first = !std::mem::replace(&mut self.saved.lock().unwrap().told_skip, true);
            if first {
                let _ = self.events.send(kagent_core::protocol::AgentEvent::keyed_notice("warn", "No morning report: Google Calendar is not connected (Settings -> Calendar).", "calendar:report"));
            }
            return;
        }
        match self.compose(Day::Today).await {
            Ok(text) => {
                self.saved.lock().unwrap().told_skip = false;
                let sent = self.deliver(&text).await;
                tracing::info!("calendar: the morning report went to: {}", if sent.is_empty() { "nobody (no channel is on)".into() } else { sent.join(", ") });
            }
            Err(e) => {
                tracing::warn!("calendar: the morning report could not be made: {e}");
                let _ = self.events.send(kagent_core::protocol::AgentEvent::keyed_notice("warn", format!("The morning report could not be made: {e}"), "calendar:report"));
            }
        }
    }

    /// The timer: a task of the module, not a task of `tasks.json`.
    pub fn start(self: &std::sync::Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            loop {
                me.tick().await;
                tokio::time::sleep(TICK).await;
            }
        });
    }
}
