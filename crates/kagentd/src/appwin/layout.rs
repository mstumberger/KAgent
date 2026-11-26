//! The pure parts of the app window (tested without a desktop): parsers of tool
//! output, drag and resize geometry, the browser's argv, the KWin scripts, default
//! placements and the microphone grant in the app profile.

use std::path::Path;

use super::{Geom, Mode, CLASS, COMPACT, FULL};

/// `xdotool getmouselocation --shell` -> (x, y).
pub fn parse_mouse(out: &str) -> Option<(i32, i32)> {
    let v = |k: &str| out.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.trim().parse().ok());
    Some((v("X=")?, v("Y=")?))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragKind {
    Move,
    /// Resize from an edge or corner: which sides move.
    Resize { left: bool, right: bool, top: bool, bottom: bool },
}

impl DragKind {
    /// `"n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw"` (bottom-right when unknown).
    pub fn resize_from(edge: &str) -> Self {
        let e = if edge.is_empty() { "se" } else { edge };
        DragKind::Resize { left: e.contains('w'), right: e.contains('e'), top: e.contains('n'), bottom: e.contains('s') }
    }
}

/// New geometry for a resize drag by (dx, dy) from `g0`, keeping the opposite sides
/// fixed and the window at least `min` in size (pure, tested).
pub fn resized(g0: Geom, dx: i32, dy: i32, kind: DragKind, min: (u32, u32)) -> Geom {
    let DragKind::Resize { left, right, top, bottom } = kind else { return g0 };
    let (mut x, mut y, mut w, mut h) = (g0.x, g0.y, g0.w as i32, g0.h as i32);
    if right {
        w = g0.w as i32 + dx;
    }
    if left {
        w = g0.w as i32 - dx;
    }
    if bottom {
        h = g0.h as i32 + dy;
    }
    if top {
        h = g0.h as i32 - dy;
    }
    w = w.clamp(min.0 as i32, 7000);
    h = h.clamp(min.1 as i32, 5000);
    if left {
        x = g0.x + g0.w as i32 - w;
    }
    if top {
        y = g0.y + g0.h as i32 - h;
    }
    Geom { x, y, w: w as u32, h: h as u32 }
}

/// Browser argv for the app window (pure, tested).
pub fn argv(bin: &str, profile: &Path, url: &str, compact: bool) -> Vec<String> {
    let (w, h) = if compact { COMPACT } else { FULL };
    vec![
        bin.to_string(),
        format!("--app={url}"),
        format!("--class={CLASS}"),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        // Voice must start from the tray or a hotkey without a click in the page.
        "--autoplay-policy=no-user-gesture-required".into(),
        format!("--window-size={w},{h}"),
    ]
}

/// Where the window goes when its mode changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    Keep,
    At(Geom),
    /// Default spot on the active screen (KWin knows the real monitor layout).
    Default,
}

/// KWin script that puts the app window into a mode (pure, tested).
pub fn kwin_mode_script(mode: Mode, place: Place) -> String {
    let c = serde_json::to_string(CLASS).unwrap_or_default();
    // Frameless in every mode: KAgent draws its own title bar and controls.
    let body = match mode {
        Mode::Compact => "w.noBorder = true; w.minimized = false; w.keepAbove = true; w.onAllDesktops = true; w.skipTaskbar = true; w.skipPager = true; workspace.activeWindow = w;",
        Mode::Full => "w.noBorder = true; w.minimized = false; w.keepAbove = false; w.onAllDesktops = false; w.skipTaskbar = false; w.skipPager = false; workspace.activeWindow = w;",
        Mode::Hidden => "w.skipTaskbar = true; w.minimized = true;",
    };
    let g = match (place, mode) {
        (Place::At(g), _) => format!(" w.frameGeometry = {{x: {}, y: {}, width: {}, height: {}}};", g.x, g.y, g.w, g.h),
        (Place::Default, Mode::Compact) => format!(
            " var a = workspace.clientArea(KWin.PlacementArea, workspace.activeScreen, workspace.currentDesktop); w.frameGeometry = {{x: a.x + a.width - {w} - 24, y: a.y + a.height - {h} - 24, width: {w}, height: {h}}};",
            w = COMPACT.0,
            h = COMPACT.1
        ),
        (Place::Default, Mode::Full) => format!(
            " var a = workspace.clientArea(KWin.PlacementArea, workspace.activeScreen, workspace.currentDesktop); var fw = Math.min({w}, a.width - 80), fh = Math.min({h}, a.height - 80); w.frameGeometry = {{x: a.x + (a.width - fw) / 2, y: a.y + (a.height - fh) / 2, width: fw, height: fh}};",
            w = FULL.0,
            h = FULL.1
        ),
        _ => String::new(),
    };
    format!("var list = workspace.windowList();\nfor (var i = 0; i < list.length; i++) {{\n  var w = list[i];\n  if (String(w.resourceClass) !== {c}) continue;\n  {body}{g}\n}}\n")
}

/// Default compact place: bottom-right of a screen of the given size.
pub fn default_compact(screen_w: u32, screen_h: u32) -> Geom {
    Geom { x: screen_w as i32 - COMPACT.0 as i32 - 24, y: screen_h as i32 - COMPACT.1 as i32 - 64, w: COMPACT.0, h: COMPACT.1 }
}

pub fn centered_full(screen_w: u32, screen_h: u32) -> Geom {
    let (w, h) = (FULL.0.min(screen_w.saturating_sub(80)), FULL.1.min(screen_h.saturating_sub(80)));
    Geom { x: (screen_w as i32 - w as i32) / 2, y: (screen_h as i32 - h as i32) / 2, w, h }
}

/// `xwininfo -id <w>` -> geometry.
pub fn parse_xwininfo(out: &str) -> Option<Geom> {
    let num = |k: &str| out.lines().find_map(|l| l.trim().strip_prefix(k)).and_then(|v| v.trim().parse::<i64>().ok());
    Some(Geom { x: num("Absolute upper-left X:")? as i32, y: num("Absolute upper-left Y:")? as i32, w: num("Width:")? as u32, h: num("Height:")? as u32 })
}

/// Allow the microphone for KAgent's own origin in the app profile, so the window never
/// prompts (Chromium content setting, written while the browser for this profile is not
/// running; nothing else is granted). Returns true when the file was changed.
pub fn grant_mic(profile: &Path, origin: &str) -> std::io::Result<bool> {
    let dir = profile.join("Default");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("Preferences");
    let mut prefs: serde_json::Value = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| serde_json::json!({}));
    let key = format!("{origin},*");
    let mic = &mut prefs["profile"]["content_settings"]["exceptions"]["media_stream_mic"];
    if mic.get(&key).and_then(|v| v.get("setting")).and_then(serde_json::Value::as_i64) == Some(1) {
        return Ok(false);
    }
    if !mic.is_object() {
        *mic = serde_json::json!({});
    }
    mic[&key] = serde_json::json!({"setting": 1, "last_modified": "13440000000000000"});
    std::fs::write(&path, serde_json::to_vec(&prefs)?)?;
    Ok(true)
}
