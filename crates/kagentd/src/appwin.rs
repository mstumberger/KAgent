//! KAgent's own window (no Tauri needed): ONE chrome-less app window of a
//! Chromium-family browser (own profile, class `kagent-app`) that switches between
//! three states without reloading, so a voice session keeps running:
//! - `compact`: the small orb widget, above all windows, on every desktop, out of the
//!   taskbar, where the user last put it (default: bottom-right);
//! - `full`: a normal 1200x820 window with chat, transcript and settings;
//! - `hidden`: minimized to the tray (voice keeps listening).
//! KDE: KWin scripts; other X11 desktops: wmctrl/xdotool.

mod layout;
pub(crate) mod scope;

pub use layout::*;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use kagent_core::platform::{desktop, Desktop};
use kagent_core::tool::ToolContext;
use kagent_core::tools::exec::exec;
use kagent_core::tools::xdg::which;
use serde::{Deserialize, Serialize};

const BINARIES: [&str; 7] = ["brave-browser", "brave", "google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge"];
pub const CLASS: &str = "kagent-app";
pub const COMPACT: (u32, u32) = (340, 460);
pub const FULL: (u32, u32) = (1200, 820);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Compact,
    Full,
    Hidden,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "compact" | "widget" => Some(Mode::Compact),
            "full" | "expand" => Some(Mode::Full),
            "hidden" | "hide" | "tray" => Some(Mode::Hidden),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Geom {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    compact: Option<Geom>,
}

pub struct AppWindows {
    data: PathBuf,
    token: String,
    port: u16,
    child: Mutex<Option<tokio::process::Child>>,
    pub mode: Mutex<Mode>,
    /// Mode before hiding, restored by "show".
    last_visible: Mutex<Mode>,
    /// X11 id of the window, cached for fast drag moves.
    cached_id: Mutex<Option<String>>,
    maximized: Mutex<bool>,
    /// Running drag/resize: generation counter, bumped to stop the follower task.
    drag_gen: std::sync::atomic::AtomicU64,
}

fn ctx() -> ToolContext {
    ToolContext::standalone(Duration::from_secs(5), 64_000)
}

impl AppWindows {
    pub fn new(data: &Path, token: &str, port: u16) -> Self {
        Self {
            data: data.to_path_buf(),
            token: token.to_string(),
            port,
            child: Mutex::new(None),
            mode: Mutex::new(Mode::Hidden),
            last_visible: Mutex::new(Mode::Compact),
            cached_id: Mutex::new(None),
            maximized: Mutex::new(false),
            drag_gen: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn saved_path(&self) -> PathBuf {
        self.data.join("window.json")
    }
    fn saved(&self) -> Saved {
        std::fs::read(self.saved_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn browser() -> Option<String> {
        BINARIES.iter().find_map(|b| which(b)).map(|p| p.display().to_string())
    }

    /// X11 id of the app window (also one we did not start, e.g. after a daemon restart).
    pub async fn window_id(&self) -> Option<String> {
        let id = self.find_window().await;
        *self.cached_id.lock().unwrap() = id.clone();
        id
    }

    async fn fast_id(&self) -> Option<String> {
        let cached = self.cached_id.lock().unwrap().clone();
        match cached {
            Some(id) => Some(id),
            None => self.window_id().await,
        }
    }

    /// Window id for frequent callers (cached; re-scanned when unknown).
    pub async fn window_id_cached(&self) -> Option<String> {
        self.fast_id().await
    }

    pub fn is_maximized(&self) -> bool {
        *self.maximized.lock().unwrap()
    }

    /// Set the flag from what the window manager says: the desktop can maximize or
    /// restore the window too, and a maximized window outlives a daemon restart.
    pub fn set_maximized(&self, value: bool) {
        *self.maximized.lock().unwrap() = value;
    }

    /// Move the window's top-left corner to screen coordinates (drag from the page).
    pub async fn move_to(&self, x: i32, y: i32) {
        let Some(id) = self.fast_id().await else { return };
        let r = exec(&ctx(), &["xdotool".to_string(), "windowmove".into(), id, x.to_string(), y.to_string()]).run().await;
        if !r.is_ok_and(|r| r.success()) {
            *self.cached_id.lock().unwrap() = None; // window was re-created
        }
    }

    /// Start following the real mouse pointer (X11 coordinates) and move or resize the
    /// window by its offset from the start, until `drag_stop` (pointer up in the page)
    /// or 20 s. Browser screen coordinates are not used: on multi-monitor layouts they
    /// do not match X11's.
    pub async fn drag_start(self: &std::sync::Arc<Self>, kind: DragKind) {
        use std::sync::atomic::Ordering;
        let generation = self.drag_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(id) = self.fast_id().await else { return };
        let c = ctx();
        let Some(m0) = exec(&c, &["xdotool", "getmouselocation", "--shell"]).run().await.ok().and_then(|r| parse_mouse(&r.stdout)) else { return };
        let Some(g0) = exec(&c, &["xwininfo", "-id", &id]).run().await.ok().and_then(|r| parse_xwininfo(&r.stdout)) else { return };
        let me = self.clone();
        tokio::spawn(async move {
            let c = ctx();
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let mut last = m0;
            while me.drag_gen.load(Ordering::SeqCst) == generation && std::time::Instant::now() < deadline {
                if let Some(m) = exec(&c, &["xdotool", "getmouselocation", "--shell"]).run().await.ok().and_then(|r| parse_mouse(&r.stdout)) {
                    if m != last {
                        last = m;
                        let (dx, dy) = (m.0 - m0.0, m.1 - m0.1);
                        let argv: Vec<String> = match kind {
                            DragKind::Move => vec!["xdotool".into(), "windowmove".into(), id.clone(), (g0.x + dx).to_string(), (g0.y + dy).to_string()],
                            DragKind::Resize { .. } => {
                                // The widget may be small; the full window keeps room for its header.
                                let min = if *me.mode.lock().unwrap() == Mode::Compact { (240, 280) } else { (560, 400) };
                                let g = resized(g0, dx, dy, kind, min);
                                let mut a: Vec<String> = vec!["xdotool".into(), "windowsize".into(), id.clone(), g.w.to_string(), g.h.to_string()];
                                if (g.x, g.y) != (g0.x, g0.y) {
                                    a.extend(["windowmove".into(), id.clone(), g.x.to_string(), g.y.to_string()]);
                                }
                                a
                            }
                        };
                        let _ = exec(&c, &argv).run().await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(14)).await;
            }
        });
    }

    pub fn drag_stop(&self) {
        self.drag_gen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Resize from the page's corner grip (full window).
    pub async fn resize(&self, w: u32, h: u32) {
        let Some(id) = self.fast_id().await else { return };
        let _ = exec(&ctx(), &["xdotool".to_string(), "windowsize".into(), id, w.clamp(320, 6000).to_string(), h.clamp(240, 4000).to_string()]).run().await;
    }

    /// Maximize / restore the full window.
    pub async fn toggle_maximize(&self) {
        let max = {
            let mut m = self.maximized.lock().unwrap();
            *m = !*m;
            *m
        };
        if desktop() == Desktop::Kde {
            let c = serde_json::to_string(CLASS).unwrap_or_default();
            let script = format!("var list = workspace.windowList();\nfor (var i = 0; i < list.length; i++) {{ var w = list[i]; if (String(w.resourceClass) === {c}) w.setMaximize({max}, {max}); }}\n");
            let _ = kagent_core::tools::apps::run_kwin_script(&ctx(), &script).await;
        } else if let Some(id) = self.fast_id().await {
            let _ = exec(&ctx(), &["wmctrl", "-i", "-r", &id, "-b", if max { "add,maximized_vert,maximized_horz" } else { "remove,maximized_vert,maximized_horz" }]).run().await;
        }
    }

    async fn find_window(&self) -> Option<String> {
        let c = ctx();
        let list = exec(&c, &["xprop", "-root", "_NET_CLIENT_LIST"]).run().await.ok()?;
        for id in kagent_core::tools::apps::parse_client_list(&list.stdout) {
            if let Ok(p) = exec(&c, &["xprop", "-id", &id, "WM_CLASS"]).run().await {
                if p.stdout.contains(&format!("\"{CLASS}\"")) {
                    return Some(id);
                }
            }
        }
        None
    }

    pub async fn running(&self) -> bool {
        let ours = {
            let mut ch = self.child.lock().unwrap();
            if let Some(c) = ch.as_mut() {
                if !matches!(c.try_wait(), Ok(None)) {
                    *ch = None;
                }
            }
            ch.is_some()
        };
        ours || self.window_id().await.is_some()
    }

    async fn screen_size(&self) -> (u32, u32) {
        // Size of the X screen; good enough for a default place the user can drag elsewhere.
        let r = exec(&ctx(), &["xdpyinfo"]).run().await.map(|r| r.stdout).unwrap_or_default();
        r.lines()
            .find_map(|l| l.trim().strip_prefix("dimensions:"))
            .and_then(|d| d.split_whitespace().next())
            .and_then(|d| d.split_once('x'))
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .unwrap_or((1920, 1080))
    }

    /// Start the window. `query` e.g. `compact&autostart`.
    async fn launch(&self, query: &str, hash: &str) -> Result<(), String> {
        let Some(bin) = Self::browser() else {
            return Err("no Chromium-family browser (Brave, Chrome, Chromium, Edge) to host KAgent's window".into());
        };
        let profile = self.data.join("app-profile");
        let _ = std::fs::create_dir_all(&profile);
        if let Err(e) = grant_mic(&profile, &format!("http://127.0.0.1:{}", self.port)) {
            tracing::warn!("could not pre-allow the microphone: {e}");
        }
        // `app` marks KAgent's own window: tray voice commands target it even when hidden.
        let mut url = format!("http://127.0.0.1:{}/?app", self.port);
        if !query.is_empty() {
            url.push('&');
            url.push_str(query);
        }
        url.push_str(&format!("#token={}", self.token));
        if !hash.is_empty() {
            url.push('&');
            url.push_str(hash);
        }
        let a = argv(&bin, &profile, &url, query.contains("compact"));
        let a = scope::in_own_scope(a, scope::under_systemd(), which("systemd-run").as_deref());
        let child = tokio::process::Command::new(&a[0])
            .args(&a[1..])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("{bin}: {e}"))?;
        *self.child.lock().unwrap() = Some(child);
        // Wait for the window to map (up to ~6 s).
        for _ in 0..24 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.window_id().await.is_some() {
                break;
            }
        }
        Ok(())
    }

    /// Put the window into a mode, starting it when needed. The caller also tells the
    /// page to switch its layout (`uiCommand layout`).
    pub async fn set_mode(&self, mode: Mode, autostart_voice: bool) -> Result<(), String> {
        if !self.running().await {
            if mode == Mode::Hidden {
                return Ok(());
            }
            let q = match (mode, autostart_voice) {
                (Mode::Compact, true) => "compact&autostart",
                (Mode::Compact, false) => "compact",
                (_, true) => "autostart",
                _ => "",
            };
            self.launch(q, "").await?;
        }
        let previous = *self.mode.lock().unwrap();
        // Leaving compact: remember where the user put the widget.
        if previous == Mode::Compact && mode != Mode::Compact {
            if let Some(id) = self.window_id().await {
                if let Some(g) = exec(&ctx(), &["xwininfo", "-id", &id]).run().await.ok().and_then(|r| parse_xwininfo(&r.stdout)) {
                    if g.w >= 280 && g.h >= 320 && g.w < 700 {
                        let _ = std::fs::write(self.saved_path(), serde_json::to_vec(&Saved { compact: Some(g) }).unwrap_or_default());
                    }
                }
            }
        }
        let place = match mode {
            // A saved spot is used only when it is a sane widget size (a bad drag once
            // stored 390x124 at the screen top).
            Mode::Compact if previous != Mode::Compact => self.saved().compact.filter(|g| g.w >= 280 && g.h >= 320 && g.w < 700 && g.y >= 0).map(Place::At).unwrap_or(Place::Default),
            Mode::Full if previous != Mode::Full => Place::Default,
            _ => Place::Keep,
        };
        self.apply(mode, place).await;
        *self.mode.lock().unwrap() = mode;
        if mode != Mode::Hidden {
            *self.last_visible.lock().unwrap() = mode;
        }
        Ok(())
    }

    /// Daemon restart with the window still open: learn its mode from its size instead of
    /// changing it. Returns false when there is no window to adopt.
    pub async fn adopt_existing(&self) -> bool {
        let Some(id) = self.window_id().await else { return false };
        let c = ctx();
        let g = exec(&c, &["xwininfo", "-id", &id]).run().await.ok().and_then(|r| parse_xwininfo(&r.stdout));
        let hidden = exec(&c, &["xprop", "-id", &id, "_NET_WM_STATE"]).run().await.is_ok_and(|r| r.stdout.contains("_NET_WM_STATE_HIDDEN"));
        let visible = if g.is_some_and(|g| g.w < 700) { Mode::Compact } else { Mode::Full };
        *self.last_visible.lock().unwrap() = visible;
        *self.mode.lock().unwrap() = if hidden { Mode::Hidden } else { visible };
        true
    }

    /// Show again in the last visible mode (tray left click on a hidden window).
    pub async fn show(&self) -> Result<Mode, String> {
        let m = *self.last_visible.lock().unwrap();
        self.set_mode(m, false).await.map(|_| m)
    }

    /// Tray left click: hide when visible, show when hidden or not running.
    pub async fn toggle(&self) -> Result<Mode, String> {
        let visible = *self.mode.lock().unwrap() != Mode::Hidden && self.running().await;
        if visible {
            self.set_mode(Mode::Hidden, false).await.map(|_| Mode::Hidden)
        } else {
            self.show().await
        }
    }

    async fn apply(&self, mode: Mode, place: Place) {
        let c = ctx();
        if desktop() == Desktop::Kde {
            let _ = kagent_core::tools::apps::run_kwin_script(&c, &kwin_mode_script(mode, place)).await;
            return;
        }
        let (sw, sh) = self.screen_size().await;
        let geom = match (place, mode) {
            (Place::At(g), _) => Some(g),
            (Place::Default, Mode::Compact) => Some(default_compact(sw, sh)),
            (Place::Default, Mode::Full) => Some(centered_full(sw, sh)),
            _ => None,
        };
        let Some(id) = self.window_id().await else { return };
        // Other window managers: the Motif hint is the common way to drop decorations.
        let _ = exec(&c, &["xprop", "-id", &id, "-f", "_MOTIF_WM_HINTS", "32c", "-set", "_MOTIF_WM_HINTS", "2, 0, 0, 0, 0"]).run().await;
        match mode {
            Mode::Hidden => {
                let _ = exec(&c, &["xdotool", "windowminimize", &id]).run().await;
            }
            m => {
                let state = if m == Mode::Compact { "add,above,sticky" } else { "remove,above,sticky" };
                let _ = exec(&c, &["wmctrl", "-i", "-r", &id, "-b", state]).run().await;
                if let Some(g) = geom {
                    let _ = exec(&c, &["wmctrl".to_string(), "-i".into(), "-r".into(), id.clone(), "-e".into(), format!("0,{},{},{},{}", g.x, g.y, g.w, g.h)]).run().await;
                }
                let _ = exec(&c, &["wmctrl", "-i", "-a", &id]).run().await;
            }
        }
    }

    pub fn close(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.start_kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_scripts_and_geometry() {
        let a = argv("/usr/bin/brave-browser", Path::new("/d/app-profile"), "http://127.0.0.1:7878/?compact#token=t", true);
        assert!(a.contains(&"--class=kagent-app".to_string()) && a.contains(&"--window-size=340,460".to_string()));
        let s = kwin_mode_script(Mode::Compact, Place::At(Geom { x: 10, y: 20, w: 340, h: 460 }));
        assert!(kwin_mode_script(Mode::Compact, Place::Default).contains("workspace.clientArea(KWin.PlacementArea"));
        assert!(s.contains(r#"String(w.resourceClass) !== "kagent-app""#) && s.contains("w.keepAbove = true") && s.contains("width: 340, height: 460"));
        assert!(kwin_mode_script(Mode::Full, Place::Keep).contains("w.keepAbove = false"));
        assert!(kwin_mode_script(Mode::Full, Place::Keep).contains("w.noBorder = true"), "frameless");
        assert!(kwin_mode_script(Mode::Hidden, Place::Keep).contains("w.minimized = true"));
        assert_eq!(default_compact(2560, 1440), Geom { x: 2196, y: 916, w: 340, h: 460 });
        assert_eq!(centered_full(2560, 1440), Geom { x: 680, y: 310, w: 1200, h: 820 });
        let g = parse_xwininfo("  Absolute upper-left X:  1556\n  Absolute upper-left Y:  885\n  Width: 340\n  Height: 432\n").unwrap();
        assert_eq!(g, Geom { x: 1556, y: 885, w: 340, h: 432 });
        assert_eq!(Mode::parse("tray"), Some(Mode::Hidden));
        let g = Geom { x: 100, y: 100, w: 400, h: 300 };
        assert_eq!(resized(g, 50, 20, DragKind::resize_from("se"), (200, 200)), Geom { x: 100, y: 100, w: 450, h: 320 });
        assert_eq!(resized(g, 50, 0, DragKind::resize_from("w"), (200, 200)), Geom { x: 150, y: 100, w: 350, h: 300 }, "left edge: right side stays");
        assert_eq!(resized(g, 0, -40, DragKind::resize_from("n"), (200, 200)), Geom { x: 100, y: 60, w: 400, h: 340 });
        assert_eq!(resized(g, 390, 0, DragKind::resize_from("w"), (200, 200)), Geom { x: 300, y: 100, w: 200, h: 300 }, "clamped at the minimum");
        assert_eq!(parse_mouse("X=4050\nY=1162\nSCREEN=0\nWINDOW=123\n"), Some((4050, 1162)));
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Default")).unwrap();
        std::fs::write(d.path().join("Default/Preferences"), r#"{"browser":{"x":1},"profile":{"content_settings":{"exceptions":{"media_stream_mic":{}}}}}"#).unwrap();
        assert!(grant_mic(d.path(), "http://127.0.0.1:7878").unwrap());
        assert!(!grant_mic(d.path(), "http://127.0.0.1:7878").unwrap(), "idempotent");
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(d.path().join("Default/Preferences")).unwrap()).unwrap();
        assert_eq!(v["profile"]["content_settings"]["exceptions"]["media_stream_mic"]["http://127.0.0.1:7878,*"]["setting"], 1);
        assert_eq!(v["browser"]["x"], 1, "other preferences kept");
    }
}
