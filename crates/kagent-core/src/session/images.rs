//! The images of a session (vision.md section 1): re-encoded once, kept as files in
//! `<sessions dir>/<sessionId>.images/`, deleted with the session.
//!
//! Every limit is a check here, not an instruction to a client: the type, the size as
//! uploaded, the longest side, the number per message.

use std::io::Cursor;
use std::path::PathBuf;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::protocol::ImageRef;

use super::SessionStore;

/// Images in one message.
pub const MAX_IMAGES: usize = 4;
/// Bytes of one upload.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
/// The longest side that is kept (pixels).
pub const MAX_SIDE: u32 = 1568;
/// A picture with at most this many distinct colours (a screenshot) stays PNG, else JPEG 85.
const PNG_COLOURS: usize = 8192;
const JPEG_QUALITY: u8 = 85;
/// A decoded image over this many pixels is refused: a small file can expand to gigabytes.
const MAX_PIXELS: u64 = 64_000_000;

#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    /// 413: more than 8 MB.
    #[error("the image is {0} bytes; the limit is 8 MB: send a smaller one")]
    TooLarge(usize),
    /// 415: not PNG, JPEG or WebP.
    #[error("only PNG, JPEG and WebP images are accepted: convert the file and send it again")]
    UnsupportedType,
    /// 400: the type was right but the file does not decode.
    #[error("the image could not be read ({0}): send another file")]
    Unreadable(String),
    /// 404.
    #[error("no image {0} in this session")]
    NotFound(String),
    /// 400: a message takes 4 images.
    #[error("a message takes at most {MAX_IMAGES} images; this one has {0}: remove some")]
    TooMany(usize),
    #[error("{0}")]
    Io(String),
}

/// What `reencode` made.
pub struct Encoded {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Decode PNG, JPEG or WebP, apply the EXIF orientation, shrink so the longest side is at
/// most `max_side`, and write PNG (few colours) or JPEG 85. The output carries no metadata
/// (no EXIF, no location): the encoders write none.
pub fn reencode(input: &[u8], max_side: u32, jpeg: bool) -> Result<Encoded, ImageError> {
    if input.len() > MAX_UPLOAD_BYTES {
        return Err(ImageError::TooLarge(input.len()));
    }
    let format = image::guess_format(input).map_err(|_| ImageError::UnsupportedType)?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP) {
        return Err(ImageError::UnsupportedType);
    }
    let unreadable = |e: image::ImageError| ImageError::Unreadable(e.to_string());
    let mut decoder = ImageReader::with_format(Cursor::new(input), format).into_decoder().map_err(unreadable)?;
    let (w, h) = decoder.dimensions();
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(ImageError::Unreadable(format!("{w}x{h} pixels is too large")));
    }
    let orientation = decoder.orientation().map_err(unreadable)?;
    let mut img = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    img.apply_orientation(orientation);
    if img.width().max(img.height()) > max_side {
        img = img.resize(max_side, max_side, image::imageops::FilterType::Lanczos3);
    }
    let rgba = img.to_rgba8();
    let few_colours = !jpeg && has_few_colours(&rgba);
    let (width, height) = (rgba.width(), rgba.height());
    let mut out = Vec::new();
    if few_colours {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(unreadable)?;
        return Ok(Encoded { bytes: out, mime: "image/png", width, height });
    }
    // JPEG has no alpha: flatten onto white.
    let mut rgb = image::RgbImage::new(width, height);
    for (x, y, p) in rgba.enumerate_pixels() {
        let a = u32::from(p.0[3]);
        let over = |c: u8| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8;
        rgb.put_pixel(x, y, image::Rgb([over(p.0[0]), over(p.0[1]), over(p.0[2])]));
    }
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&rgb).map_err(unreadable)?;
    Ok(Encoded { bytes: out, mime: "image/jpeg", width, height })
}

fn has_few_colours(img: &image::RgbaImage) -> bool {
    let mut seen = std::collections::HashSet::new();
    for p in img.pixels() {
        seen.insert(p.0);
        if seen.len() > PNG_COLOURS {
            return false;
        }
    }
    true
}

/// A JPEG of the longest side `max_side` for a live video frame (voice, vision.md section 3).
pub fn to_frame(input: &[u8], max_side: u32) -> Result<Encoded, ImageError> {
    reencode(input, max_side, true)
}

/// A grey thumbnail of `w` x `h` pixels (one byte each) of a PNG, JPEG or WebP: what screen watch compares
/// between two looks (screen-watch.md section 2). Nothing is kept.
pub fn thumb_gray(input: &[u8], w: u32, h: u32) -> Result<Vec<u8>, ImageError> {
    let unreadable = |e: image::ImageError| ImageError::Unreadable(e.to_string());
    let reader = ImageReader::new(Cursor::new(input)).with_guessed_format().map_err(|e| ImageError::Unreadable(e.to_string()))?;
    let img = reader.decode().map_err(unreadable)?;
    Ok(img.resize_exact(w, h, image::imageops::FilterType::Triangle).to_luma8().into_raw())
}

/// A PNG of `rgb` (three bytes a pixel, `w` x `h`): a window the X server handed over as raw pixels.
pub fn png_of_rgb(w: u32, h: u32, rgb: Vec<u8>) -> Result<Vec<u8>, ImageError> {
    let img = image::RgbImage::from_raw(w, h, rgb).ok_or_else(|| ImageError::Unreadable("the pixels do not fit the size".into()))?;
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| ImageError::Unreadable(e.to_string()))?;
    Ok(out)
}

fn ext_of(mime: &str) -> &'static str {
    if mime == "image/png" { "png" } else { "jpg" }
}

fn clean(id: &str) -> String {
    id.chars().filter(char::is_ascii_alphanumeric).collect()
}

impl SessionStore {
    /// `<sessions dir>/<id>.images`.
    pub fn images_dir(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{}.images", super::sanitize(session_id)))
    }

    /// Re-encode `bytes` and keep the result as an image of the session.
    pub async fn put_image(&self, session_id: &str, bytes: Vec<u8>) -> Result<ImageRef, ImageError> {
        let enc = tokio::task::spawn_blocking(move || reencode(&bytes, MAX_SIDE, false)).await.map_err(|e| ImageError::Io(e.to_string()))??;
        let dir = self.images_dir(session_id);
        tokio::fs::create_dir_all(&dir).await.map_err(|e| ImageError::Io(e.to_string()))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let image = ImageRef { id: id.clone(), mime: enc.mime.into(), width: enc.width, height: enc.height, bytes: enc.bytes.len() as u64, description: None, read_by: None, data: None };
        let io = |e: std::io::Error| ImageError::Io(e.to_string());
        tokio::fs::write(dir.join(format!("{id}.{}", ext_of(enc.mime))), &enc.bytes).await.map_err(io)?;
        tokio::fs::write(dir.join(format!("{id}.json")), serde_json::to_vec(&image).map_err(|e| ImageError::Io(e.to_string()))?).await.map_err(io)?;
        Ok(image)
    }

    /// The reference of a stored image (what a message that names only the id carries).
    pub async fn image_ref(&self, session_id: &str, image_id: &str) -> Result<ImageRef, ImageError> {
        let raw = tokio::fs::read(self.images_dir(session_id).join(format!("{}.json", clean(image_id)))).await.map_err(|_| ImageError::NotFound(image_id.to_string()))?;
        serde_json::from_slice(&raw).map_err(|e| ImageError::Io(e.to_string()))
    }

    /// The references of `ids`, in order; at most 4, each of this session.
    pub async fn image_refs(&self, session_id: &str, ids: &[String]) -> Result<Vec<ImageRef>, ImageError> {
        if ids.len() > MAX_IMAGES {
            return Err(ImageError::TooMany(ids.len()));
        }
        let mut out = Vec::new();
        for id in ids {
            out.push(self.image_ref(session_id, id).await?);
        }
        Ok(out)
    }

    /// The stored file and its type.
    pub async fn image_file(&self, session_id: &str, image_id: &str) -> Result<(Vec<u8>, String), ImageError> {
        let image = self.image_ref(session_id, image_id).await?;
        let path = self.images_dir(session_id).join(format!("{}.{}", clean(&image.id), ext_of(&image.mime)));
        let bytes = tokio::fs::read(path).await.map_err(|_| ImageError::NotFound(image_id.to_string()))?;
        Ok((bytes, image.mime))
    }

    pub(super) async fn remove_images(&self, session_id: &str) {
        let _ = tokio::fs::remove_dir_all(self.images_dir(session_id)).await;
    }
}

#[cfg(test)]
mod tests;
