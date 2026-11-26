//! Rounded corners for KAgent's frameless window on X11: the window gets a rounded
//! bounding shape (X SHAPE extension), which every window manager honours. A browser
//! window cannot be transparent, so CSS alone cannot round the real edge. The browser
//! clears the shape whenever the window is moved or resized, so a guard that is told of
//! every change of the shape (ShapeNotify) puts it back at once; once a second the
//! watcher also compares the shape with the wanted one.

use std::sync::Arc;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{Atom, AtomEnum, ChangeWindowAttributesAux, ClipOrdering, ConnectionExt as _, EventMask, Rectangle};
use x11rb::protocol::Event;

use crate::appwin::AppWindows;

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

/// What to do with the shape found on the window (pure, tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The window has the shape it should have.
    Keep,
    /// Give the window the rounded shape for its size: it has none (the browser cleared
    /// it) or one left over from another size.
    Round,
    /// Remove the shape (a maximized window fills the screen edge to edge).
    Clear,
}

/// `Rectangle` has no `PartialEq` in this build.
fn same(current: &[Rectangle], wanted: &[Rectangle]) -> bool {
    current.len() == wanted.len()
        && current.iter().zip(wanted).all(|(a, b)| a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height)
}

/// `radius` 0 means the window should have no shape.
pub fn action(current: &[Rectangle], w: u16, h: u16, radius: u16) -> Action {
    if radius == 0 {
        if current.is_empty() { Action::Keep } else { Action::Clear }
    } else if same(current, &rounded_rects(w, h, radius)) {
        Action::Keep
    } else {
        Action::Round
    }
}

fn id_of(window_hex: &str) -> Option<u32> {
    u32::from_str_radix(window_hex.trim_start_matches("0x"), 16).ok()
}

/// The window's size and its bounding shape (`None` when there is no such window).
fn state<C: Connection>(conn: &C, id: u32) -> Option<(u16, u16, Vec<Rectangle>)> {
    let g = conn.get_geometry(id).ok()?.reply().ok()?;
    let shape = conn.shape_get_rectangles(id, SK::BOUNDING).ok()?.reply().ok()?.rectangles;
    Some((g.width, g.height, shape))
}

/// Whether the window manager reports the window as maximized.
fn wm_maximized<C: Connection>(conn: &C, id: u32) -> Option<bool> {
    let (state, vert, horz) = (
        conn.intern_atom(false, b"_NET_WM_STATE").ok()?.reply().ok()?.atom,
        conn.intern_atom(false, b"_NET_WM_STATE_MAXIMIZED_VERT").ok()?.reply().ok()?.atom,
        conn.intern_atom(false, b"_NET_WM_STATE_MAXIMIZED_HORZ").ok()?.reply().ok()?.atom,
    );
    let reply = conn.get_property(false, id, state, AtomEnum::ATOM, 0, 64).ok()?.reply().ok()?;
    // No property at all (format 0) means no state is set.
    let set: Vec<Atom> = reply.value32().map(|atoms| atoms.collect()).unwrap_or_default();
    Some(set.contains(&vert) || set.contains(&horz))
}

/// The rounded shape for the size, or no shape when `radius` is 0.
fn put<C: Connection>(conn: &C, id: u32, w: u16, h: u16, radius: u16) {
    if radius == 0 {
        let _ = conn.shape_mask(SO::SET, SK::BOUNDING, id, 0, 0, x11rb::NONE);
    } else {
        let _ = conn.shape_rectangles(SO::SET, SK::BOUNDING, ClipOrdering::UNSORTED, id, 0, 0, &rounded_rects(w, h, radius));
    }
    let _ = conn.flush();
}

/// Bring the shape in line with the window's state. Returns whether the window is
/// maximized (`None` when there is no such window).
fn check(window_hex: &str, radius: u16) -> Option<bool> {
    let id = id_of(window_hex)?;
    let (conn, _) = x11rb::connect(None).ok()?;
    let (w, h, current) = state(&conn, id)?;
    let maximized = wm_maximized(&conn, id)?;
    let r = if maximized { 0 } else { radius };
    if action(&current, w, h, r) != Action::Keep {
        put(&conn, id, w, h, r);
    }
    Some(maximized)
}

/// Until the window is destroyed: whenever the bounding shape is no longer the wanted
/// one (the browser clears it on every move or resize), put it back. Ends with `None`
/// when the window is gone.
fn guard<C: Connection>(conn: &C, id: u32, radius: u16, is_maximized: impl Fn() -> bool) -> Option<()> {
    conn.shape_select_input(id, true).ok()?.check().ok()?;
    conn.change_window_attributes(id, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY)).ok()?.check().ok()?;
    loop {
        match conn.wait_for_event().ok()? {
            Event::DestroyNotify(e) if e.window == id => return None,
            Event::ShapeNotify(e) if e.shape_kind == SK::BOUNDING && e.affected_window == id => {}
            _ => continue,
        }
        let r = if is_maximized() { 0 } else { radius };
        let (w, h, current) = state(conn, id)?;
        if action(&current, w, h, r) != Action::Keep {
            put(conn, id, w, h, r);
        }
    }
}

/// The guard on a connection of its own (it blocks on the X server, so it has its own
/// thread).
fn follow(window_hex: &str, radius: u16, windows: &Arc<AppWindows>) -> Option<()> {
    let id = id_of(window_hex)?;
    let (conn, _) = x11rb::connect(None).ok()?;
    guard(&conn, id, radius, || windows.is_maximized())
}

/// Keep KAgent's window rounded. Once a second the shape is compared with the wanted one
/// (none while the window manager says the window is maximized, which is also what
/// `AppWindows` is told), and a guard thread is kept on the window: the guard puts the
/// shape back the moment the browser clears it.
pub fn watch(windows: Arc<AppWindows>, radius: u16) {
    if kagent_core::platform::wayland() {
        return; // Wayland: the compositor decides; nothing to do from here
    }
    tokio::spawn(async move {
        let mut follower: Option<(String, std::thread::JoinHandle<()>)> = None;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(id) = windows.window_id_cached().await else { continue };
            let id2 = id.clone();
            let maximized = tokio::task::spawn_blocking(move || check(&id2, radius)).await.ok().flatten();
            let Some(maximized) = maximized else { continue }; // no such window
            windows.set_maximized(maximized);
            let running = follower.as_ref().is_some_and(|(followed, handle)| *followed == id && !handle.is_finished());
            if running {
                continue;
            }
            let (id3, windows) = (id.clone(), windows.clone());
            follower = Some((id, std::thread::spawn(move || {
                let _ = follow(&id3, radius, &windows);
            })));
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

    #[test]
    fn a_shape_that_is_missing_or_left_over_is_rounded_again() {
        assert_eq!(action(&rounded_rects(100, 60, 10), 100, 60, 10), Action::Keep);
        // The browser cleared it, or the window changed size since it was applied.
        assert_eq!(action(&[], 100, 60, 10), Action::Round);
        assert_eq!(action(&rounded_rects(90, 60, 10), 100, 60, 10), Action::Round);
    }

    #[test]
    fn a_maximized_window_is_left_without_a_shape() {
        assert_eq!(action(&[], 100, 60, 0), Action::Keep);
        assert_eq!(action(&rounded_rects(100, 60, 10), 100, 60, 0), Action::Clear);
    }

    /// The guard against the real X server, on a window of this test (1x1, never shown):
    /// `KAGENT_X11=1 cargo test -p kagentd x11_ -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs an X server: see the comment"]
    fn x11_guard_puts_the_shape_back_and_ends_with_the_window() {
        assert_eq!(std::env::var("KAGENT_X11").as_deref(), Ok("1"), "set KAGENT_X11=1");
        let (mine, _) = x11rb::connect(None).unwrap();
        let (its, _) = x11rb::connect(None).unwrap();
        let id = mine.generate_id().unwrap();
        mine.create_window(x11rb::COPY_DEPTH_FROM_PARENT, id, mine.setup().roots[0].root, 0, 0, 1, 1, 0, x11rb::protocol::xproto::WindowClass::INPUT_OUTPUT, 0, &x11rb::protocol::xproto::CreateWindowAux::new()).unwrap().check().unwrap();
        assert_eq!(wm_maximized(&mine, id), Some(false), "a plain window is not maximized");
        let guard = std::thread::spawn(move || guard(&its, id, 14, || false));
        let mut back = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !back && std::time::Instant::now() < deadline {
            // What the browser does on every move or resize: clear the shape, from its own
            // connection (repeated until the guard has had its say).
            put(&mine, id, 1, 1, 14);
            mine.shape_mask(SO::SET, SK::BOUNDING, id, 0, 0, x11rb::NONE).unwrap().check().unwrap();
            let until = std::time::Instant::now() + Duration::from_millis(200);
            while std::time::Instant::now() < until {
                let (_, _, current) = state(&mine, id).unwrap();
                if action(&current, 1, 1, 14) == Action::Keep {
                    back = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(back, "the shape was not put back");
        mine.destroy_window(id).unwrap().check().unwrap();
        assert_eq!(guard.join().unwrap(), None, "the guard ends when the window is destroyed");
    }
}
