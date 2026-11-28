//! Desktop apps and windows (docs/specs/tools.md section 9): windows (KWin), notify,
//! open. Split from desktop.rs to keep files small.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use super::xdg;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

fn contract(name: &str, description: &str, schema: Value) -> ToolContract {
    ToolContract::read_only(name, description, schema).mutating(SideEffect::System, Risk::Low)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

// ---------------------------------------------------------------- windows

pub struct Windows {
    contract: ToolContract,
}

impl Windows {
    pub fn new() -> Self {
        Self {
            contract: contract(
                "windows",
                "Desktop windows (KDE: KWin; GNOME Wayland: Window Calls extension; X11: wmctrl): list (read-only), activate, close, minimize, or move one to a virtual desktop. Match a window by part of its title or application name.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["list","activate","close","minimize","move_to_desktop"]},
                    "window":{"type":"string","description":"part of the window title or app name"},
                    "desktop":{"type":"integer","description":"move_to_desktop: 1-based desktop number"}
                }}),
            ),
        }
    }
}

/// `_NET_CLIENT_LIST(WINDOW): window id # 0x4a00007, 0x5200003` -> ids.
pub fn parse_client_list(out: &str) -> Vec<String> {
    out.split('#').nth(1).map(|ids| ids.split(',').map(|s| s.trim().to_string()).filter(|s| s.starts_with("0x")).collect()).unwrap_or_default()
}

fn xprop_value(out: &str, key: &str) -> String {
    out.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_once('=').map(|(_, v)| v.trim().trim_matches('"').replace("\", \"", " / ")))
        .unwrap_or_default()
}

/// KWin script acting on the first window whose caption or resource class contains the
/// needle. JSON-escaped needle; no other input reaches the script.
pub fn kwin_script(action: &str, needle: &str, desktop: i64) -> String {
    let n = serde_json::to_string(&needle.to_lowercase()).unwrap_or_else(|_| "\"\"".into());
    let act = match action {
        "activate" => "workspace.activeWindow = w;".to_string(),
        "close" => "w.closeWindow();".to_string(),
        "minimize" => "w.minimized = true;".to_string(),
        // KAgent's own widget: above everything, on every desktop, out of the taskbar.
        "keep_above" => "w.keepAbove = true; w.onAllDesktops = true; w.skipTaskbar = true; w.skipPager = true;".to_string(),
        _ => format!("var d = workspace.desktops[{}]; if (d) {{ w.desktops = [d]; }}", desktop.max(1) - 1),
    };
    format!(
        "var n = {n};\nvar list = workspace.windowList();\nfor (var i = 0; i < list.length; i++) {{\n  var w = list[i];\n  if (!w.normalWindow) continue;\n  if (w.caption.toLowerCase().indexOf(n) >= 0 || String(w.resourceClass).toLowerCase().indexOf(n) >= 0) {{ {act} break; }}\n}}\n"
    )
}

/// Load, run and unload a KWin script. Returns (ok, loadScript result).
pub async fn run_kwin_script(ctx: &ToolContext, script: &str) -> anyhow::Result<(bool, super::exec::ExecResult)> {
    let path = std::env::temp_dir().join(format!("kagent-kwin-{}.js", uuid::Uuid::new_v4().simple()));
    tokio::fs::write(&path, script).await?;
    let plugin = format!("kagent_{}", uuid::Uuid::new_v4().simple());
    let p = path.display().to_string();
    let load = exec(ctx, &["qdbus6", "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.loadScript", &p, &plugin]).run().await?;
    let id = load.stdout.trim().to_string();
    let ok = load.success() && id.parse::<i64>().is_ok_and(|i| i >= 0);
    if ok {
        let _ = exec(ctx, &["qdbus6", "org.kde.KWin", &format!("/Scripting/Script{id}"), "org.kde.kwin.Script.run"]).run().await;
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = exec(ctx, &["qdbus6", "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.unloadScript", &plugin]).run().await;
    }
    let _ = tokio::fs::remove_file(&path).await;
    Ok((ok, load))
}

#[async_trait]
impl Tool for Windows {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = s(input, "action");
        let a = CallAssessment::from_contract(&self.contract, format!("window {action} {}", s(input, "window")).trim().to_string(), format!("windows {action}"));
        match action {
            "list" => a.as_read_only(),
            // Closing a window can lose unsaved work.
            "close" => a.destructive(),
            _ => a,
        }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let action = s(&input, "action");
        let backend = crate::platform::desktop::window_backend(&ctx).await;
        use crate::platform::desktop::WinBackend;
        if matches!(backend, WinBackend::WindowCalls | WinBackend::Wmctrl) {
            let wins = crate::platform::desktop::list_windows(&ctx, backend).await;
            if action == "list" {
                return Ok(if wins.is_empty() {
                    ToolOutput::error("no windows listed")
                } else {
                    let rows = wins.iter().take(80).map(|w| format!("desktop {}  pid {}  [{}]  {}{}", w.desktop, w.pid, w.class, w.title, if w.focus { "  (focused)" } else { "" })).collect::<Vec<_>>().join("\n");
                    ToolOutput::ok(format!("{rows}\n{}", crate::util::more_note(wins.len(), 80, "name a window with `window` to act on one")).trim_end().to_string())
                });
            }
            let needle = s(&input, "window");
            if needle.trim().is_empty() {
                return Ok(ToolOutput::error("window (title or app name) required"));
            }
            let Some(w) = crate::platform::desktop::find_window(&wins, needle) else {
                return Ok(ToolOutput::error(format!("no window matches '{needle}'")));
            };
            let desk = input.get("desktop").and_then(Value::as_i64).unwrap_or(1);
            return Ok(match crate::platform::desktop::window_action(&ctx, backend, w, action, desk).await {
                Ok(()) => ToolOutput::ok(format!("window {action} '{}' done", w.title)),
                Err(e) => ToolOutput::error(e),
            });
        }
        if backend == WinBackend::None && action != "list" {
            return Ok(ToolOutput::error(crate::platform::desktop::no_backend_hint()));
        }
        if action == "list" {
            let root = exec(&ctx, &["xprop", "-root", "_NET_CLIENT_LIST"]).run().await?;
            let ids = parse_client_list(&root.stdout);
            if ids.is_empty() {
                return Ok(ToolOutput::error(if crate::platform::wayland() && backend == WinBackend::KWin {
                    "listing windows on KDE Wayland is not supported; activate/close by name works".to_string()
                } else {
                    format!("could not list windows: {}", crate::platform::desktop::no_backend_hint())
                }));
            }
            let mut rows = Vec::new();
            for id in ids.iter().take(80) {
                let p = exec(&ctx, &["xprop", "-id", id, "_NET_WM_NAME", "WM_CLASS", "_NET_WM_DESKTOP", "_NET_WM_PID"]).run().await?;
                let desk = match xprop_value(&p.stdout, "_NET_WM_DESKTOP").parse::<i64>() { Ok(d) if d >= 0xFFFF_FFFF => "all".to_string(), Ok(d) => (d + 1).to_string(), Err(_) => "-".into() };
                rows.push(format!("desktop {desk}  pid {}  [{}]  {}", xprop_value(&p.stdout, "_NET_WM_PID"), xprop_value(&p.stdout, "WM_CLASS"), xprop_value(&p.stdout, "_NET_WM_NAME")));
            }
            return Ok(ToolOutput::ok(rows.join("\n")));
        }
        let needle = s(&input, "window");
        if needle.trim().is_empty() {
            return Ok(ToolOutput::error("window (title or app name) required"));
        }
        let script = kwin_script(action, needle, input.get("desktop").and_then(Value::as_i64).unwrap_or(1));
        let (ok, load) = run_kwin_script(&ctx, &script).await?;
        let text = format!("window {action} '{needle}' {}", if ok { "sent to KWin" } else { "failed: KWin scripting unavailable" });
        Ok(if ok { ToolOutput::ok(text) } else { ToolOutput::error(format!("{text}\n{}", load.render())) })
    }
}

// ---------------------------------------------------------------- notify / open

pub struct Notify {
    contract: ToolContract,
}

impl Notify {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "notify",
                "Show a desktop notification to the user (no approval needed).",
                json!({"type":"object","required":["title"],"properties":{
                    "title":{"type":"string"},
                    "body":{"type":"string"},
                    "urgency":{"type":"string","enum":["low","normal","critical"]}
                }}),
            ),
        }
    }
}

#[async_trait]
impl Tool for Notify {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let urgency = match s(&input, "urgency") { "low" => "low", "critical" => "critical", _ => "normal" };
        let r = exec(&ctx, &["notify-send", "-a", "KAgent", "-i", "audio-input-microphone", "-u", urgency, s(&input, "title"), s(&input, "body")]).run().await?;
        Ok(if r.success() { ToolOutput::ok("notification shown") } else { ToolOutput::error(r.render()) })
    }
}

pub struct Open {
    contract: ToolContract,
}

impl Open {
    pub fn new() -> Self {
        Self {
            contract: contract(
                "open",
                "Open a URL or a file/folder with its default app, launch an application by name (vlc, firefox, kate, dolphin, \"VS Code\"), or open a file/URL in a specific app (target + app, e.g. play a video in VLC). File paths must be absolute (~ allowed).",
                json!({"type":"object","properties":{
                    "target":{"type":"string","description":"absolute path or URL; may be omitted when launching an app"},
                    "app":{"type":"string","description":"application name or .desktop id"}
                }}),
            ),
        }
    }
}

/// Opening a web page, a folder, a plain (non-executable) file or launching an app is
/// harmless; other URL schemes and executables keep the approval card.
pub fn open_is_harmless(target: &str) -> bool {
    let t = target.trim();
    if t.is_empty() {
        return true;
    }
    if t.starts_with("https://") || t.starts_with("http://") {
        return true;
    }
    let p = xdg::expand(t);
    if !p.is_absolute() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(&p) {
        Ok(m) if m.is_dir() => true,
        Ok(m) => m.is_file() && m.permissions().mode() & 0o111 == 0 && !t.ends_with(".desktop"),
        Err(_) => false,
    }
}

#[async_trait]
impl Tool for Open {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let (t, app) = (s(input, "target"), s(input, "app"));
        let summary = match (t.is_empty(), app.is_empty()) {
            (false, false) => format!("open {t} in {app}"),
            (true, false) => format!("launch {app}"),
            _ => format!("open {t}"),
        };
        let mut a = CallAssessment::from_contract(&self.contract, summary.clone(), summary);
        let bare_app = !t.is_empty() && !t.contains('/') && !t.contains(':') && !t.starts_with('~');
        if bare_app || open_is_harmless(t) {
            a.needs_approval = false;
        }
        a
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let t = s(&input, "target").trim().to_string();
        let mut app_name = s(&input, "app").trim().to_string();
        if t.starts_with('-') || app_name.starts_with('-') {
            return Ok(ToolOutput::error("invalid target"));
        }
        // A bare word target is an application name.
        let looks_like_app = !t.is_empty() && !t.contains('/') && !t.contains(':') && !t.starts_with('~');
        let target = if looks_like_app && app_name.is_empty() {
            app_name = t.clone();
            String::new()
        } else if t.contains("://") || t.is_empty() {
            t.clone()
        } else {
            let p = xdg::expand(&t);
            if !p.is_absolute() {
                return Ok(ToolOutput::error("file paths must be absolute"));
            }
            if !p.exists() {
                return Ok(ToolOutput::error(format!("{} does not exist", p.display())));
            }
            p.display().to_string()
        };
        if target.is_empty() && app_name.is_empty() {
            return Ok(ToolOutput::error("target or app required"));
        }
        let argv: Vec<String> = if app_name.is_empty() {
            vec!["xdg-open".into(), target.clone()]
        } else {
            let Some(app) = xdg::find_app(&app_name) else {
                return Ok(ToolOutput::error(format!("application '{app_name}' is not installed (no .desktop entry); it can be installed with the `package` tool")));
            };
            let mut v = vec!["gio".to_string(), "launch".into(), app.path.display().to_string()];
            if !target.is_empty() {
                v.push(target.clone());
            }
            app_name = if app.name.is_empty() { app.id } else { app.name };
            v
        };
        // Launchers stay alive with the app: do not wait for them.
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let what = match (target.is_empty(), app_name.is_empty()) {
            (true, _) => app_name.clone(),
            (false, true) => target.clone(),
            (false, false) => format!("{target} in {app_name}"),
        };
        match cmd.spawn() {
            Ok(_) => Ok(ToolOutput::ok(format!("opened {what}"))),
            Err(e) => Ok(ToolOutput::error(format!("could not open {what}: {e}"))),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_list_and_kwin_script() {
        assert_eq!(parse_client_list("_NET_CLIENT_LIST(WINDOW): window id # 0x4a00007, 0x5200003"), vec!["0x4a00007", "0x5200003"]);
        let sc = kwin_script("close", "Fire\"fox'); evil(", 1);
        assert!(sc.contains(r#"var n = "fire\"fox'); evil(";"#), "{sc}");
        assert!(sc.contains("w.closeWindow();"));
        assert!(kwin_script("move_to_desktop", "x", 3).contains("workspace.desktops[2]"));
    }

    #[test]
    fn open_approval_only_for_risky_targets() {
        let d = tempfile::tempdir().unwrap();
        let movie = d.path().join("a b.mkv");
        std::fs::write(&movie, "x").unwrap();
        let script = d.path().join("run.sh");
        std::fs::write(&script, "#!/bin/sh").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(open_is_harmless("https://example.com"));
        assert!(open_is_harmless(&movie.display().to_string()));
        assert!(open_is_harmless(&d.path().display().to_string()));
        assert!(open_is_harmless(""));
        assert!(!open_is_harmless(&script.display().to_string()));
        assert!(!open_is_harmless("steam://run/1"));
        assert!(!open_is_harmless("relative.txt"));
    }
}
