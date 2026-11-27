//! The routes' work (calendar.md section 7): the view, the callback page, the maps test and
//! the status of each refusal. The bearer rule of the router was checked against a running
//! daemon (the report of S45); the handlers are thin over these functions.

use std::net::IpAddr;

use axum::http::StatusCode;
use serde_json::json;

use super::super::fake::{CLIENT_ID, CLIENT_SECRET, MAPS_KEY, REFRESH_TOKEN};
use super::super::fake_api::{FAMILY, HOLIDAYS};
use super::super::http::{callback_page, http_error, CallbackQuery};
use super::super::CalError;
use super::rig;

fn query(code: Option<&str>, state: Option<&str>, error: Option<&str>) -> CallbackQuery {
    CallbackQuery { code: code.map(String::from), state: state.map(String::from), error: error.map(String::from) }
}

const LOOPBACK: &str = "127.0.0.1";

#[tokio::test]
async fn the_callback_connects_on_a_known_state_from_loopback_and_says_so_in_a_small_page() {
    let r = rig().await;
    r.cal.set_client(CLIENT_ID, Some(CLIENT_SECRET)).await.unwrap();
    let url = reqwest::Url::parse(&r.cal.connect().unwrap()).unwrap();
    let q: std::collections::HashMap<String, String> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let code = r.fake.authorize(&q["code_challenge"]);
    let peer: IpAddr = LOOPBACK.parse().unwrap();

    // Not from this machine: refused before anything is looked at.
    let (status, page) = callback_page(&r.cal, "203.0.113.9".parse().unwrap(), query(Some(&code), Some(&q["state"]), None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(page.contains("this machine only") && r.fake.seen_at("/token").is_empty());
    // Google said no, or the link is not an answer.
    let (status, page) = callback_page(&r.cal, peer, query(None, None, Some("access_denied"))).await;
    assert!(status == StatusCode::BAD_REQUEST && page.contains("access_denied"), "{page}");
    assert_eq!(callback_page(&r.cal, peer, query(None, None, None)).await.0, StatusCode::BAD_REQUEST);
    // An unknown state, then the right one.
    let (status, page) = callback_page(&r.cal, peer, query(Some(&code), Some("<script>alert(1)</script>"), None)).await;
    assert!(status == StatusCode::BAD_REQUEST && page.contains("unknown, used or older"), "{page}");
    assert!(!page.contains("<script>"), "nothing of the request is put in the page unescaped");
    let (status, page) = callback_page(&r.cal, peer, query(Some(&code), Some(&q["state"]), None)).await;
    assert!(status == StatusCode::OK && page.contains("<h1>Connected</h1>") && page.contains("You can close this tab"), "{page}");
    assert!(!page.contains(&code) && !page.contains(REFRESH_TOKEN));
    // The same link again: used.
    assert_eq!(callback_page(&r.cal, peer, query(Some(&code), Some(&q["state"]), None)).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_view_lists_the_accounts_calendars_with_the_switches_and_no_secret() {
    let r = rig().await;
    let off = r.cal.view().await;
    assert_eq!((off["state"].as_str(), off["hasClientSecret"].as_bool(), off["hasMapsKey"].as_bool(), off["calendars"].as_array().map(Vec::len)), (Some("off"), Some(false), Some(false), Some(0)));
    r.ready(&["primary", FAMILY]).await;
    r.cal.set_maps_key(Some(MAPS_KEY)).await.unwrap();
    let v = r.cal.view().await;
    assert_eq!((v["state"].as_str(), v["account"].as_str(), v["hasClientSecret"].as_bool(), v["hasMapsKey"].as_bool()), (Some("connected"), Some("owner@example.com"), Some(true), Some(true)));
    let list = v["calendars"].as_array().unwrap();
    assert_eq!(list.len(), 4);
    let find = |id: &str| list.iter().find(|c| c["id"] == id).unwrap();
    assert_eq!((find("owner@example.com")["on"].clone(), find("owner@example.com")["writable"].clone(), find("owner@example.com")["name"].clone()), (json!(true), json!(true), json!("Personal")), "primary is on by its alias");
    assert_eq!((find(FAMILY)["on"].clone(), find(HOLIDAYS)["on"].clone(), find(HOLIDAYS)["writable"].clone()), (json!(true), json!(false), json!(false)));
    assert_eq!(find(HOLIDAYS)["color"], "#0b8043");
    // Every call of the Calendar API carried an access token, and no token call did.
    assert!(r.fake.seen_at("/calendar/v3").iter().all(|s| s.bearer.as_deref().is_some_and(|t| t.starts_with("ya29."))));
    assert!(r.fake.seen_at("/token").iter().all(|s| s.bearer.is_none()));
    let text = v.to_string();
    for secret in [CLIENT_SECRET, REFRESH_TOKEN, MAPS_KEY, "ya29."] {
        assert!(!text.contains(secret), "{secret} in the view");
    }
    // A calendar list that fails says so, and is not an empty list.
    r.fake.expire_access_tokens();
    r.fake.revoke_grant();
    let broken = r.cal.view().await;
    assert!(broken["error"].as_str().is_some_and(|e| e.contains("Reconnect")) && broken["state"] != "connected", "{broken}");
}

#[tokio::test]
async fn the_maps_test_is_one_search_for_ljubljana_on_the_map() {
    let r = rig().await;
    assert_eq!(r.cal.maps_test().await, json!({"ok": false, "error": "no Maps key is set"}));
    r.cal.set_maps_key(Some(MAPS_KEY)).await.unwrap();
    let nothing = r.cal.maps_test().await;
    assert_eq!(nothing["ok"], false, "{nothing}");
    r.fake.add_place("Ljubljana", "Ljubljana, Slovenia", 46.05, 14.5);
    assert_eq!(r.cal.maps_test().await, json!({"ok": true, "place": {"name": "Ljubljana", "address": "Ljubljana, Slovenia"}}));
    assert_eq!(r.fake.seen_at("/v1/places:searchText").last().unwrap().json()["textQuery"], "Ljubljana");
    r.cal.set_maps_key(Some("AIzaSyWrongKey-111111111111111111111")).await.unwrap();
    let wrong = r.cal.maps_test().await;
    assert!(wrong["ok"] == false && wrong["error"].as_str().is_some_and(|e| e.contains("refused the key") && !e.contains("AIzaSyWrong")), "{wrong}");
}

#[test]
fn each_refusal_has_its_status() {
    let status = |e: CalError| http_error(e).0;
    assert_eq!(status(CalError::Setup("x".into())), StatusCode::BAD_REQUEST);
    assert_eq!(status(CalError::Refused("x".into())), StatusCode::BAD_REQUEST);
    assert_eq!(status(CalError::NotConnected("x".into())), StatusCode::CONFLICT);
    assert_eq!(status(CalError::Upstream("x".into())), StatusCode::BAD_GATEWAY);
}
