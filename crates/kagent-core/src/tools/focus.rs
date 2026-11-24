//! What the user is looking at and acting on (docs/specs/tools.md section 14):
//! `focus_context` (active window, its open files, clipboard files, recent files,
//! newest downloads), `clipboard` (get/set via Klipper) and `input` (synthetic keyboard
//! and mouse via xdotool on X11).

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use super::xdg;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// `_NET_ACTIVE_WINDOW(WINDOW): window id # 0x5200078` -> `0x5200078`.
pub fn parse_active(out: &str) -> Option<String> {
    out.split('#').nth(1).map(|x| x.trim().to_string()).filter(|x| x.starts_with("0x") && x != "0x0")
}

/// Regular user files a process holds open (the video in a player, the PDF in a
/// viewer). Skips config, caches, libraries and devices.
pub fn interesting_open_file(path: &str, home: &str) -> bool {
    if !path.starts_with(home) || path.contains("/.") || path.ends_with(" (deleted)") {
        return false;
    }
    let lower = path.to_lowercase();
    !(lower.ends_with(".db") || lower.ends_with(".sqlite") || lower.ends_with(".lock") || lower.ends_with(".log") || lower.contains("/snap/"))
}

fn open_files(pid: u32) -> Vec<String> {
    let home = xdg::home().display().to_string();
    let mut v: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .map(|rd| rd.flatten().filter_map(|e| std::fs::read_link(e.path()).ok()).map(|p| p.display().to_string()).filter(|p| interesting_open_file(p, &home)).collect())
        .unwrap_or_default();
    v.sort();
    v.dedup();
    v.truncate(10);
    v
}

/// Clipboard text that is a file list (Dolphin copy) -> local paths.
pub fn clipboard_paths(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter_map(|l| xdg::uri_to_path(l).or_else(|| l.starts_with('/').then(|| l.to_string())))
        .filter(|p| std::path::Path::new(p).exists())
        .collect()
}

async fn clipboard_text(ctx: &ToolContext) -> Option<String> {
    crate::platform::desktop::clipboard_get(ctx).await
}

fn newest(dir: PathBuf, n: usize) -> Vec<String> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    if !m.is_file() {
                        return None;
                    }
                    Some((m.modified().ok()?, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().take(n).map(|(t, p)| format!("{}  ({})", p.display(), chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M"))).collect()
}

// ---------------------------------------------------------------- focus_context

pub struct FocusContext {
    contract: ToolContract,
}

impl FocusContext {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "focus_context",
                "Resolve 'this file', 'this window', 'what I'm looking at': the active window (title, app, files it has open), files copied to the clipboard (e.g. Ctrl+C in Dolphin), recently used files and the newest downloads. Clipboard text is only shown when include_clipboard_text is true.",
                json!({"type":"object","properties":{"include_clipboard_text":{"type":"boolean"}}}),
            ),
        }
    }
}

#[async_trait]
impl Tool for FocusContext {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let mut out = Vec::new();
        let root = exec(&ctx, &["xprop", "-root", "_NET_ACTIVE_WINDOW"]).run().await?;
        let wl = if crate::platform::wayland() { crate::platform::desktop::active_window_wayland(&ctx).await } else { None };
        if let Some(w) = &wl {
            out.push(format!("Active window: {} [{}] pid {}", w.title, w.class, w.pid));
            if let Ok(cwd) = std::fs::read_link(format!("/proc/{}/cwd", w.pid)) {
                out.push(format!("  working dir: {}", cwd.display()));
            }
            let files = open_files(w.pid);
            if !files.is_empty() {
                out.push(format!("  open files:\n{}", files.iter().map(|f| format!("    {f}")).collect::<Vec<_>>().join("\n")));
            }
        }
        match parse_active(&root.stdout).filter(|_| wl.is_none()) {
            Some(id) => {
                let p = exec(&ctx, &["xprop", "-id", &id, "_NET_WM_NAME", "WM_CLASS", "_NET_WM_PID"]).run().await?;
                let val = |k: &str| p.stdout.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_once('=')).map(|(_, v)| v.trim().trim_matches('"').replace("\", \"", " / ")).unwrap_or_default();
                let pid: u32 = val("_NET_WM_PID").parse().unwrap_or(0);
                out.push(format!("Active window: {} [{}] pid {pid}", val("_NET_WM_NAME"), val("WM_CLASS")));
                if pid > 0 {
                    if let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                        out.push(format!("  working dir: {}", cwd.display()));
                    }
                    let files = open_files(pid);
                    if !files.is_empty() {
                        out.push(format!("  open files:\n{}", files.iter().map(|f| format!("    {f}")).collect::<Vec<_>>().join("\n")));
                    }
                }
            }
            None if wl.is_some() => {}
            None => out.push(format!("Active window: unknown ({})", if crate::platform::wayland() { "Wayland: on GNOME install the Window Calls extension" } else { "xprop failed" })),
        }
        match clipboard_text(&ctx).await {
            Some(t) => {
                let paths = clipboard_paths(&t);
                if !paths.is_empty() {
                    out.push(format!("Clipboard files:\n{}", paths.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n")));
                } else if input.get("include_clipboard_text").and_then(Value::as_bool).unwrap_or(false) {
                    out.push(format!("Clipboard text: {}", crate::util::truncate_str(&t, 1_000)));
                } else if !t.is_empty() {
                    out.push(format!("Clipboard: text, {} chars (not shown)", t.chars().count()));
                }
            }
            None => out.push("Clipboard: unavailable (no Klipper, wl-clipboard or xclip)".into()),
        }
        let xbel = xdg::home().join(".local/share/recently-used.xbel");
        if let Ok(t) = std::fs::read_to_string(xbel) {
            let recent: Vec<(String, String)> = xdg::parse_recent(&t, 40).into_iter().filter(|(p, _)| std::path::Path::new(p).exists()).take(8).collect();
            if !recent.is_empty() {
                out.push(format!("Recently used:\n{}", recent.iter().map(|(p, m)| format!("  {p}  ({})", m.get(..16).unwrap_or(m).replace('T', " "))).collect::<Vec<_>>().join("\n")));
            }
        }
        let dl = newest(xdg::user_dir("DOWNLOAD", "Downloads"), 5);
        if !dl.is_empty() {
            out.push(format!("Newest downloads:\n{}", dl.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n")));
        }
        Ok(ToolOutput::ok(out.join("\n")))
    }
}

// ---------------------------------------------------------------- clipboard

pub struct Clipboard {
    contract: ToolContract,
}

impl Clipboard {
    pub fn new() -> Self {
        let mut contract = ToolContract::read_only(
            "clipboard",
            "Read the clipboard text, or put text on the clipboard (e.g. a command or an email draft for the user to paste).",
            json!({"type":"object","required":["action"],"properties":{"action":{"type":"string","enum":["get","set"]},"text":{"type":"string"}}}),
        )
        .mutating(SideEffect::System, Risk::Low);
        contract.needs_approval = false;
        Self { contract }
    }
}

#[async_trait]
impl Tool for Clipboard {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let a = CallAssessment::from_contract(&self.contract, format!("clipboard {}", s(input, "action")), "clipboard".into());
        if s(input, "action") == "get" { a.as_read_only() } else { a }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        if s(&input, "action") == "set" {
            return Ok(match crate::platform::desktop::clipboard_set(&ctx, s(&input, "text")).await {
                Ok(()) => ToolOutput::ok(format!("clipboard set ({} chars)", s(&input, "text").chars().count())),
                Err(e) => ToolOutput::error(e),
            });
        }
        Ok(match clipboard_text(&ctx).await {
            Some(t) => ToolOutput::ok(crate::util::truncate_str(&t, 8_000)),
            None => ToolOutput::error("clipboard unavailable (no Klipper, wl-clipboard or xclip)"),
        })
    }
}

// ---------------------------------------------------------------- input

pub struct Input {
    contract: ToolContract,
}

impl Input {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "input",
                "Synthetic keyboard/mouse input (xdotool on X11, ydotool on Wayland): type text, press a key combo (ctrl+s, alt+Tab, Return), click/move the mouse at screen coordinates, scroll. Always give `window` (part of its title) so it is focused right before the input. For chat messages to people use `message` instead. Prefer real tools (open, media, browser, terminal) when they exist.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["type","key","click","move","scroll"]},
                    "window":{"type":"string","description":"part of the target window's title (e.g. Signal): it is focused right before the input, so the approval click cannot steal the keys"},
                    "text":{"type":"string","description":"type: text to type"},
                    "enter":{"type":"boolean","description":"type: press Enter after a short pause"},
                    "keys":{"type":"string","description":"key: xdotool key names, e.g. ctrl+s, alt+Tab, Return, ctrl+shift+t"},
                    "x":{"type":"integer"},"y":{"type":"integer"},
                    "button":{"type":"string","enum":["left","middle","right"]},
                    "double":{"type":"boolean"},
                    "amount":{"type":"integer","description":"scroll: lines, negative = up"}
                }}),
            )
            .mutating(SideEffect::System, Risk::Medium),
        }
    }
}

/// POSIX ERE escape for `xdotool search --name`.
pub fn regex_escape(s: &str) -> String {
    s.chars().map(|c| if ".^$*+?()[]{}|\\".contains(c) { format!("\\{c}") } else { c.to_string() }).collect()
}

/// xdotool chain prefix that focuses a window by title (synchronously) so the
/// following keys go there, even if an approval click focused KAgent's own window.
pub fn activate_chain(window: &str) -> Vec<String> {
    vec!["search".into(), "--limit".into(), "1".into(), "--name".into(), regex_escape(window.trim()), "windowactivate".into(), "--sync".into()]
}

/// Separate xdotool command that waits, then presses Enter. A pause is needed: apps
/// treat a burst of keys as a paste, and Enter inside a paste is a newline (Claude Code).
pub fn enter_cmd() -> Vec<String> {
    ["xdotool", "sleep", "0.5", "key", "Return"].iter().map(|s| s.to_string()).collect()
}

/// Validated xdotool argv for an input call.
pub fn input_argv(input: &Value) -> Result<Vec<String>, String> {
    let num = |k: &str| input.get(k).and_then(Value::as_i64);
    let mut v = vec!["xdotool".to_string()];
    let window = s(input, "window").trim();
    if !window.is_empty() {
        if window.len() > 120 {
            return Err("window name too long".into());
        }
        v.extend(activate_chain(window));
    }
    match s(input, "action") {
        "type" => {
            let t = s(input, "text");
            if t.is_empty() || t.chars().count() > 2_000 {
                return Err("text required (max 2000 chars)".into());
            }
            // `type` takes every remaining argument as text, so nothing may follow it in
            // the chain: Enter is a separate command (see `enter_cmd`).
            v.extend(["type".into(), "--delay".into(), "8".into(), "--".into(), t.into()]);
        }
        "key" => {
            let k = s(input, "keys").trim();
            let ok = regex::Regex::new(r"^[A-Za-z0-9_]+(\+[A-Za-z0-9_]+)*( [A-Za-z0-9_]+(\+[A-Za-z0-9_]+)*){0,9}$").expect("static");
            if !ok.is_match(k) {
                return Err(format!("bad key combo '{k}'"));
            }
            v.push("key".into());
            v.push("--".into());
            v.extend(k.split(' ').map(String::from));
        }
        "move" | "click" => {
            if let (Some(x), Some(y)) = (num("x"), num("y")) {
                if !(0..=20_000).contains(&x) || !(0..=20_000).contains(&y) {
                    return Err("coordinates out of range".into());
                }
                v.extend(["mousemove".into(), "--sync".into(), x.to_string(), y.to_string()]);
            } else if s(input, "action") == "move" {
                return Err("x and y required".into());
            }
            if s(input, "action") == "click" {
                let b = match s(input, "button") { "middle" => "2", "right" => "3", _ => "1" };
                v.extend(["click".into()]);
                if input.get("double").and_then(Value::as_bool).unwrap_or(false) {
                    v.extend(["--repeat".into(), "2".into()]);
                }
                v.push(b.into());
            }
        }
        "scroll" => {
            let n = num("amount").unwrap_or(3).clamp(-50, 50);
            v.extend(["click".into(), "--repeat".into(), n.abs().max(1).to_string(), if n < 0 { "4" } else { "5" }.into()]);
        }
        a => return Err(format!("unknown action {a}")),
    }
    Ok(v)
}


/// ydotool (Wayland) argv: type, key (input event codes), click, move.
pub fn ydotool_argv(input: &Value) -> Result<Vec<String>, String> {
    let x11 = input_argv(input)?; // same validation
    let mut v = vec!["ydotool".to_string()];
    match s(input, "action") {
        "type" => v.extend(["type".into(), "--".into(), s(input, "text").into()]),
        "key" => {
            v.push("key".into());
            v.extend(crate::platform::desktop::ydotool_key_args(s(input, "keys")).ok_or("unsupported key name for ydotool")?);
        }
        "move" | "click" => {
            if let (Some(x), Some(y)) = (input.get("x").and_then(Value::as_i64), input.get("y").and_then(Value::as_i64)) {
                if s(input, "action") == "move" {
                    v.extend(["mousemove".into(), "--absolute".into(), "-x".into(), x.to_string(), "-y".into(), y.to_string()]);
                    return Ok(v);
                }
                return Err("on Wayland click at coordinates: move first, then click".into());
            }
            let code = match s(input, "button") { "right" => "0xC1", "middle" => "0xC2", _ => "0xC0" };
            v.push("click".into());
            if input.get("double").and_then(Value::as_bool).unwrap_or(false) {
                v.extend(["--repeat".into(), "2".into()]);
            }
            v.push(code.into());
        }
        _ => return Err(format!("{} is not supported on Wayland (validated: {})", s(input, "action"), x11.join(" "))),
    }
    Ok(v)
}

#[async_trait]
impl Tool for Input {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let what = match s(input, "action") {
            "type" => format!("type \"{}\"", crate::util::truncate_str(s(input, "text"), 80)),
            "key" => format!("press {}", s(input, "keys")),
            a => format!("mouse {a} {} {}", input.get("x").map(|v| v.to_string()).unwrap_or_default(), input.get("y").map(|v| v.to_string()).unwrap_or_default()),
        };
        let target = s(input, "window").trim();
        let into = if target.is_empty() { "the focused window".to_string() } else { format!("'{target}'") };
        let a = CallAssessment::from_contract(&self.contract, format!("{what} into {into}"), format!("input {} {target}", s(input, "action")).trim().to_string());
        // Typing a destructive command (into a terminal) is the same as running it.
        if s(input, "action") == "type" && crate::safety::looks_destructive(s(input, "text")) { a.destructive() } else { a }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        if crate::platform::wayland() {
            if xdg::which("ydotool").is_none() {
                return Ok(ToolOutput::error("synthetic input on Wayland needs ydotool + the ydotoold service (access to /dev/uinput)"));
            }
            let argv = match ydotool_argv(&input) {
                Ok(a) => a,
                Err(e) => return Ok(ToolOutput::error(e)),
            };
            let r = exec(&ctx, &argv).run().await?;
            return Ok(if r.success() { ToolOutput::ok(format!("{} done", s(&input, "action"))) } else { ToolOutput::error(format!("{} (is ydotoold running?)", r.render())) });
        }
        if xdg::which("xdotool").is_none() {
            return Ok(ToolOutput::error(format!("xdotool is not installed; the owner can run: {}", crate::platform::install_cmd("xdotool"))));
        }
        let argv = match input_argv(&input) {
            Ok(a) => a,
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        let mut r = exec(&ctx, &argv).run().await?;
        if r.success() && s(&input, "action") == "type" && input.get("enter").and_then(Value::as_bool).unwrap_or(false) {
            r = exec(&ctx, &enter_cmd()).run().await?;
        }
        Ok(if r.success() { ToolOutput::ok(format!("{} done", s(&input, "action"))) } else { ToolOutput::error(r.render()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_window_and_open_files() {
        assert_eq!(parse_active("_NET_ACTIVE_WINDOW(WINDOW): window id # 0x5200078").as_deref(), Some("0x5200078"));
        assert_eq!(parse_active("_NET_ACTIVE_WINDOW(WINDOW): window id # 0x0"), None);
        assert!(interesting_open_file("/home/u/Videos/film.mkv", "/home/u"));
        assert!(!interesting_open_file("/home/u/.cache/x", "/home/u"));
        assert!(!interesting_open_file("/usr/lib/libc.so", "/home/u"));
        assert!(!interesting_open_file("/home/u/places.sqlite", "/home/u"));
    }

    #[test]
    fn clipboard_file_lists() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a b.txt");
        std::fs::write(&f, "x").unwrap();
        let uri = super::super::media::to_uri(&f.display().to_string());
        assert_eq!(clipboard_paths(&format!("{uri}\nfile:///nope/zzz")), vec![f.display().to_string()]);
        assert!(clipboard_paths("hello world").is_empty());
    }

    #[test]
    fn input_validation() {
        assert_eq!(input_argv(&json!({"action":"key","keys":"ctrl+s"})).unwrap(), vec!["xdotool", "key", "--", "ctrl+s"]);
        assert_eq!(
            input_argv(&json!({"action":"type","text":"hi","window":"Signal (2)","enter":true})).unwrap(),
            vec!["xdotool", "search", "--limit", "1", "--name", "Signal \\(2\\)", "windowactivate", "--sync", "type", "--delay", "8", "--", "hi"]
        );
        assert!(input_argv(&json!({"action":"key","keys":"ctrl+s; rm -rf"})).is_err());
        assert_eq!(input_argv(&json!({"action":"type","text":"-n hi"})).unwrap().last().unwrap(), "-n hi");
        assert_eq!(input_argv(&json!({"action":"click","x":10,"y":20,"button":"right"})).unwrap(), vec!["xdotool", "mousemove", "--sync", "10", "20", "click", "3"]);
        assert_eq!(input_argv(&json!({"action":"scroll","amount":-2})).unwrap(), vec!["xdotool", "click", "--repeat", "2", "4"]);
        assert!(input_argv(&json!({"action":"move"})).is_err());
        assert_eq!(ydotool_argv(&json!({"action":"key","keys":"ctrl+s"})).unwrap(), vec!["ydotool", "key", "29:1", "31:1", "31:0", "29:0"]);
        assert_eq!(ydotool_argv(&json!({"action":"type","text":"-hi"})).unwrap(), vec!["ydotool", "type", "--", "-hi"]);
        assert_eq!(ydotool_argv(&json!({"action":"click","button":"right"})).unwrap(), vec!["ydotool", "click", "0xC1"]);
        assert!(ydotool_argv(&json!({"action":"scroll"})).is_err());
    }
}
