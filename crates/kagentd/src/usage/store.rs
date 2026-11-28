//! The counters (usage.md section 2): requests and tokens per provider, model and source, in windows of a minute, a day
//! and a month of the machine's local time, in `<data>/usage.sqlite`. Rows older than 13 months are deleted.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Local, Months};
use rusqlite::{params, Connection};
use serde::Serialize;

/// The windows, as the `win` column spells them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Minute,
    Day,
    Month,
}

impl Window {
    fn name(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Day => "day",
            Self::Month => "month",
        }
    }

    /// The key of the window `at` falls in: `YYYY-MM-DDTHH:MM`, `YYYY-MM-DD` or `YYYY-MM`.
    pub fn key(self, at: DateTime<Local>) -> String {
        at.format(match self {
            Self::Minute => "%Y-%m-%dT%H:%M",
            Self::Day => "%Y-%m-%d",
            Self::Month => "%Y-%m",
        })
        .to_string()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// What one model used today and this month.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTotals {
    pub model: String,
    pub today: Totals,
    pub month: Totals,
}

pub struct Counters {
    conn: Mutex<Connection>,
}

impl Counters {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Self::with(Connection::open(path)?)
    }

    /// Counters kept in memory: when the file cannot be opened, and in tests.
    pub fn memory() -> anyhow::Result<Self> {
        Self::with(Connection::open_in_memory()?)
    }

    fn with(conn: Connection) -> anyhow::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS usage(
               win TEXT NOT NULL, start TEXT NOT NULL, provider TEXT NOT NULL, model TEXT NOT NULL, source TEXT NOT NULL,
               requests INTEGER NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL,
               PRIMARY KEY(win, start, provider, model, source)) WITHOUT ROWID;",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// One answered request: added to the minute, the day and the month of `at`.
    pub fn record(&self, at: DateTime<Local>, provider: &str, model: &str, source: &str, input: u64, output: u64) -> anyhow::Result<()> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        for w in [Window::Minute, Window::Day, Window::Month] {
            tx.execute(
                "INSERT INTO usage(win,start,provider,model,source,requests,input_tokens,output_tokens) VALUES(?1,?2,?3,?4,?5,1,?6,?7)
                 ON CONFLICT(win,start,provider,model,source) DO UPDATE SET requests=requests+1, input_tokens=input_tokens+excluded.input_tokens, output_tokens=output_tokens+excluded.output_tokens",
                params![w.name(), w.key(at), provider, model, source, input as i64, output as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// What `provider` used in the window of `at`, all models and sources.
    pub fn totals(&self, provider: &str, window: Window, at: DateTime<Local>) -> Totals {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT COALESCE(SUM(requests),0), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0) FROM usage WHERE win=?1 AND start=?2 AND provider=?3",
            params![window.name(), window.key(at), provider],
            |r| Ok(Totals { requests: r.get::<_, i64>(0)? as u64, input_tokens: r.get::<_, i64>(1)? as u64, output_tokens: r.get::<_, i64>(2)? as u64 }),
        )
        .unwrap_or_default()
    }

    /// Today's and this month's totals per model of `provider`, the busiest month first.
    pub fn per_model(&self, provider: &str, at: DateTime<Local>) -> Vec<ModelTotals> {
        let c = self.conn.lock().unwrap();
        let (day, month) = (Window::Day.key(at), Window::Month.key(at));
        let mut st = match c.prepare(
            "SELECT model,
               COALESCE(SUM(CASE WHEN win='day' AND start=?2 THEN requests END),0), COALESCE(SUM(CASE WHEN win='day' AND start=?2 THEN input_tokens END),0), COALESCE(SUM(CASE WHEN win='day' AND start=?2 THEN output_tokens END),0),
               COALESCE(SUM(CASE WHEN win='month' AND start=?3 THEN requests END),0), COALESCE(SUM(CASE WHEN win='month' AND start=?3 THEN input_tokens END),0), COALESCE(SUM(CASE WHEN win='month' AND start=?3 THEN output_tokens END),0)
             FROM usage WHERE provider=?1 AND ((win='day' AND start=?2) OR (win='month' AND start=?3)) GROUP BY model",
        ) {
            Ok(st) => st,
            Err(_) => return Vec::new(),
        };
        let n = |r: &rusqlite::Row, i: usize| r.get::<_, i64>(i).map(|v| v as u64);
        let rows = st.query_map(params![provider, day, month], |r| {
            Ok(ModelTotals {
                model: r.get(0)?,
                today: Totals { requests: n(r, 1)?, input_tokens: n(r, 2)?, output_tokens: n(r, 3)? },
                month: Totals { requests: n(r, 4)?, input_tokens: n(r, 5)?, output_tokens: n(r, 6)? },
            })
        });
        let mut out: Vec<ModelTotals> = rows.map(|rows| rows.flatten().collect()).unwrap_or_default();
        out.sort_by(|a, b| b.month.requests.cmp(&a.month.requests).then(a.model.cmp(&b.model)));
        out
    }

    /// Rows older than 13 months. Returns how many went.
    pub fn prune(&self, now: DateTime<Local>) -> usize {
        let Some(cutoff) = now.checked_sub_months(Months::new(13)) else { return 0 };
        let c = self.conn.lock().unwrap();
        [Window::Minute, Window::Day, Window::Month]
            .iter()
            .map(|w| c.execute("DELETE FROM usage WHERE win=?1 AND start<?2", params![w.name(), w.key(cutoff)]).unwrap_or(0))
            .sum()
    }

    /// Per source: how many requests of `provider` today (for the tests and the tool).
    #[cfg(test)]
    pub fn by_source(&self, provider: &str, at: DateTime<Local>) -> Vec<(String, u64)> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT source, SUM(requests) FROM usage WHERE win='day' AND start=?1 AND provider=?2 GROUP BY source ORDER BY source").unwrap();
        st.query_map(params![Window::Day.key(at), provider], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))).unwrap().flatten().collect()
    }

    #[cfg(test)]
    pub fn rows(&self, window: Window) -> usize {
        self.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM usage WHERE win=?1", params![window.name()], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }
}
