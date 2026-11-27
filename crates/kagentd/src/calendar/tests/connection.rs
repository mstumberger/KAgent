//! The connection (calendar.md section 1): the URL, the callback, the token, the refresh,
//! `invalid_grant`, the revoke.

use chrono::Duration;

use super::super::fake::{CLIENT_ID, CLIENT_SECRET, REFRESH_TOKEN};
use super::super::{CalError, ConnState};
use super::rig;

fn texts(list: Vec<String>) -> Vec<String> {
    list
}

#[tokio::test]
async fn connect_needs_a_client_and_gives_the_url_of_the_spec() {
    let r = rig().await;
    assert!(matches!(r.cal.connect(), Err(CalError::Setup(m)) if m.contains("client id and the client secret")), "nothing pasted yet");
    assert!(r.cal.set_client("  ", Some("x")).await.is_err(), "an empty id");
    assert!(r.cal.set_client("id with spaces", None).await.is_err());
    r.cal.set_client(CLIENT_ID, None).await.unwrap();
    assert!(r.cal.connect().is_err(), "the id alone is not a client");
    assert!(!r.cal.has_client_secret());
    r.cal.set_client(CLIENT_ID, Some(CLIENT_SECRET)).await.unwrap();
    assert!(r.cal.has_client_secret());
    assert_eq!(r.settings.read().unwrap().calendar_config().client_id, CLIENT_ID);
    r.cal.set_client(CLIENT_ID, Some("")).await.unwrap();
    assert!(r.cal.has_client_secret(), "an empty secret keeps the one stored");
    let url = r.cal.connect().unwrap();
    assert!(url.starts_with(&format!("{}/o/oauth2/v2/auth?", r.fake.base)), "{url}");
    assert!(url.contains("code_challenge_method=S256") && url.contains("access_type=offline") && url.contains("127.0.0.1%3A7878%2Fapi%2Fcalendar%2Foauth%2Fcallback"), "{url}");
    assert!(!url.contains("client_secret") && !url.contains(CLIENT_SECRET), "the secret is never in the URL");
    assert_eq!(r.cal.state(), ConnState::Connecting);
}

#[tokio::test]
async fn the_callback_exchanges_the_code_stores_the_token_and_switches_the_module_on() {
    let mut r = rig().await;
    r.connect().await.unwrap();
    assert_eq!(r.cal.state(), ConnState::Connected);
    assert_eq!(texts(r.drain()).iter().filter(|e| e.starts_with("state:")).cloned().collect::<Vec<_>>(), ["state:connecting", "state:connected"]);

    // The exchange carried the verifier of the challenge, the client and the loopback redirect.
    let exchange = &r.fake.seen_at("/token")[0];
    assert!(exchange.body.contains("grant_type=authorization_code") && exchange.body.contains("code_verifier=") && exchange.body.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A7878"), "{}", exchange.body);

    // The refresh token and the client secret are in the 0600 file, one JSON text; the access token is not.
    use std::os::unix::fs::PermissionsExt;
    let file = r.secret_file();
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    let stored: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(stored["refreshToken"], REFRESH_TOKEN);
    assert_eq!(stored["clientSecret"], CLIENT_SECRET);
    assert!(!std::fs::read_to_string(&file).unwrap().contains("ya29."), "the access token lives in memory only");
    let settings_file = std::fs::read_to_string(r.dir.path().join("settings.json")).unwrap();
    assert!(!settings_file.contains(CLIENT_SECRET) && !settings_file.contains(REFRESH_TOKEN), "no secret in settings.json");

    // The first connection switches the module on and puts its tools where they are used.
    let s = r.settings.read().unwrap();
    assert!(s.calendar_config().enabled);
    assert!(s.tool_groups.iter().any(|g| g == "calendar"), "{:?}", s.tool_groups);
    assert!(s.section::<crate::sections::VoiceSection>("voice").unwrap().tool_groups.iter().any(|g| g == "calendar"));
}

#[tokio::test]
async fn a_second_connection_leaves_the_owners_choices_alone() {
    let r = rig().await;
    r.connect().await.unwrap();
    r.settings.write().unwrap().patch(serde_json::json!({"toolGroups": ["system"]})).unwrap();
    r.cal.disconnect().await.unwrap();
    r.connect().await.unwrap();
    assert_eq!(r.settings.read().unwrap().tool_groups, ["system"], "the owner took the group away: a new connection does not put it back");
}

#[tokio::test]
async fn a_callback_with_an_unknown_used_or_old_state_is_refused() {
    let r = rig().await;
    r.cal.set_client(CLIENT_ID, Some(CLIENT_SECRET)).await.unwrap();
    let url = reqwest::Url::parse(&r.cal.connect().unwrap()).unwrap();
    let q: std::collections::HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let code = r.fake.authorize(&q["code_challenge"]);
    let unknown = r.cal.callback(&code, "not-a-state").await.unwrap_err();
    assert!(matches!(&unknown, CalError::Refused(m) if m.contains("unknown, used or older than 10 minutes")), "{unknown}");
    assert!(r.fake.seen_at("/token").is_empty(), "nothing was sent to Google for an unknown state");
    r.cal.callback(&code, &q["state"]).await.unwrap();
    let again = r.cal.callback(&code, &q["state"]).await.unwrap_err();
    assert!(matches!(again, CalError::Refused(_)), "a state is used once");

    r.cal.disconnect().await.unwrap();
    let url = reqwest::Url::parse(&r.cal.connect().unwrap()).unwrap();
    let q: std::collections::HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let code = r.fake.authorize(&q["code_challenge"]);
    r.later(Duration::minutes(11));
    assert!(matches!(r.cal.callback(&code, &q["state"]).await, Err(CalError::Refused(_))), "older than 10 minutes");
    assert_eq!(r.cal.state(), ConnState::Off);
}

#[tokio::test]
async fn a_code_google_refuses_is_an_error_without_a_secret_and_the_state_says_so() {
    let r = rig().await;
    r.cal.set_client(CLIENT_ID, Some(CLIENT_SECRET)).await.unwrap();
    let url = reqwest::Url::parse(&r.cal.connect().unwrap()).unwrap();
    let state = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.into_owned();
    let e = r.cal.callback("4/never-issued", &state).await.unwrap_err();
    assert!(matches!(&e, CalError::Upstream(m) if m.contains("no longer accepts")), "{e}");
    assert!(!e.to_string().contains(CLIENT_SECRET));
    // The state of the failure stays visible until the owner tries again.
    let (state, why) = r.cal.state_and_error();
    assert!(state == ConnState::Error && why.is_some_and(|w| !w.contains(CLIENT_SECRET)));
    r.cal.connect().unwrap();
    assert_eq!(r.cal.state(), ConnState::Connecting);
}

#[tokio::test]
async fn the_access_token_is_kept_and_refreshed_a_minute_before_it_ends() {
    let r = rig().await;
    r.connect().await.unwrap();
    let first = r.cal.access_token().await.unwrap();
    assert_eq!(r.cal.access_token().await.unwrap(), first, "kept");
    assert_eq!(r.fake.seen_at("/token").len(), 1, "no refresh yet");
    r.later(Duration::seconds(3599 - 59));
    let second = r.cal.access_token().await.unwrap();
    assert_ne!(second, first, "refreshed with less than a minute left");
    let refresh = &r.fake.seen_at("/token")[1];
    assert!(refresh.body.contains("grant_type=refresh_token") && refresh.body.contains("refresh_token=1%2F%2Ffake-refresh-token-0001"), "{}", refresh.body);
    // Google said 401: the next call asks for a new one.
    r.cal.forget_access();
    assert_ne!(r.cal.access_token().await.unwrap(), second);
}

#[tokio::test]
async fn invalid_grant_sets_reconnect_raises_one_notice_and_deletes_nothing() {
    let mut r = rig().await;
    r.connect().await.unwrap();
    r.drain();
    r.fake.revoke_grant();
    r.later(Duration::hours(2));
    for _ in 0..3 {
        let e = r.cal.access_token().await.unwrap_err();
        assert!(matches!(&e, CalError::NotConnected(m) if m.contains("Reconnect")), "{e}");
    }
    assert_eq!(r.cal.state(), ConnState::Reconnect);
    let told = r.drain();
    assert_eq!(told.iter().filter(|e| e.starts_with("notice:")).count(), 1, "one notice, not one per call: {told:?}");
    assert_eq!(told.iter().filter(|e| e.starts_with("state:")).count(), 1, "{told:?}");
    let stored = std::fs::read_to_string(r.secret_file()).unwrap();
    assert!(stored.contains(REFRESH_TOKEN) && stored.contains(CLIENT_SECRET), "nothing is deleted");
    assert!(r.settings.read().unwrap().calendar_config().enabled, "nor switched off");

    // Connecting again clears it, and a later break is told again.
    r.connect().await.unwrap();
    assert_eq!(r.cal.state(), ConnState::Connected);
    r.fake.revoke_grant();
    r.later(Duration::hours(2));
    r.cal.access_token().await.unwrap_err();
    assert!(r.drain().iter().any(|e| e.starts_with("notice:")));
}

#[tokio::test]
async fn another_failure_of_the_token_call_is_an_error_not_a_reconnect() {
    let r = rig().await;
    r.connect().await.unwrap();
    r.later(Duration::hours(2));
    r.fake.state.lock().unwrap().token_error = Some("server_error".into());
    let e = r.cal.access_token().await.unwrap_err();
    assert!(matches!(&e, CalError::Upstream(m) if m.contains("server_error")), "{e}");
    assert_eq!(r.cal.state(), ConnState::Error);
    r.fake.state.lock().unwrap().token_error = None;
    r.cal.access_token().await.unwrap();
    assert_eq!(r.cal.state(), ConnState::Connected, "a good refresh clears an error");
}

#[tokio::test]
async fn disconnect_revokes_at_google_deletes_the_token_and_keeps_the_client() {
    let mut r = rig().await;
    r.connect().await.unwrap();
    r.drain();
    r.cal.disconnect().await.unwrap();
    assert_eq!(r.fake.state.lock().unwrap().revoked, [REFRESH_TOKEN], "Google was told to drop the token");
    assert_eq!(r.cal.state(), ConnState::Off);
    assert_eq!(r.drain(), ["state:off"]);
    let stored = std::fs::read_to_string(r.secret_file()).unwrap();
    assert!(!stored.contains(REFRESH_TOKEN) && stored.contains(CLIENT_SECRET), "the token is gone, the client stays: {stored}");
    assert_eq!(r.settings.read().unwrap().calendar_config().client_id, CLIENT_ID, "the settings stay");
    assert!(matches!(r.cal.access_token().await, Err(CalError::NotConnected(_))));
    r.cal.disconnect().await.unwrap();
    assert_eq!(r.fake.state.lock().unwrap().revoked.len(), 1, "nothing to revoke the second time");
}

#[tokio::test]
async fn a_restart_reads_the_connection_back() {
    let r = rig().await;
    r.connect().await.unwrap();
    let again = super::super::Calendar::new(super::super::Parts {
        settings: r.settings.clone(),
        settings_path: r.dir.path().join("settings.json"),
        secrets: crate::secrets::TokenStore::File(r.secret_file()),
        hosts: r.fake.hosts(),
        redirect: "http://127.0.0.1:7878/api/calendar/oauth/callback".into(),
        events: tokio::sync::broadcast::channel(8).0,
        clock: std::sync::Arc::new(chrono::Utc::now),
        zone: super::zone(1),
        data_dir: r.dir.path().join("data"),
        maps: crate::secrets::TokenStore::File(r.dir.path().join("maps.secret")),
        web: None,
        notifier: None,
        paired: std::sync::Arc::new(|| false),
        tasks: std::sync::Arc::new(|_| Vec::new()),
    })
    .await;
    assert_eq!(again.state(), ConnState::Connected);
    assert!(again.has_client_secret());
}
