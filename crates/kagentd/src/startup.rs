//! Startup behaviour (settings.startup): the systemd user unit for autostart and
//! the greeting notification. Everything here is user-level - no sudo.

use std::path::PathBuf;

use crate::settings::{Greeting, Persona};

const UNIT: &str = "kagentd.service";

fn unit_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/systemd/user")
        .join(UNIT)
}

fn run(argv: &[&str]) -> Option<String> {
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Some(text)
    } else {
        tracing::warn!("{} failed: {}", argv[0], text.trim());
        None
    }
}

/// Write the user unit so it points at the currently running binary, then enable
/// (true) or disable (false) it. Never starts a second copy: `enable` takes effect
/// at the next login; when the daemon was started by the unit it just keeps running.
/// Idempotent and safe to call on every start and on every settings change.
pub fn apply_autostart(enabled: bool) {
    if !enabled {
        // Keep the unit file (the daemon may still be running under it), just
        // stop it from starting at login.
        if run(&["systemctl", "--user", "disable", UNIT]).is_some() {
            tracing::info!("autostart off: {} disabled", UNIT);
        }
        return;
    }
    let Some(exe) = std::env::current_exe().ok().map(|p| p.canonicalize().unwrap_or(p)) else {
        tracing::warn!("autostart: cannot resolve the running binary path");
        return;
    };
    let workdir = exe.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let unit = format!(
        "# Managed by kagentd (settings.startup.autostart). Manual edits are overwritten.\n\
[Unit]\n\
Description=KAgent system agent daemon\n\
After=graphical-session.target pipewire.service\n\
PartOf=graphical-session.target\n\
\n\
[Service]\n\
WorkingDirectory={}\n\
ExecStart={} serve\n\
Restart=on-failure\n\
RestartSec=3\n\
\n\
[Install]\n\
WantedBy=graphical-session.target\n",
        workdir.display(),
        exe.display()
    );
    let path = unit_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let changed = std::fs::read_to_string(&path).map(|old| old != unit).unwrap_or(true);
    if changed {
        if let Err(e) = std::fs::write(&path, unit) {
            tracing::warn!("autostart: cannot write {}: {e}", path.display());
            return;
        }
        tracing::info!("autostart: unit written -> {}", path.display());
    }
    run(&["systemctl", "--user", "daemon-reload"]);
    if run(&["systemctl", "--user", "enable", UNIT]).is_some() {
        tracing::info!("autostart on: {} enabled (starts at login)", UNIT);
    }
}

/// Greeting text: custom text if set, else time-of-day + persona address.
pub fn greeting_text(greeting: &Greeting, persona: &Persona, hour: u32) -> String {
    if !greeting.text.trim().is_empty() {
        return greeting.text.clone();
    }
    let who = if persona.address.is_empty() { persona.name.clone() } else { persona.address.clone() };
    let part = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    format!("{part}, {who}. KAgent is up and watching.")
}

/// Greet the user once the daemon is up: desktop notification, auto text from the
/// persona and time of day unless a custom text is set.
pub fn greet(greeting: &Greeting, persona: &Persona) {
    if !greeting.enabled || std::env::var_os("KAGENT_NO_NOTIFY").is_some() {
        return;
    }
    let hour = chrono::Timelike::hour(&chrono::Local::now());
    let text = greeting_text(greeting, persona, hour);
    let title = persona.name.clone();
    tracing::info!("greeting: {text}");
    // Fire and forget: a missing notification daemon must never block startup.
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "KAgent", "-i", "kagent", "-u", "normal", &title, &text])
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persona() -> Persona {
        Persona { name: "KAgent".into(), address: "Marko".into(), language: "auto".into() }
    }

    #[test]
    fn greeting_text_by_time_and_custom() {
        let p = persona();
        let auto = Greeting::default();
        assert!(greeting_text(&auto, &p, 9).starts_with("Good morning, Marko."));
        assert!(greeting_text(&auto, &p, 14).starts_with("Good afternoon, Marko."));
        assert!(greeting_text(&auto, &p, 23).starts_with("Good evening, Marko."));
        let custom = Greeting { enabled: true, text: "Pozdravljen!".into() };
        assert_eq!(greeting_text(&custom, &p, 9), "Pozdravljen!");
        // Falls back to the persona name when no address is set.
        let no_addr = Persona { address: String::new(), ..persona() };
        assert!(greeting_text(&auto, &no_addr, 9).starts_with("Good morning, KAgent."));
    }
}
