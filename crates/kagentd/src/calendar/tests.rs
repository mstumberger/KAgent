//! Tests of the calendar module against the fake Google (calendar.md section 9). `rig` is
//! the one place a module is built; each file of `tests/` holds the checks of one part.

mod connection;

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

pub async fn rig() -> Rig {
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
    })
    .await;
    Rig { cal, fake, dir, settings, events, skew }
}
