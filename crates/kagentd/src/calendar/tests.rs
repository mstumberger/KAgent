//! Tests of the calendar module against the fake Google (calendar.md section 9). `rig` is
//! the one place a module is built; each file of `tests/` holds the checks of one part.

mod change;
mod connection;
mod google;
mod places;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, RwLock};

use chrono::{DateTime, Duration, Utc};
use tokio::sync::broadcast;

use kagent_core::protocol::AgentEvent;

use super::fake::{self, FakeGoogle};
use super::{Calendar, Parts};
use crate::secrets::TokenStore;
use crate::settings::Settings;

pub struct Rig {
    pub cal: Arc<Calendar>,
    pub fake: FakeGoogle,
    pub dir: tempfile::TempDir,
    pub settings: Arc<RwLock<Settings>>,
    pub events: broadcast::Receiver<AgentEvent>,
    skew: Arc<AtomicI64>,
}

impl Rig {
    /// The clock moves on by `by`.
    pub fn later(&self, by: Duration) {
        self.skew.fetch_add(by.num_seconds(), Ordering::SeqCst);
    }

    /// A tool context with the daemon's services: the journal in this rig's folder, and the module as the undo of calendar records.
    pub fn ctx(&self) -> kagent_core::tool::ToolContext {
        let journal = Arc::new(kagent_core::journal::ChangeJournal::new(self.dir.path().join("changes")).unwrap());
        let services = Arc::new(kagent_core::tool::ToolServices::new(journal, Arc::new(kagent_core::tools::privop::PrivClient::with_path("/nonexistent/kagent-priv"))));
        super::change::register_undo(&self.cal, &services);
        let mut ctx = kagent_core::tool::ToolContext::standalone(std::time::Duration::from_secs(5), 24_000);
        ctx.services = Some(services);
        ctx.session_id = "s1".into();
        ctx.call_id = "c1".into();
        ctx
    }

    /// The tools' handle on the module.
    pub fn cell(&self) -> super::tools::CalendarCell {
        let cell: super::tools::CalendarCell = Default::default();
        let _ = cell.set(self.cal.clone());
        cell
    }

    /// Connected, switched on, and these calendars switched on (the first is the default).
    pub async fn ready(&self, calendars: &[&str]) {
        self.connect().await.unwrap();
        self.settings.write().unwrap().patch(serde_json::json!({"calendar": {"calendars": calendars, "defaultCalendar": calendars[0]}})).unwrap();
    }

    pub fn secret_file(&self) -> std::path::PathBuf {
        self.dir.path().join("calendar.json")
    }

    /// The `calendarChanged` states and notices since the last call.
    pub fn drain(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                AgentEvent::CalendarChanged { state } => out.push(format!("state:{state}")),
                AgentEvent::Notice { text, .. } => out.push(format!("notice:{text}")),
                _ => {}
            }
        }
        out
    }

    /// The owner pastes the client, presses Connect, approves at Google, and Google calls back.
    pub async fn connect(&self) -> Result<(), super::CalError> {
        self.cal.set_client(fake::CLIENT_ID, Some(fake::CLIENT_SECRET)).await?;
        let url = reqwest::Url::parse(&self.cal.connect()?).unwrap();
        let q: std::collections::HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        let code = self.fake.authorize(&q["code_challenge"]);
        self.cal.callback(&code, &q["state"]).await
    }
}

/// The machine is on UTC+01:00 (Ljubljana in winter).
pub fn zone(hours: i32) -> super::model::Zone {
    Arc::new(move |_| chrono::FixedOffset::east_opt(hours * 3600).unwrap())
}

pub async fn rig() -> Rig {
    rig_with(None).await
}

pub async fn rig_with(web: Option<Arc<crate::web::Web>>) -> Rig {
    let fake = FakeGoogle::start().await;
    let dir = tempfile::tempdir().unwrap();
    let settings = Arc::new(RwLock::new(Settings::defaults()));
    let (events_tx, events) = broadcast::channel(64);
    let start: DateTime<Utc> = "2026-11-27T08:00:00Z".parse().unwrap();
    let skew = Arc::new(AtomicI64::new(0));
    let clock_skew = skew.clone();
    let cal = Calendar::new(Parts {
        settings: settings.clone(),
        settings_path: dir.path().join("settings.json"),
        secrets: TokenStore::File(dir.path().join("calendar.json")),
        hosts: fake.hosts(),
        redirect: "http://127.0.0.1:7878/api/calendar/oauth/callback".into(),
        events: events_tx,
        clock: Arc::new(move || start + Duration::seconds(clock_skew.load(Ordering::SeqCst))),
        zone: zone(1),
        data_dir: dir.path().join("data"),
        maps: TokenStore::File(dir.path().join("maps.secret")),
        web,
    })
    .await;
    Rig { cal, fake, dir, settings, events, skew }
}
