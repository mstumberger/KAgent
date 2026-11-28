use super::*;
use crate::protocol::Message;

fn png(w: u32, h: u32) -> Vec<u8> {
    // Two colours: a screenshot-like picture.
    let img = image::RgbImage::from_fn(w, h, |x, _| if x % 7 < 3 { image::Rgb([20, 20, 20]) } else { image::Rgb([240, 240, 240]) });
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
    out
}

fn photo_jpeg(w: u32, h: u32) -> Vec<u8> {
    // Many colours: a photograph-like picture.
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7 % 256) as u8, (y * 11 % 256) as u8, ((x + y) * 3 % 256) as u8]));
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(img).write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg).unwrap();
    out
}

/// A JPEG with an EXIF APP1 segment (with a fake GPS tag) right after the start marker.
fn with_exif(jpeg: &[u8]) -> Vec<u8> {
    let payload = b"Exif\0\0GPSLatitude=46.05N";
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.extend(payload);
    out.extend(&jpeg[2..]);
    out
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn webp(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 7, 255]));
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), ImageFormat::WebP).unwrap();
    out
}

#[test]
fn a_screenshot_stays_png_and_a_photo_becomes_jpeg() {
    let shot = reencode(&png(300, 200), MAX_SIDE, false).unwrap();
    assert_eq!((shot.mime, shot.width, shot.height), ("image/png", 300, 200));
    let photo = reencode(&photo_jpeg(300, 200), MAX_SIDE, false).unwrap();
    assert_eq!(photo.mime, "image/jpeg", "many colours: JPEG 85");
    assert_eq!(&photo.bytes[..2], &[0xFF, 0xD8]);
    let w = reencode(&webp(64, 48), MAX_SIDE, false).unwrap();
    assert_eq!((w.width, w.height), (64, 48), "WebP is accepted");
}

#[test]
fn the_longest_side_is_cut_to_1568() {
    let big = reencode(&png(3136, 1000), MAX_SIDE, false).unwrap();
    assert_eq!((big.width, big.height), (1568, 500), "the ratio is kept");
    let tall = reencode(&photo_jpeg(800, 2000), MAX_SIDE, false).unwrap();
    assert_eq!(tall.height, 1568);
    let frame = to_frame(&png(3000, 1500), 1024).unwrap();
    assert_eq!((frame.mime, frame.width, frame.height), ("image/jpeg", 1024, 512), "a live frame is a JPEG of 1024 px");
}

#[test]
fn no_exif_and_no_text_chunks_survive() {
    let src = with_exif(&photo_jpeg(120, 80));
    assert!(contains(&src, b"GPSLatitude"), "the fixture has a location");
    let out = reencode(&src, MAX_SIDE, false).unwrap();
    assert!(!contains(&out.bytes, b"Exif") && !contains(&out.bytes, b"GPSLatitude"), "metadata is dropped");
    // A PNG text chunk is dropped too.
    let mut p = png(40, 40);
    let iend = p.len() - 12;
    let mut chunk = Vec::new();
    chunk.extend(23u32.to_be_bytes());
    chunk.extend(b"tEXt");
    chunk.extend(b"Comment\0owner@home.example");
    chunk.extend(0u32.to_be_bytes()); // wrong crc: the decoder skips ancillary text
    p.splice(iend..iend, chunk);
    if let Ok(out) = reencode(&p, MAX_SIDE, false) {
        assert!(!contains(&out.bytes, b"owner@home"), "text chunks are dropped");
    }
}

#[test]
fn other_types_and_big_uploads_are_refused() {
    assert!(matches!(reencode(b"GIF89a\x01\x00\x01\x00", MAX_SIDE, false), Err(ImageError::UnsupportedType)), "GIF is a 415");
    assert!(matches!(reencode(b"%PDF-1.7 not an image", MAX_SIDE, false), Err(ImageError::UnsupportedType)));
    assert!(matches!(reencode(&[], MAX_SIDE, false), Err(ImageError::UnsupportedType)));
    let mut huge = png(10, 10);
    huge.resize(MAX_UPLOAD_BYTES + 1, 0);
    assert!(matches!(reencode(&huge, MAX_SIDE, false), Err(ImageError::TooLarge(n)) if n == MAX_UPLOAD_BYTES + 1), "8 MB and a byte is a 413");
    let mut cut = png(50, 50);
    cut.truncate(40);
    assert!(matches!(reencode(&cut, MAX_SIDE, false), Err(ImageError::Unreadable(_))), "a PNG that does not decode is refused with the reason");
    let msg = ImageError::TooLarge(9).to_string();
    assert!(msg.contains("send a smaller one"), "an error says what to do instead: {msg}");
}

#[tokio::test]
async fn an_image_round_trips_through_the_session_file_and_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let image = store.put_image(&meta.id, png(100, 60)).await.unwrap();
    assert_eq!((image.mime.as_str(), image.width, image.height), ("image/png", 100, 60));
    assert!(store.images_dir(&meta.id).join(format!("{}.png", image.id)).is_file(), "the pixels are a file beside the session");
    store.append(&meta.id, &[Message::user("look").with_images(vec![image.clone()])]).await.unwrap();
    let line = std::fs::read_to_string(dir.path().join(format!("{}.jsonl", meta.id))).unwrap();
    assert!(line.contains("\"images\"") && !line.contains("data"), "the file has the reference, not the pixels: {line}");
    let plain = std::fs::read_to_string(dir.path().join(format!("{}.jsonl", meta.id))).unwrap();
    assert!(!plain.contains("iVBOR"), "no base64 in the JSONL");

    // A new store over the same folder: a restart.
    let again = SessionStore::new(dir.path()).unwrap();
    let msgs = again.load_messages(&meta.id).await.unwrap();
    assert_eq!(msgs[0].images, vec![image.clone()]);
    let (bytes, mime) = again.image_file(&meta.id, &image.id).await.unwrap();
    assert_eq!((mime.as_str(), bytes.len() as u64), ("image/png", image.bytes));
    assert_eq!(again.image_refs(&meta.id, &[image.id.clone()]).await.unwrap(), vec![image]);
    // A message without images has no key.
    store.append(&meta.id, &[Message::user("plain")]).await.unwrap();
    let raw = std::fs::read_to_string(dir.path().join(format!("{}.jsonl", meta.id))).unwrap();
    assert_eq!(raw.lines().last().unwrap().contains("images"), false, "empty is left out of the file");
}

#[tokio::test]
async fn at_most_four_images_and_only_this_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let (a, b) = (store.create("a").await.unwrap(), store.create("b").await.unwrap());
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(store.put_image(&a.id, png(20, 20)).await.unwrap().id);
    }
    assert!(store.image_refs(&a.id, &ids[..4]).await.is_ok());
    let err = store.image_refs(&a.id, &ids).await.unwrap_err();
    assert!(matches!(err, ImageError::TooMany(5)), "{err}");
    assert!(matches!(store.image_refs(&b.id, &ids[..1]).await, Err(ImageError::NotFound(_))), "another session's image is not found");
    assert!(matches!(store.image_ref(&a.id, "../../etc/passwd").await, Err(ImageError::NotFound(_))), "ids are not paths");
}

#[tokio::test]
async fn images_are_deleted_with_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path()).unwrap();
    let meta = store.create("t").await.unwrap();
    let image = store.put_image(&meta.id, photo_jpeg(50, 50)).await.unwrap();
    assert!(store.images_dir(&meta.id).is_dir());
    store.delete(&meta.id).await.unwrap();
    assert!(!store.images_dir(&meta.id).exists(), "the folder goes with the session");
    assert!(matches!(store.image_file(&meta.id, &image.id).await, Err(ImageError::NotFound(_))));
}
