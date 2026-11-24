//! Desktop backends: clipboard, color scheme, brightness, windows, active window,
//! synthetic input. KDE (Klipper, KWin, Plasma), GNOME (gsettings, Window Calls
//! extension on Wayland), and generic X11/Wayland tools (xclip, wl-clipboard, wmctrl,
//! xdotool, ydotool). Parsers are pure and unit-tested.

use std::process::Stdio;

use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::{desktop, wayland, Desktop};
use crate::tool::ToolContext;
use crate::tools::exec::exec;
use crate::tools::xdg::which;

// ---------------------------------------------------------------- clipboard

pub async fn clipboard_get(ctx: &ToolContext) -> Option<String> {
    if desktop() == Desktop::Kde {
        if let Ok(r) = exec(ctx, &["qdbus6", "org.kde.klipper", "/klipper", "org.kde.klipper.klipper.getClipboardContents"]).run().await {
            if r.success() {
                return Some(r.stdout.trim_end_matches('\n').to_string());
            }
        }
    }
    let argv: &[&str] = if wayland() && which("wl-paste").is_some() {
        &["wl-paste", "--no-newline"]
    } else if which("xclip").is_some() {
        &["xclip", "-o", "-selection", "clipboard"]
    } else if which("xsel").is_some() {
        &["xsel", "--clipboard", "--output"]
    } else {
        return None;
    };
    let r = exec(ctx, argv).run().await.ok()?;
    r.success().then_some(r.stdout)
}

/// Clipboard owners (xclip, wl-copy) fork a server that keeps the pipes open, so the
/// text goes in on stdin and stdout/stderr are not captured.
async fn feed(argv: &[&str], text: &str) -> Result<(), String> {
    let mut child = tokio::process::Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", argv[0]))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
        Ok(Ok(s)) if s.success() => Ok(()),
        Ok(Ok(s)) => Err(format!("{} exited with {s}", argv[0])),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Ok(()), // still serving the selection: that is success
    }
}

pub async fn clipboard_set(ctx: &ToolContext, text: &str) -> Result<(), String> {
    if desktop() == Desktop::Kde {
        if let Ok(r) = exec(ctx, &["qdbus6", "org.kde.klipper", "/klipper", "org.kde.klipper.klipper.setClipboardContents", text]).run().await {
            if r.success() {
                return Ok(());
            }
        }
    }
    if wayland() && which("wl-copy").is_some() {
        feed(&["wl-copy"], text).await
    } else if which("xclip").is_some() {
        feed(&["xclip", "-selection", "clipboard", "-i"], text).await
    } else if which("xsel").is_some() {
        feed(&["xsel", "--clipboard", "--input"], text).await
    } else {
        Err(format!("no clipboard tool: {}", super::install_cmd(if wayland() { "wl-clipboard" } else { "xclip" })))
    }
}

// ---------------------------------------------------------------- color scheme

/// `'prefer-dark'` -> `prefer-dark` (gsettings prints GVariant text).
pub fn gvariant_str(out: &str) -> String {
    out.trim().trim_matches('\'').to_string()
}

/// GTK3 theme to pair with a GNOME color scheme so legacy apps follow (Adwaita has a
/// built-in dark variant, Ubuntu's Yaru ships `Yaru-dark`; others are left alone).
pub fn gtk_theme_for(current: &str, dark: bool) -> Option<String> {
    let base = current.strip_suffix("-dark").unwrap_or(current);
    let family = base.split('-').next().unwrap_or(base);
    if !matches!(family, "Adwaita" | "Yaru") {
        return None;
    }
    let want = if dark { format!("{base}-dark") } else { base.to_string() };
    (want != current).then_some(want)
}

// ---------------------------------------------------------------- windows

#[derive(Debug, Clone, PartialEq)]
pub struct Win {
    pub id: String,
    pub desktop: String,
    pub pid: u32,
    pub class: String,
    pub title: String,
    pub focus: bool,
}

/// `wmctrl -l -p -x`: `0x04000007  0 1234   brave-browser.Brave-browser  host Title words`.
pub fn parse_wmctrl(out: &str) -> Vec<Win> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let id = it.next()?.to_string();
            let desk = it.next()?.to_string();
            let pid = it.next()?.parse().unwrap_or(0);
            let class = it.next()?.to_string();
            let _host = it.next()?;
            let title = it.collect::<Vec<_>>().join(" ");
            Some(Win { id, desktop: if desk == "-1" { "all".into() } else { (desk.parse::<i64>().unwrap_or(0) + 1).to_string() }, pid, class, title, focus: false })
        })
        .collect()
}

/// gdbus result `('[{"id":1,...}]',)` -> the JSON string inside.
pub fn gdbus_string(out: &str) -> Option<String> {
    let t = out.trim();
    let inner = t.strip_prefix("('").and_then(|x| x.strip_suffix("',)"))?;
    Some(inner.replace("\\'", "'").replace("\\\\", "\\"))
}

/// Window Calls extension `List` JSON -> windows.
pub fn parse_window_calls(json: &str) -> Vec<Win> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|w| Win {
                    id: w.get("id").map(|i| i.to_string()).unwrap_or_default(),
                    desktop: w.get("in_current_workspace").and_then(Value::as_bool).map(|c| if c { "current" } else { "other" }.to_string()).unwrap_or_default(),
                    pid: w.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
                    class: w.get("wm_class").and_then(Value::as_str).unwrap_or("").to_string(),
                    title: w.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                    focus: w.get("focus").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

const WC_DEST: &str = "org.gnome.Shell";
const WC_PATH: &str = "/org/gnome/Shell/Extensions/Windows";
const WC_IFACE: &str = "org.gnome.Shell.Extensions.Windows";

async fn window_calls(ctx: &ToolContext, method: &str, args: &[&str]) -> Option<String> {
    let m = format!("{WC_IFACE}.{method}");
    let mut argv = vec!["gdbus", "call", "--session", "--dest", WC_DEST, "--object-path", WC_PATH, "--method", &m];
    argv.extend_from_slice(args);
    let r = exec(ctx, &argv).run().await.ok()?;
    r.success().then_some(r.stdout)
}

/// Which window backend this session can use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WinBackend {
    KWin,
    WindowCalls,
    Wmctrl,
    None,
}

pub async fn window_backend(ctx: &ToolContext) -> WinBackend {
    if desktop() == Desktop::Kde {
        WinBackend::KWin
    } else if wayland() {
        if desktop() == Desktop::Gnome && window_calls(ctx, "List", &[]).await.is_some() {
            WinBackend::WindowCalls
        } else {
            WinBackend::None
        }
    } else if which("wmctrl").is_some() {
        WinBackend::Wmctrl
    } else {
        WinBackend::None
    }
}

pub fn no_backend_hint() -> String {
    if wayland() && desktop() == Desktop::Gnome {
        "window control on GNOME Wayland needs the 'Window Calls' GNOME Shell extension (extensions.gnome.org/extension/4724/window-calls/)".into()
    } else if wayland() {
        "window control on this Wayland desktop is not supported".into()
    } else {
        format!("window control needs wmctrl ({})", super::install_cmd("wmctrl"))
    }
}

/// Windows via the non-KDE backends (KDE uses KWin scripting in tools::apps).
pub async fn list_windows(ctx: &ToolContext, backend: WinBackend) -> Vec<Win> {
    match backend {
        WinBackend::Wmctrl => exec(ctx, &["wmctrl", "-l", "-p", "-x"]).run().await.map(|r| parse_wmctrl(&r.stdout)).unwrap_or_default(),
        WinBackend::WindowCalls => {
            let Some(json) = window_calls(ctx, "List", &[]).await.and_then(|o| gdbus_string(&o)) else { return vec![] };
            let mut wins = parse_window_calls(&json);
            for w in wins.iter_mut().filter(|w| w.title.is_empty()).take(40) {
                if let Some(t) = window_calls(ctx, "GetTitle", &[&w.id]).await.and_then(|o| gdbus_string(&o)) {
                    w.title = t;
                }
            }
            wins
        }
        _ => vec![],
    }
}

pub fn find_window<'a>(wins: &'a [Win], needle: &str) -> Option<&'a Win> {
    let n = needle.to_lowercase();
    wins.iter().find(|w| w.title.to_lowercase().contains(&n) || w.class.to_lowercase().contains(&n))
}

/// activate / close / minimize / move_to_desktop on one window.
pub async fn window_action(ctx: &ToolContext, backend: WinBackend, w: &Win, action: &str, desktop_no: i64) -> Result<(), String> {
    let r = match (backend, action) {
        (WinBackend::Wmctrl, "activate") => exec(ctx, &["wmctrl", "-i", "-a", &w.id]).run().await,
        (WinBackend::Wmctrl, "close") => exec(ctx, &["wmctrl", "-i", "-c", &w.id]).run().await,
        (WinBackend::Wmctrl, "move_to_desktop") => exec(ctx, &["wmctrl".to_string(), "-i".into(), "-r".into(), w.id.clone(), "-t".into(), (desktop_no.max(1) - 1).to_string()]).run().await,
        (WinBackend::Wmctrl, "minimize") if which("xdotool").is_some() => exec(ctx, &["xdotool", "windowminimize", &w.id]).run().await,
        (WinBackend::WindowCalls, "activate") => return window_calls(ctx, "Activate", &[&w.id]).await.map(|_| ()).ok_or_else(|| "Window Calls refused".into()),
        (WinBackend::WindowCalls, "close") => return window_calls(ctx, "Close", &[&w.id]).await.map(|_| ()).ok_or_else(|| "Window Calls refused".into()),
        (WinBackend::WindowCalls, "minimize") => return window_calls(ctx, "Minimize", &[&w.id]).await.map(|_| ()).ok_or_else(|| "Window Calls refused".into()),
        (WinBackend::WindowCalls, "move_to_desktop") => {
            let ws = (desktop_no.max(1) - 1).to_string();
            return window_calls(ctx, "MoveToWorkspace", &[&w.id, &ws]).await.map(|_| ()).ok_or_else(|| "Window Calls refused".into());
        }
        _ => return Err(format!("{action} is not supported by this window backend")),
    };
    match r {
        Ok(r) if r.success() => Ok(()),
        Ok(r) => Err(r.render()),
        Err(e) => Err(e.to_string()),
    }
}

/// The focused window where the X11 tools cannot see it (GNOME Wayland).
pub async fn active_window_wayland(ctx: &ToolContext) -> Option<Win> {
    if desktop() != Desktop::Gnome {
        return None;
    }
    let wins = list_windows(ctx, WinBackend::WindowCalls).await;
    wins.into_iter().find(|w| w.focus)
}

// ---------------------------------------------------------------- input (Wayland)

/// Linux input event code for an xdotool-style key name (ydotool needs codes).
pub fn key_code(name: &str) -> Option<u32> {
    let n = name.to_lowercase();
    let letters = "qwertyuiop";
    if n.len() == 1 {
        let c = n.chars().next()?;
        if let Some(i) = letters.find(c) {
            return Some(16 + i as u32);
        }
        if let Some(i) = "asdfghjkl".find(c) {
            return Some(30 + i as u32);
        }
        if let Some(i) = "zxcvbnm".find(c) {
            return Some(44 + i as u32);
        }
        if let Some(d) = c.to_digit(10) {
            return Some(if d == 0 { 11 } else { d + 1 });
        }
    }
    if let Some(f) = n.strip_prefix('f').and_then(|x| x.parse::<u32>().ok()) {
        return match f {
            1..=10 => Some(58 + f),
            11 => Some(87),
            12 => Some(88),
            _ => None,
        };
    }
    Some(match n.as_str() {
        "ctrl" | "control" | "control_l" => 29,
        "shift" | "shift_l" => 42,
        "alt" | "alt_l" => 56,
        "super" | "super_l" | "meta" => 125,
        "return" | "enter" => 28,
        "tab" => 15,
        "escape" | "esc" => 1,
        "backspace" => 14,
        "delete" => 111,
        "space" => 57,
        "up" => 103,
        "down" => 108,
        "left" => 105,
        "right" => 106,
        "home" => 102,
        "end" => 107,
        "page_up" | "prior" => 104,
        "page_down" | "next" => 109,
        "minus" => 12,
        "equal" => 13,
        "period" => 52,
        "comma" => 51,
        "slash" => 53,
        _ => return None,
    })
}

/// `ctrl+shift+t` -> `29:1 42:1 20:1 20:0 42:0 29:0` (press in order, release reversed).
pub fn ydotool_key_args(combo: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for chord in combo.split_whitespace() {
        let codes: Vec<u32> = chord.split('+').map(key_code).collect::<Option<_>>()?;
        out.extend(codes.iter().map(|c| format!("{c}:1")));
        out.extend(codes.iter().rev().map(|c| format!("{c}:0")));
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_values_and_themes() {
        assert_eq!(gvariant_str("'prefer-dark'\n"), "prefer-dark");
        assert_eq!(gtk_theme_for("Yaru", true).as_deref(), Some("Yaru-dark"));
        assert_eq!(gtk_theme_for("Yaru-dark", false).as_deref(), Some("Yaru"));
        assert_eq!(gtk_theme_for("Adwaita", true).as_deref(), Some("Adwaita-dark"));
        assert_eq!(gtk_theme_for("Adwaita-dark", true), None, "already dark");
        assert_eq!(gtk_theme_for("Nordic", true), None, "unknown theme left alone");
    }

    #[test]
    fn window_parsers() {
        let w = parse_wmctrl("0x04000007  0 1234   brave-browser.Brave-browser  box GitHub - Brave\n0x05000001 -1 99 plank.Plank box plank\n");
        assert_eq!(w[0], Win { id: "0x04000007".into(), desktop: "1".into(), pid: 1234, class: "brave-browser.Brave-browser".into(), title: "GitHub - Brave".into(), focus: false });
        assert_eq!(w[1].desktop, "all");
        let raw = r#"('[{"wm_class":"firefox","pid":4242,"id":2571357406,"focus":true,"in_current_workspace":true,"title":"Marko\'s page"}]',)"#;
        let json = gdbus_string(raw).unwrap();
        let wins = parse_window_calls(&json);
        assert_eq!((wins[0].id.as_str(), wins[0].pid, wins[0].focus, wins[0].title.as_str()), ("2571357406", 4242, true, "Marko's page"));
        assert_eq!(find_window(&wins, "FIREFOX").unwrap().pid, 4242);
        assert!(gdbus_string("Error: GDBus.Error").is_none());
    }

    #[test]
    fn ydotool_keys() {
        assert_eq!(ydotool_key_args("ctrl+s").unwrap(), vec!["29:1", "31:1", "31:0", "29:0"]);
        assert_eq!(ydotool_key_args("ctrl+shift+t Return").unwrap(), vec!["29:1", "42:1", "20:1", "20:0", "42:0", "29:0", "28:1", "28:0"]);
        assert_eq!(key_code("F12"), Some(88));
        assert_eq!(key_code("0"), Some(11));
        assert!(ydotool_key_args("ctrl+hyper").is_none());
    }
}
