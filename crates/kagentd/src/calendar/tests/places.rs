//! Places and travel (calendar.md sections 5 and 9): the field mask, the bias, the web
//! fallback with `source: "web"`, the key that is in no error, and the drive arithmetic.

use std::sync::{Arc, OnceLock, RwLock};

use chrono::Duration;
use kagent_core::tool::{Tool, ToolContext};
use serde_json::{json, Value};

use super::super::fake::MAPS_KEY;
use super::super::place_tool::PlaceSearch;
use super::super::places::{Source, PLACES_MASK, ROUTES_MASK};
use super::{rig, rig_with, Rig};
use crate::web::fake::{Fake as WebFake, MemoryKeys, Reply};
use crate::web::{Engines, SettingsCell, Web};

async fn run(r: &Rig, input: Value) -> (bool, String) {
    let out = PlaceSearch::new(r.cell()).call(input, ToolContext::standalone(std::time::Duration::from_secs(5), 24_000)).await.unwrap();
    (out.is_error, out.content)
}

/// DuckDuckGo's page with two results: the first has no address, the second has.
const PAGE: &str = "<div class=\"result results_links web-result \">\
    <h2 class=\"result__title\"><a class=\"result__a\" href=\"https://a.example/\">Thai Orchid Restaurant</a></h2>\
    <a class=\"result__snippet\" href=\"https://a.example/\">The best thai food in town, open daily.</a></div>\
    <div class=\"result results_links web-result \">\
    <h2 class=\"result__title\"><a class=\"result__a\" href=\"https://b.example/\">Thai Orchid - Maribor</a></h2>\
    <a class=\"result__snippet\" href=\"https://b.example/\">Find us at Glavni trg 5, 2000 Maribor. Reservations welcome.</a></div>";

async fn web_with(page: &str) -> (Arc<Web>, WebFake) {
    let fake = WebFake::start(&[("/html/", Reply::ok("text/html; charset=UTF-8", page.as_bytes()))]).await;
    let cell: SettingsCell = Arc::new(OnceLock::new());
    cell.set(Arc::new(RwLock::new(crate::settings::Settings::defaults()))).unwrap();
    let web = Web::with(cell, Arc::new(MemoryKeys::default()), fake.egress(), Engines { duckduckgo: fake.url("/html/"), brave: fake.url("/brave") });
    (Arc::new(web), fake)
}

async fn mapped() -> Rig {
    let r = rig().await;
    r.ready(&["primary"]).await;
    r.cal.set_maps_key(Some(MAPS_KEY)).await.unwrap();
    r.fake.add_place("Thai Orchid", "Glavni trg 5, 2000 Maribor", 46.5547, 15.6459);
    r.fake.add_place("Kardeljeva", "Kardeljeva ploščad 1, Ljubljana", 46.0569, 14.5058);
    r
}

#[tokio::test]
async fn places_is_asked_with_the_field_mask_and_the_key_in_a_header_only() {
    let r = mapped().await;
    let (err, text) = run(&r, json!({"query": "Thai Orchid", "near": "Maribor"})).await;
    assert!(!err, "{text}");
    assert!(text.contains("(source: Google Places)") && text.contains("1. Thai Orchid | address: Glavni trg 5, 2000 Maribor | mapsUrl https://maps.google.com/?cid=") && text.contains("source places"), "{text}");
    assert!(!text.contains("could not check this on the map"), "{text}");
    let asked = r.fake.seen_at("/v1/places:searchText");
    let main = asked.last().unwrap();
    assert_eq!((main.key.as_deref(), main.field_mask.as_deref()), (Some(MAPS_KEY), Some(PLACES_MASK)));
    assert!(!main.path.contains(MAPS_KEY) && !main.query.contains(MAPS_KEY) && !main.body.contains(MAPS_KEY), "the key is in no URL and no body");
    assert_eq!(main.json()["textQuery"], "Thai Orchid");
    assert_eq!(main.json()["maxResultCount"], 3);
    assert!(PLACES_MASK.split(',').all(|f| f.starts_with("places.")) && !PLACES_MASK.contains("reviews"), "only the four fields");
}

#[tokio::test]
async fn the_search_is_biased_to_near_or_else_to_home_which_is_resolved_once() {
    let r = mapped().await;
    // near: the place named is looked up first, and its coordinates are the centre.
    run(&r, json!({"query": "Thai Orchid", "near": "Thai Orchid"})).await;
    let calls = r.fake.seen_at("/v1/places:searchText");
    assert_eq!(calls[0].json()["maxResultCount"], 1, "near is resolved first");
    let bias = &calls[1].json()["locationBias"]["circle"];
    assert_eq!((bias["center"]["latitude"].as_f64(), bias["center"]["longitude"].as_f64(), bias["radius"].as_f64()), (Some(46.5547), Some(15.6459), Some(20000.0)));

    // No near: home, looked up once for its text and then kept.
    r.settings.write().unwrap().patch(json!({"calendar": {"home": "Kardeljeva ploščad 1, Ljubljana"}})).unwrap();
    let before = r.fake.seen_at("/v1/places:searchText").len();
    run(&r, json!({"query": "Thai Orchid"})).await;
    let calls = r.fake.seen_at("/v1/places:searchText");
    assert_eq!(calls.len() - before, 2, "home, then the search");
    assert_eq!(calls[before + 1].json()["locationBias"]["circle"]["center"]["latitude"].as_f64(), Some(46.0569));
    run(&r, json!({"query": "Thai Orchid"})).await;
    assert_eq!(r.fake.seen_at("/v1/places:searchText").len() - before, 3, "home is not asked again");
    let saved: Value = serde_json::from_slice(&std::fs::read(r.dir.path().join("data/calendar-state.json")).unwrap()).unwrap();
    assert_eq!((saved["homeFor"].as_str(), saved["homeLatLng"][0].as_f64()), (Some("Kardeljeva ploščad 1, Ljubljana"), Some(46.0569)));
    // Another home text is looked up again.
    r.settings.write().unwrap().patch(json!({"calendar": {"home": "Glavni trg 5, 2000 Maribor"}})).unwrap();
    assert_eq!(r.cal.home_lat_lng().await, Some([46.5547, 15.6459]));
    // Without a home and without near there is no bias.
    r.settings.write().unwrap().patch(json!({"calendar": {"home": ""}})).unwrap();
    run(&r, json!({"query": "Thai Orchid"})).await;
    assert!(r.fake.seen_at("/v1/places:searchText").last().unwrap().json().get("locationBias").is_none());
}

#[tokio::test]
async fn without_a_key_the_web_gives_the_address_and_the_answer_says_it_is_not_from_the_map() {
    let (web, _fake) = web_with(PAGE).await;
    let r = rig_with(Some(web)).await;
    r.ready(&["primary"]).await;
    let (err, text) = run(&r, json!({"query": "Thai Orchid Maribor"})).await;
    assert!(!err, "{text}");
    assert!(text.contains("(source: web search)"), "{text}");
    assert!(text.contains("1. Thai Orchid | address: Glavni trg 5, 2000 Maribor | mapsUrl https://www.google.com/maps/search/?api=1&query=Thai%20Orchid%2C%20Glavni%20trg%205%2C%202000%20Maribor | source web"), "the address is from the first result that has one: {text}");
    assert!(text.contains("I could not check this on the map"), "{text}");
    assert!(r.fake.seen_at("/v1/places:searchText").is_empty(), "no key, no Places call");
    let (found, _) = r.cal.find_place("Thai Orchid Maribor", None).await.unwrap();
    assert_eq!(found[0].source, Source::Web);
}

#[tokio::test]
async fn a_failed_places_call_falls_back_to_the_web_and_the_key_is_in_no_error() {
    let (web, _fake) = web_with(PAGE).await;
    let r = rig_with(Some(web)).await;
    r.ready(&["primary"]).await;
    let wrong = "AIzaSyWrongKey-111111111111111111111";
    r.cal.set_maps_key(Some(wrong)).await.unwrap();
    let (err, text) = run(&r, json!({"query": "Thai Orchid Maribor"})).await;
    assert!(!err && text.contains("source web") && text.contains("I could not check this on the map"), "{text}");
    assert!(text.contains("Google Maps refused the key"), "the owner can see why: {text}");
    assert!(!text.contains(wrong), "Google's message repeated the key; it is not passed on: {text}");
    // The map is down: the same.
    r.cal.set_maps_key(Some(MAPS_KEY)).await.unwrap();
    r.fake.state.lock().unwrap().maps_down = true;
    let (err, text) = run(&r, json!({"query": "Thai Orchid Maribor"})).await;
    assert!(!err && text.contains("source web") && text.contains("Google Maps answered 500"), "{text}");
    assert!(!text.contains(MAPS_KEY));
}

#[tokio::test]
async fn nothing_found_and_no_web_is_an_error_that_says_so() {
    let r = rig().await;
    r.ready(&["primary"]).await;
    let (err, text) = run(&r, json!({"query": "Nowhere Cafe"})).await;
    assert!(err && text.contains("no Maps key is set") && text.contains("web search is not available"), "{text}");
    let (web, _fake) = web_with("<html><body>nothing</body></html>").await;
    let r = rig_with(Some(web)).await;
    r.ready(&["primary"]).await;
    let (err, text) = run(&r, json!({"query": "Nowhere Cafe"})).await;
    assert!(err && text.contains("web search failed"), "{text}");
    let (err, text) = run(&r, json!({"query": "  "})).await;
    assert!(err && text.contains("query required"), "{text}");
    let (err, _) = run(&r, json!({"query": "x".repeat(201)})).await;
    assert!(err);
}

#[tokio::test]
async fn the_drive_asks_for_the_no_traffic_time_then_for_traffic_at_that_departure() {
    let r = mapped().await;
    let arrive = r.cal.now() + Duration::hours(2);
    let drive = r.cal.drive([46.0569, 14.5058], [46.5547, 15.6459], arrive).await.unwrap();
    assert_eq!((drive.no_traffic, drive.with_traffic), (1500, 1800));
    let asked = r.fake.seen_at("/directions/v2:computeRoutes");
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[0].field_mask.as_deref(), Some(ROUTES_MASK));
    let body = asked[1].json();
    assert_eq!((body["travelMode"].as_str(), body["routingPreference"].as_str()), (Some("DRIVE"), Some("TRAFFIC_AWARE")));
    let departure = body["departureTime"].as_str().unwrap();
    assert_eq!(departure, (arrive - Duration::seconds(1500)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true), "the event's start minus the no-traffic duration");
    assert!(asked[0].json().get("departureTime").is_none());
    // An event that starts in 10 minutes: leaving 25 minutes before it is in the past, so now.
    r.cal.drive([1.0, 1.0], [2.0, 2.0], r.cal.now() + Duration::minutes(10)).await.unwrap();
    let late = r.fake.seen_at("/directions/v2:computeRoutes").last().unwrap().json();
    assert_eq!(late["departureTime"], (r.cal.now() + Duration::seconds(30)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    // Without a key there is no drive.
    r.cal.set_maps_key(None).await.unwrap();
    assert!(r.cal.drive([1.0, 1.0], [2.0, 2.0], arrive).await.is_err());
}

#[tokio::test]
async fn the_maps_key_is_checked_stored_privately_and_forgotten() {
    let r = rig().await;
    assert!(r.cal.set_maps_key(Some("short")).await.is_err(), "a key has a shape");
    assert!(!r.cal.has_maps_key().await);
    r.cal.set_maps_key(Some(&format!("  {MAPS_KEY} \n"))).await.unwrap();
    assert!(r.cal.has_maps_key().await);
    use std::os::unix::fs::PermissionsExt;
    let file = r.dir.path().join("maps.secret");
    assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), MAPS_KEY);
    r.cal.set_maps_key(None).await.unwrap();
    assert!(!r.cal.has_maps_key().await && !file.exists());
}
