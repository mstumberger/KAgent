//! Tray icon (StatusNotifierItem: KDE natively, GNOME with the AppIndicator
//! extension). KAgent lives here: left click shows/hides its window, middle click mutes,
//! scroll changes the voice volume, the menu switches widget / full window / hidden and
//! controls voice. The icon is drawn here and shows the state at a glance: grey idle,
//! green pulsing listening, red mic muted, blue speaking, orange error, with a dot when
//! alerts are open. Always `Active`, so Plasma never tucks it into the overflow.
//! Menu callbacks are synchronous: they queue an `Action` for one task.

use std::sync::Arc;
use std::time::Duration;

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, TrayMethods};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::appwin::Mode;
use crate::AppState;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Toggle,
    Window(Mode),
    OpenAlerts,
    Listen,
    StopListening,
    ToggleMute,
    Volume(f64),
    VolumeStep(f64),
    StopAll,
    Permission(kagent_core::permission::PermissionMode),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Idle,
    Listening,
    Speaking,
    Muted,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub voice: String,
    pub muted: bool,
    pub speaking: bool,
    pub volume: f64,
    pub alerts: usize,
    pub mode: Mode,
    pub full: bool,
    pub permission: kagent_core::permission::PermissionMode,
    /// Animation phase for the listening pulse.
    pub pulse: bool,
}

impl Default for View {
    fn default() -> Self {
        Self { voice: String::new(), muted: false, speaking: false, volume: 0.6, alerts: 0, mode: Mode::Hidden, full: false, permission: Default::default(), pulse: false }
    }
}

pub const VOLUMES: [f64; 5] = [0.2, 0.4, 0.6, 0.8, 1.0];

impl View {
    pub fn listening(&self) -> bool {
        !matches!(self.voice.as_str(), "" | "idle" | "error")
    }
    pub fn look(&self) -> Look {
        if self.voice == "error" {
            Look::Error
        } else if !self.listening() {
            Look::Idle
        } else if self.muted {
            Look::Muted
        } else if self.speaking {
            Look::Speaking
        } else {
            Look::Listening
        }
    }
    pub fn tooltip(&self) -> String {
        let v = match self.look() {
            Look::Idle => "not listening (right click: Listen)".to_string(),
            Look::Listening => format!("listening ({}% volume)", (self.volume * 100.0).round()),
            Look::Speaking => "speaking".into(),
            Look::Muted => "mic muted".into(),
            Look::Error => "voice error".into(),
        };
        let mut t = format!("KAgent: {v}");
        if self.full {
            t.push_str(", full control");
        }
        if self.alerts > 0 {
            t.push_str(&format!(", {} open alerts", self.alerts));
        }
        t
    }
    /// Index of the volume radio closest to the current volume.
    pub fn volume_index(&self) -> usize {
        VOLUMES.iter().enumerate().min_by(|a, b| (a.1 - self.volume).abs().total_cmp(&(b.1 - self.volume).abs())).map(|(i, _)| i).unwrap_or(2)
    }
}

/// Draw the tray icon: a filled orb in the state colour with a soft glow, a white slash
/// when muted, a thin grey ring when idle, an orange dot when alerts are open.
/// ARGB32, network byte order.
pub fn draw_icon(look: Look, pulse: bool, alerts: bool, size: i32) -> ksni::Icon {
    let (r, g, b) = match look {
        Look::Idle => (0x8b, 0x94, 0x9e),
        Look::Listening => (0x3f, 0xb9, 0x50),
        Look::Speaking => (0x58, 0xa6, 0xff),
        Look::Muted => (0xf8, 0x51, 0x49),
        Look::Error => (0xd2, 0x99, 0x22),
    };
    let n = size as f32;
    let c = (n - 1.0) / 2.0;
    let core = n * if look == Look::Listening && pulse { 0.40 } else { 0.34 };
    let ring = n * 0.48;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let idle = look == Look::Idle;
            let mut a = if idle {
                // Idle: a ring with a small dot, quiet but visible on any panel.
                if (d <= ring && d >= ring - n * 0.10) || d <= n * 0.14 { 230.0 } else { 0.0 }
            } else if d <= core {
                255.0
            } else if d <= ring {
                120.0 * (1.0 - (d - core) / (ring - core))
            } else {
                0.0
            };
            let (mut pr, mut pg, mut pb) = (r, g, b);
            if !idle && d <= core && ((dx + n * 0.12).powi(2) + (dy + n * 0.12).powi(2)).sqrt() < n * 0.10 {
                (pr, pg, pb) = (255, 255, 255); // highlight
            }
            if look == Look::Muted && d <= ring && (dx - dy).abs() < n * 0.07 {
                (pr, pg, pb) = (255, 255, 255);
                a = 255.0;
            }
            if alerts && ((x as f32 - (n - n * 0.17)).powi(2) + (y as f32 - n * 0.17).powi(2)).sqrt() <= n * 0.16 {
                (pr, pg, pb) = (0xf0, 0x88, 0x3e);
                a = 255.0;
            }
            data.extend_from_slice(&[a.clamp(0.0, 255.0) as u8, pr, pg, pb]);
        }
    }
    ksni::Icon { width: size, height: size, data }
}

struct KTray {
    view: View,
    tx: mpsc::UnboundedSender<Action>,
}

impl KTray {
    fn send(&self, a: Action) {
        let _ = self.tx.send(a);
    }
}

impl ksni::Tray for KTray {
    fn id(&self) -> String {
        "kagent".into()
    }
    fn title(&self) -> String {
        "KAgent".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let alerts = self.view.alerts > 0;
        [16, 22, 32, 48].iter().map(|s| draw_icon(self.view.look(), self.view.pulse, alerts, *s)).collect()
    }
    fn status(&self) -> ksni::Status {
        // Passive items go to Plasma's hidden overflow: KAgent must stay visible.
        ksni::Status::Active
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.view.tooltip(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::Toggle);
    }
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::ToggleMute);
    }
    fn scroll(&mut self, delta: i32, _o: ksni::Orientation) {
        self.send(Action::VolumeStep(if delta > 0 { -0.05 } else { 0.05 }));
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let v = &self.view;
        let mode_idx = match v.mode {
            Mode::Compact => 0,
            Mode::Full => 1,
            Mode::Hidden => 2,
        };
        let mut items: Vec<MenuItem<Self>> = vec![
            RadioGroup {
                selected: mode_idx,
                select: Box::new(|t: &mut Self, i| t.send(Action::Window([Mode::Compact, Mode::Full, Mode::Hidden][i.min(2)]))),
                options: vec![
                    RadioItem { label: "Widget (small, always on top)".into(), ..Default::default() },
                    RadioItem { label: "Full window".into(), ..Default::default() },
                    RadioItem { label: "Only in tray".into(), ..Default::default() },
                ],
            }
            .into(),
            MenuItem::Separator,
        ];
        if v.listening() {
            items.push(StandardItem { label: "Stop listening".into(), icon_name: "media-playback-stop".into(), activate: Box::new(|t: &mut Self| t.send(Action::StopListening)), ..Default::default() }.into());
        } else {
            items.push(StandardItem { label: "Listen".into(), icon_name: "audio-input-microphone".into(), activate: Box::new(|t: &mut Self| t.send(Action::Listen)), ..Default::default() }.into());
        }
        items.push(CheckmarkItem { label: "Mute microphone".into(), checked: v.muted, enabled: v.listening(), activate: Box::new(|t: &mut Self| t.send(Action::ToggleMute)), ..Default::default() }.into());
        items.push(
            SubMenu {
                label: format!("Voice volume ({}%)", (v.volume * 100.0).round()),
                icon_name: "audio-volume-medium".into(),
                submenu: vec![RadioGroup {
                    selected: v.volume_index(),
                    select: Box::new(|t: &mut Self, i| t.send(Action::Volume(VOLUMES[i.min(VOLUMES.len() - 1)]))),
                    options: VOLUMES.iter().map(|x| RadioItem { label: format!("{}%", (x * 100.0) as u32), ..Default::default() }).collect(),
                }
                .into()],
                ..Default::default()
            }
            .into(),
        );
        items.push(MenuItem::Separator);
        {
            use kagent_core::permission::PermissionMode as P;
            const LEVELS: [(P, &str); 4] = [(P::ReadOnly, "Read-only"), (P::Ask, "Ask before changes"), (P::Trusted, "Trusted"), (P::Full, "Full control (only root asks)")];
            let cur = LEVELS.iter().position(|(p, _)| *p == v.permission).unwrap_or(1);
            items.push(
                SubMenu {
                    label: format!("Permission: {}", LEVELS[cur].1),
                    icon_name: "security-medium".into(),
                    submenu: vec![RadioGroup {
                        selected: cur,
                        select: Box::new(|t: &mut Self, i| t.send(Action::Permission(LEVELS[i.min(3)].0))),
                        options: LEVELS.iter().map(|(_, l)| RadioItem { label: (*l).into(), ..Default::default() }).collect(),
                    }
                    .into()],
                    ..Default::default()
                }
                .into(),
            );
        }
        items.push(StandardItem { label: if v.alerts > 0 { format!("Alerts ({})", v.alerts) } else { "Alerts".into() }, icon_name: "dialog-warning".into(), activate: Box::new(|t: &mut Self| t.send(Action::OpenAlerts)), ..Default::default() }.into());
        items.push(StandardItem { label: "Stop all tasks".into(), icon_name: "process-stop".into(), activate: Box::new(|t: &mut Self| t.send(Action::StopAll)), ..Default::default() }.into());
        items.push(MenuItem::Separator);
        items.push(StandardItem { label: "Quit KAgent".into(), icon_name: "application-exit".into(), activate: Box::new(|t: &mut Self| t.send(Action::Quit)), ..Default::default() }.into());
        items
    }
}

fn layout(mode: Mode) -> Value {
    json!({"mode": if mode == Mode::Compact { "compact" } else { "full" }})
}

/// Switch the app window and tell the page which layout to show.
pub async fn set_window(st: &AppState, mode: Mode, autostart_voice: bool) {
    if let Err(e) = st.windows.set_mode(mode, autostart_voice).await {
        tracing::warn!("window: {e}");
        return;
    }
    if mode != Mode::Hidden {
        st.ui.command("layout", layout(mode));
    }
}

async fn run_action(st: &Arc<AppState>, a: Action) {
    let voice = st.ui.voice();
    match a {
        Action::Toggle => match st.windows.toggle().await {
            Ok(m) if m != Mode::Hidden => {
                st.ui.command("layout", layout(m));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("tray: {e}"),
        },
        Action::Window(m) => set_window(st, m, false).await,
        Action::OpenAlerts => {
            set_window(st, Mode::Full, false).await;
            st.ui.command("show", json!({"view": "alerts"}));
        }
        Action::Listen => {
            if st.windows.running().await {
                st.ui.command("voice_start", json!({"target": "app"}));
            } else {
                // Not running: start as the widget and listen right away.
                set_window(st, Mode::Compact, true).await;
            }
        }
        Action::StopListening => {
            st.ui.command("voice_stop", json!({}));
        }
        Action::ToggleMute => {
            st.ui.command("mute_toggle", json!({}));
        }
        Action::Volume(v) => {
            st.ui.command("volume", json!({"value": v}));
        }
        Action::VolumeStep(d) => {
            let cur = voice.get("volume").and_then(Value::as_f64).unwrap_or(0.6);
            st.ui.command("volume", json!({"value": (cur + d).clamp(0.0, 1.0)}));
        }
        Action::StopAll => {
            let n = st.agent.stop_all().await;
            st.screen_watch.stop();
            tracing::info!("tray: stop all ({n} cancelled)");
            st.ui.command("voice_stop", json!({}));
        }
        Action::Permission(mode) => {
            let settings = {
                let mut s = st.settings.write().unwrap();
                s.permission_mode = mode;
                st.agent.permissions().set_mode(s.permission_mode);
                if let Err(e) = s.save(&crate::paths::Paths::resolve().settings()) {
                    tracing::warn!("tray: could not save settings: {e:#}");
                }
                tracing::info!("tray: permission mode -> {:?}", s.permission_mode);
                s.clone()
            };
            let _ = st.events.send(kagent_core::protocol::AgentEvent::SettingsChanged { settings: serde_json::to_value(&settings).unwrap_or(Value::Null) });
        }
        Action::Quit => {
            tracing::info!("tray: quit");
            st.windows.close();
            // Same path as a service stop: SIGTERM -> graceful shutdown + audit stop record.
            let _ = tokio::process::Command::new("kill").args(["-TERM", &std::process::id().to_string()]).status().await;
        }
    }
}

/// Start the tray; returns false when there is no tray host (e.g. GNOME without the
/// AppIndicator extension) or it is disabled (`KAGENT_NO_TRAY`, tests).
pub async fn start(st: Arc<AppState>) -> bool {
    if std::env::var_os("KAGENT_NO_TRAY").is_some() {
        return false;
    }
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tray = KTray { view: View::default(), tx };
    let handle = match tray.spawn().await {
        Ok(h) => h,
        Err(e) => {
            tracing::info!("no tray icon: {e} (GNOME needs the AppIndicator extension)");
            return false;
        }
    };
    let st2 = st.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(700));
        let mut pulse = false;
        loop {
            tokio::select! {
                a = rx.recv() => match a {
                    Some(a) => run_action(&st2, a).await,
                    None => break,
                },
                _ = tick.tick() => pulse = !pulse,
            }
            let voice = st2.ui.voice();
            let speaking = st2.levels.snapshot(u64::MAX).get("state").and_then(Value::as_str) == Some("speaking");
            let mut view = View {
                voice: voice.get("status").and_then(Value::as_str).unwrap_or("").to_string(),
                muted: voice.get("muted").and_then(Value::as_bool).unwrap_or(false),
                speaking,
                volume: voice.get("volume").and_then(Value::as_f64).unwrap_or(0.6),
                alerts: st2.alerts.open_count(),
                mode: *st2.windows.mode.lock().unwrap(),
                full: st2.settings.read().unwrap().permission_mode == kagent_core::permission::PermissionMode::Full,
                permission: st2.settings.read().unwrap().permission_mode,
                pulse: false,
            };
            // Only the listening look animates; other states repaint only on change.
            view.pulse = pulse && view.look() == Look::Listening;
            handle.update(|t| if t.view != view { t.view = view }).await;
        }
    });
    tracing::info!("tray icon ready");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(status: &str, muted: bool, speaking: bool) -> View {
        View { voice: status.into(), muted, speaking, volume: 0.4, alerts: 2, ..Default::default() }
    }

    #[test]
    fn looks_and_tooltips() {
        assert_eq!(v("", false, false).look(), Look::Idle);
        assert_eq!(v("live", false, false).look(), Look::Listening);
        assert_eq!(v("live", false, true).look(), Look::Speaking);
        assert_eq!(v("live", true, true).look(), Look::Muted);
        assert_eq!(v("error", false, false).look(), Look::Error);
        assert_eq!(v("live", false, false).tooltip(), "KAgent: listening (40% volume), 2 open alerts");
        assert_eq!(v("live", false, false).volume_index(), 1);
    }

    #[test]
    fn icon_pixels() {
        let i = draw_icon(Look::Listening, false, false, 22);
        assert_eq!(i.data.len(), 22 * 22 * 4);
        let center = ((11 * 22 + 11) * 4) as usize;
        assert_eq!(i.data[center], 255, "opaque core");
        assert_eq!(&i.data[center + 1..center + 4], &[0x3f, 0xb9, 0x50], "green core");
        assert_eq!(i.data[0], 0, "corner transparent");
        let muted = draw_icon(Look::Muted, false, false, 22);
        assert_eq!(&muted.data[center + 1..center + 4], &[255, 255, 255], "white slash through the centre");
        let alert = draw_icon(Look::Idle, false, true, 22);
        let corner = ((3 * 22 + 18) * 4) as usize;
        assert_eq!(&alert.data[corner + 1..corner + 4], &[0xf0, 0x88, 0x3e], "alert dot top-right");
    }
}
