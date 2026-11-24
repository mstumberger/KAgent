//! KAgent desktop shell: main window (chat/control center), transparent always-on-top
//! presence overlay, tray (open, overlay, stop everything, quit) and a global hotkey
//! (Super+Space) that shows the overlay and starts listening. All logic stays in
//! kagentd; this is only windows around http://127.0.0.1:7878.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const DAEMON: &str = "127.0.0.1:7878";
const HOTKEY: &str = "Super+Space";

fn config_dir() -> PathBuf {
    match std::env::var_os("KAGENT_HOME") {
        Some(h) => PathBuf::from(h).join("config"),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config/kagent"),
    }
}

fn daemon_up() -> bool {
    TcpStream::connect_timeout(&DAEMON.parse().unwrap(), Duration::from_millis(300)).is_ok()
}

/// Start kagentd if nothing listens yet (PATH, or next to this binary).
fn ensure_daemon() {
    if daemon_up() {
        return;
    }
    let sibling = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("kagentd")));
    let bin = sibling.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from("kagentd"));
    let _ = std::process::Command::new(bin)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    let start = Instant::now();
    while !daemon_up() && start.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn token() -> String {
    std::fs::read_to_string(config_dir().join("token")).unwrap_or_default().trim().to_string()
}

/// Tiny HTTP POST (no extra HTTP client dependency for two calls).
fn post(path: &str) {
    if let Ok(mut s) = TcpStream::connect(DAEMON) {
        let req = format!(
            "POST {path} HTTP/1.1\r\nHost: {DAEMON}\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            token()
        );
        let _ = s.write_all(req.as_bytes());
        let mut buf = [0u8; 512];
        let _ = s.read(&mut buf);
    }
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Hotkey: show the overlay and start talking; again: stop and hide.
fn toggle_overlay(app: &AppHandle, talk: bool) {
    let Some(w) = app.get_webview_window("overlay") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.eval("window.dispatchEvent(new KeyboardEvent('keydown',{code:'Escape'}))");
        let _ = w.hide();
    } else {
        let _ = w.show();
        let _ = w.set_focus();
        if talk {
            let _ = w.eval("window.dispatchEvent(new KeyboardEvent('keydown',{code:'Space'}))");
        }
    }
}

fn main() {
    ensure_daemon();
    let tok = token();
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_overlay(app, true);
                    }
                })
                .build(),
        )
        .setup(move |app| {
            let base = format!("http://{DAEMON}");
            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(format!("{base}/#token={tok}").parse()?))
                .title("KAgent")
                .inner_size(1200.0, 820.0)
                .build()?;
            WebviewWindowBuilder::new(app, "overlay", WebviewUrl::External(format!("{base}/?overlay#token={tok}").parse()?))
                .title("KAgent overlay")
                .inner_size(380.0, 560.0)
                .transparent(true)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible(false)
                .build()?;

            let open = MenuItem::with_id(app, "open", "Open KAgent", true, None::<&str>)?;
            let overlay = MenuItem::with_id(app, "overlay", "Talk (Super+Space)", true, None::<&str>)?;
            let stop = MenuItem::with_id(app, "stop", "Stop everything", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &overlay, &stop, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("bundle icon"))
                .tooltip("KAgent")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id().as_ref() {
                    "open" => show_main(app),
                    "overlay" => toggle_overlay(app, true),
                    "stop" => post("/api/stop-all"),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            if let Err(e) = app.global_shortcut().register(HOTKEY) {
                eprintln!("kagent-app: could not register {HOTKEY}: {e} (bind it in KDE settings to `kagent-app` instead)");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window keeps the tray (and the daemon) running.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("kagent-app failed");
}
