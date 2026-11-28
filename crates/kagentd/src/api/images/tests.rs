use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use kagent_core::session::SessionStore;
use serde_json::{json, Value};

use super::*;

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(w, h, |x, _| if x % 5 < 2 { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

async fn upload_route(State(store): State<Arc<SessionStore>>, Path(id): Path<String>, form: Multipart) -> ApiResult<Response> {
    handle_upload(&store, &id, form).await
}

/// The route as the daemon mounts it (its own body limit), on a loopback port.
async fn serve(store: Arc<SessionStore>) -> String {
    let app = Router::new().route("/sessions/{id}/images", post(upload_route).layer(DefaultBodyLimit::max(UPLOAD_BODY_BYTES))).layer(DefaultBodyLimit::max(2 * 1024 * 1024)).with_state(store);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

fn form(field: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = "kagent-test-boundary";
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"a.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
    body.extend(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

async fn post_file(url: &str, session: &str, field: &str, bytes: &[u8]) -> (u16, Value) {
    let (ctype, body) = form(field, bytes);
    let resp = reqwest::Client::new().post(format!("{url}/sessions/{session}/images")).header("content-type", ctype).body(body).send().await.unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn an_upload_answers_the_image_ref_and_limits_are_http_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(dir.path()).unwrap());
    let meta = store.create("t").await.unwrap();
    let url = serve(store.clone()).await;

    let (status, image) = post_file(&url, &meta.id, "file", &png(64, 32)).await;
    assert_eq!(status, 200, "{image}");
    assert_eq!((image["mime"].as_str(), image["width"].as_u64(), image["height"].as_u64()), (Some("image/png"), Some(64), Some(32)));
    assert!(image["id"].as_str().is_some_and(|id| !id.is_empty()) && image["bytes"].as_u64().unwrap() > 0, "{image}");
    assert!(store.image_ref(&meta.id, image["id"].as_str().unwrap()).await.is_ok(), "the file is in the session's folder");

    let (status, err) = post_file(&url, &meta.id, "file", b"GIF89a\x01\x00\x01\x00").await;
    assert_eq!(status, 415, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("PNG, JPEG and WebP"), "the error says what to do: {err}");

    let mut big = png(8, 8);
    big.resize(8 * 1024 * 1024 + 1, 0);
    let (status, err) = post_file(&url, &meta.id, "file", &big).await;
    assert_eq!(status, 413, "{err}");
    assert!(err["error"]["message"].as_str().unwrap().contains("8 MB"), "{err}");

    let (status, err) = post_file(&url, &meta.id, "other", &png(8, 8)).await;
    assert_eq!(status, 400, "{err}");
    let (status, err) = post_file(&url, "no-such-session", "file", &png(8, 8)).await;
    assert_eq!(status, 404, "{err}");
}

#[tokio::test]
async fn a_message_names_images_by_id_at_most_four() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(upload(&store, &meta.id, png(16, 16)).await.unwrap().id);
    }
    assert_eq!(message_images(&store, &meta.id, &ids[..4]).await.unwrap().len(), 4);
    let err = message_images(&store, &meta.id, &ids).await.unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST, "five images in one message");
    assert!(err.2.contains("at most 4"), "{}", err.2);
    let err = message_images(&store, &meta.id, &["ffff".to_string()]).await.unwrap_err();
    assert_eq!(err.0, StatusCode::NOT_FOUND);

    assert_eq!(image_ids(&json!({"text": "x"})).unwrap(), Vec::<String>::new());
    assert_eq!(image_ids(&json!({"images": ["a", "b"]})).unwrap(), vec!["a", "b"]);
    assert!(image_ids(&json!({"images": "a"})).is_err() && image_ids(&json!({"images": [1]})).is_err());
}

#[test]
fn every_image_error_has_its_status() {
    let status = |e: ImageError| http_error(e).0.as_u16();
    assert_eq!(status(ImageError::TooLarge(9)), 413);
    assert_eq!(status(ImageError::UnsupportedType), 415);
    assert_eq!(status(ImageError::Unreadable("x".into())), 400);
    assert_eq!(status(ImageError::TooMany(5)), 400);
    assert_eq!(status(ImageError::NotFound("x".into())), 404);
}
