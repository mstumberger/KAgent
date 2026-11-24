//! Rounded corners for KAgent's frameless window on X11: the window gets a rounded
//! bounding shape (X SHAPE extension), which every window manager honours. A browser
//! window cannot be transparent, so CSS alone cannot round the real edge. Re-applied
//! whenever the size changes (a watcher compares the size every second).

use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{ClipOrdering, ConnectionExt as _, Rectangle};

/// Rows of a rounded rectangle as rectangles (pure, tested): the corner rows are
/// inset by the circle, the middle is one block.
pub fn rounded_rects(w: u16, h: u16, r: u16) -> Vec<Rectangle> {
    let r = r.min(w / 2).min(h / 2);
    let mut out = Vec::with_capacity(r as usize * 2 + 1);
    for y in 0..r {
        // Distance from the corner circle's centre to this row's middle.
        let dy = r as f32 - y as f32 - 0.5;
        let inset = r as f32 - (r as f32 * r as f32 - dy * dy).max(0.0).sqrt();
        let inset = inset.round() as u16;
        let width = w.saturating_sub(inset * 2);
        out.push(Rectangle { x: inset as i16, y: y as i16, width, height: 1 });
        out.push(Rectangle { x: inset as i16, y: (h - 1 - y) as i16, width, height: 1 });
    }
    out.push(Rectangle { x: 0, y: r as i16, width: w, height: h.saturating_sub(r * 2) });
    out
}

/// Apply a rounded shape to the window (`0x...` id). Returns the size it was applied for.
pub fn apply(window_hex: &str, radius: u16) -> Option<(u16, u16)> {
    let id = u32::from_str_radix(window_hex.trim_start_matches("0x"), 16).ok()?;
    let (conn, _) = x11rb::connect(None).ok()?;
    let geo = conn.get_geometry(id).ok()?.reply().ok()?;
    let rects = rounded_rects(geo.width, geo.height, radius);
    conn.shape_rectangles(SO::SET, SK::BOUNDING, ClipOrdering::UNSORTED, id, 0, 0, &rects).ok()?;
    conn.flush().ok()?;
    Some((geo.width, geo.height))
}

/// Remove the shape (a maximized window should fill the screen edge to edge).
pub fn clear(window_hex: &str) {
    let Some(id) = u32::from_str_radix(window_hex.trim_start_matches("0x"), 16).ok() else { return };
    if let Ok((conn, _)) = x11rb::connect(None) {
        let _ = conn.shape_mask(SO::SET, SK::BOUNDING, id, 0, 0, x11rb::NONE);
        let _ = conn.flush();
    }
}

/// Keep KAgent's window rounded: once a second, when the window, its size or the
/// maximized state changed, re-apply the shape (cleared while maximized).
pub fn watch(windows: std::sync::Arc<crate::appwin::AppWindows>, radius: u16) {
    if kagent_core::platform::wayland() {
        return; // Wayland: the compositor decides; nothing to do from here
    }
    tokio::spawn(async move {
        let mut last: Option<(String, u16, u16, u16)> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(id) = windows.window_id_cached().await else { continue };
            let r = if windows.is_maximized() { 0 } else { radius };
            let id2 = id.clone();
            let size = tokio::task::spawn_blocking(move || {
                let n = u32::from_str_radix(id2.trim_start_matches("0x"), 16).ok()?;
                let (conn, _) = x11rb::connect(None).ok()?;
                let g = conn.get_geometry(n).ok()?.reply().ok()?;
                Some((g.width, g.height))
            })
            .await
            .ok()
            .flatten();
            let Some((w, h)) = size else { continue };
            let key = Some((id.clone(), w, h, r));
            if key == last {
                continue;
            }
            last = key;
            let _ = tokio::task::spawn_blocking(move || if r == 0 { clear(&id) } else { let _ = apply(&id, r); }).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_shape_rows() {
        let rects = rounded_rects(100, 60, 10);
        // 10 top + 10 bottom rows + the middle block.
        assert_eq!(rects.len(), 21);
        let top = rects[0];
        assert!(top.x >= 5 && top.width == 100 - 2 * top.x as u16, "first row is inset");
        let last_corner_row = rects[18];
        assert!(last_corner_row.x <= 1, "near the end of the corner the inset is ~0");
        let mid = rects[20];
        assert_eq!((mid.x, mid.y, mid.width, mid.height), (0, 10, 100, 40));
        assert_eq!(rounded_rects(10, 10, 30).len(), 11, "radius clamped to half the size");
    }
}
