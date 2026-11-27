//! Tokens and keys never leave the module (calendar.md sections 1 and 5): a whole run against
//! the fake, with the log captured, and every tool result, error and view read for them.

use std::io::Write;
use std::sync::{Arc, Mutex};

use kagent_core::tool::Tool;
use serde_json::json;

use super::super::fake::{CLIENT_SECRET, MAPS_KEY, REFRESH_TOKEN};
use super::super::tools::CalendarEvents;
use super::{rig, Rig};

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Log {
    type Writer = Log;
    fn make_writer(&'a self) -> Log {
        self.clone()
    }
}

async fn run(r: &Rig, tool: &dyn Tool, input: serde_json::Value) -> String {
    let out = tool.call(input, r.ctx()).await.unwrap();
    out.content
}

#[tokio::test]
async fn no_token_and_no_key_is_in_a_tool_result_a_view_an_error_or_a_log_line() {
    let log = Log::default();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_max_level(tracing::Level::DEBUG).with_ansi(false).with_writer(log.clone()).finish());
    let mut shown: Vec<String> = Vec::new();

    let mut r = rig().await;
    r.ready(&["primary"]).await;
    r.fake.add_event("primary", json!({"summary": "Dentist", "start": {"dateTime": "2026-11-27T10:00:00+01:00"}, "end": {"dateTime": "2026-11-27T11:00:00+01:00"}}));
    let wrong = "AIzaSyWrongKey-111111111111111111111";
    r.cal.set_maps_key(Some(wrong)).await.unwrap();
    let access = r.cal.access_token().await.unwrap();

    let events = CalendarEvents::new(r.cell());
    let change = super::super::change::CalendarChange::new(r.cell());
    let place = super::super::place_tool::PlaceSearch::new(r.cell());
    let report = super::super::report_tool::CalendarReport::new(r.cell());
    shown.push(run(&r, &events, json!({})).await);
    shown.push(run(&r, &change, json!({"action": "add", "title": "Lunch", "start": "2026-11-28T12:00", "location": {"name": "X", "address": "Y"}})).await);
    shown.push(run(&r, &change, json!({"action": "delete", "id": "nope"})).await);
    shown.push(run(&r, &place, json!({"query": "Thai Orchid"})).await);
    shown.push(run(&r, &report, json!({})).await);
    // Failures: an expired access token, a revoked grant, a refused code, a Maps key Google refuses.
    r.fake.expire_access_tokens();
    r.fake.revoke_grant();
    shown.push(run(&r, &events, json!({})).await);
    shown.push(run(&r, &events, json!({})).await);
    shown.push(r.cal.callback("4/bad", "bad-state").await.unwrap_err().to_string());
    shown.extend(r.cal.set_client("x y", Some("z")).await.err().map(|e| e.to_string()));
    shown.push(format!("{:?}", r.cal.state_and_error()));
    r.cal.disconnect().await.unwrap();
    shown.push(run(&r, &events, json!({})).await);
    shown.extend(r.drain());

    let secrets = [("access token", access.as_str()), ("refresh token", REFRESH_TOKEN), ("client secret", CLIENT_SECRET), ("Maps key", MAPS_KEY), ("wrong Maps key", wrong), ("any access token", "ya29.")];
    let logged = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    assert!(logged.contains("calendar: connected") && logged.contains("disconnected"), "the log is real, not empty: {logged}");
    for (what, secret) in secrets {
        for text in &shown {
            assert!(!text.contains(secret), "{what} in a result: {text}");
        }
        assert!(!logged.contains(secret), "{what} in the log:\n{logged}");
    }
    // And what the settings and the saved files hold apart from the secret store.
    let settings = serde_json::to_string(&*r.settings.read().unwrap()).unwrap();
    for (what, secret) in [("client secret", CLIENT_SECRET), ("refresh token", REFRESH_TOKEN), ("Maps key", MAPS_KEY)] {
        assert!(!settings.contains(secret), "{what} in the settings");
    }
    let state = std::fs::read_to_string(r.dir.path().join("data/calendar-state.json")).unwrap_or_default();
    assert!(!state.contains(CLIENT_SECRET) && !state.contains(REFRESH_TOKEN) && !state.contains(MAPS_KEY), "{state}");
}
