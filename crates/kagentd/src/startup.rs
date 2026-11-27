//! Startup behaviour (settings.startup): the systemd user unit for autostart and
//! the greeting notification. Everything here is user-level - no sudo.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::Persona;

const UNIT: &str = "kagentd.service";

/// settings.startup: autostart with the session + greeting on daemon start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Startup {
    /// Keep the systemd user unit kagentd.service enabled (starts at login).
    #[serde(default = "default_true")]
    pub autostart: bool,
    #[serde(default)]
    pub greeting: Greeting,
}

impl Default for Startup {
    fn default() -> Self {
        Self { autostart: true, greeting: Greeting::default() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Greeting {
    /// Desktop notification greeting once the daemon is up.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Empty = auto ("Good morning/afternoon/evening, <address>. ...").
    #[serde(default)]
    pub text: String,
}

impl Default for Greeting {
    fn default() -> Self {
        Self { enabled: true, text: String::new() }
    }
}

fn default_true() -> bool {
    true
}

fn unit_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/systemd/user")
        .join(UNIT)
}

fn running_exe() -> Option<PathBuf> {
    std::env::current_exe().ok().map(|p| p.canonicalize().unwrap_or(p))
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

/// Whether a daemon start may rewrite the user unit (foundations.md section 7).
/// Never from a `KAGENT_HOME` instance. A binary outside a `target` directory always
/// may. One inside a `target` directory only when the existing unit already points
/// into that same `target`: a unit that points to a release binary is never replaced
/// by it. So a debug or smoke-test start cannot repoint the owner's autostart.
pub fn should_rewrite_unit(home_set: bool, exe: Option<&Path>, unit_exe: Option<&Path>) -> bool {
    if home_set {
        return false;
    }
    match exe.and_then(target_dir) {
        None => true,
        Some(target) => unit_exe.and_then(target_dir) == Some(target),
    }
}

/// The `target` directory the path is inside, if any.
fn target_dir(path: &Path) -> Option<&Path> {
    path.ancestors().skip(1).find(|a| a.file_name().is_some_and(|n| n == "target"))
}

/// The binary the existing unit's `ExecStart=` names, if there is a unit.
fn unit_exec_start() -> Option<PathBuf> {
    let text = std::fs::read_to_string(unit_path()).ok()?;
    let path = exec_start_binary(&text)?;
    Some(path.canonicalize().unwrap_or(path))
}

/// `ExecStart=` of a unit text: the binary, without systemd's prefixes and quotes.
/// A unit that cannot be read this way gives `None`, which keeps a start from
/// rewriting it.
fn exec_start_binary(unit: &str) -> Option<PathBuf> {
    let line = unit.lines().map(str::trim).find(|l| l.starts_with("ExecStart="))?;
    let command = line["ExecStart=".len()..].trim_start().trim_start_matches(['@', '-', '+', '!']).trim_start();
    let binary = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"')?.0,
        None => command.split_whitespace().next()?,
    };
    Some(PathBuf::from(binary))
}

/// Daemon start: bring the unit in step with the setting, rewriting it only when
/// this start may (`should_rewrite_unit`).
pub fn sync_at_start(enabled: bool) {
    if !enabled {
        apply_autostart(false);
        return;
    }
    let may = should_rewrite_unit(std::env::var_os("KAGENT_HOME").is_some(), running_exe().as_deref(), unit_exec_start().as_deref());
    if may {
        apply_autostart(true);
    } else {
        tracing::info!("autostart: {} left as it is (KAGENT_HOME is set, or this binary is under target/ and the unit points elsewhere)", UNIT);
    }
}

/// `PUT /api/settings {startup.autostart}`: an explicit owner action, so it applies
/// from any binary. Not from a `KAGENT_HOME` instance (tests, a second instance),
/// which has no business with the owner's unit.
pub fn apply_requested(enabled: bool) {
    if std::env::var_os("KAGENT_HOME").is_some() {
        tracing::info!("KAGENT_HOME is set: autostart saved, {} left as it is", UNIT);
        return;
    }
    apply_autostart(enabled);
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
    let Some(exe) = running_exe() else {
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

/// The end of the greeting while the owner's name is not known.
pub const ASK_NAME: &str = "What should I call you? Tell me, or set it in Settings -> General.";

/// Greeting text: custom text if set, else time-of-day + persona address.
pub fn greeting_text(greeting: &Greeting, persona: &Persona, hour: u32) -> String {
    if !greeting.text.trim().is_empty() {
        return greeting.text.clone();
    }
    let part = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    // No name yet (voice-control.md section 5): the greeting asks for it.
    if persona.address.trim().is_empty() {
        return format!("{part}. KAgent is up and watching. {ASK_NAME}");
    }
    format!("{part}, {}. KAgent is up and watching.", persona.address)
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
        // No address: no name in the greeting, and it asks for one (voice-control.md section 5).
        let no_addr = Persona { address: String::new(), ..persona() };
        assert_eq!(greeting_text(&auto, &no_addr, 9), "Good morning. KAgent is up and watching. What should I call you? Tell me, or set it in Settings -> General.");
        assert_eq!(greeting_text(&custom, &no_addr, 9), "Pozdravljen!", "a custom text is the owner's own words");
    }

    #[test]
    fn guard_table() {
        let release = PathBuf::from("/home/me/KAgent/target/release/kagentd");
        let debug = PathBuf::from("/home/me/KAgent/target/debug/kagentd");
        let installed = PathBuf::from("/home/me/.local/bin/kagentd");
        let other_target = PathBuf::from("/opt/other/target/release/kagentd");
        // (KAGENT_HOME set, running binary, binary the unit points to, may rewrite)
        for (home_set, exe, unit_exe, want) in [
            (true, &debug, Some(&debug), false),
            (true, &installed, None, false),
            (false, &installed, Some(&debug), true),
            (false, &installed, None, true),
            (false, &debug, None, false),
            (false, &debug, Some(&installed), false),
            (false, &debug, Some(&debug), true),
            (false, &debug, Some(&release), true),
            (false, &debug, Some(&other_target), false),
        ] {
            assert_eq!(should_rewrite_unit(home_set, Some(exe), unit_exe.map(PathBuf::as_path)), want, "home set {home_set}, {exe:?}, unit {unit_exe:?}");
        }
    }

    #[test]
    fn exec_start_binary_of_a_unit() {
        let written = format!(
            "# Managed by kagentd\n[Service]\nWorkingDirectory={}\nExecStart={} serve\nRestart=on-failure\n",
            Path::new("/w").display(),
            Path::new("/opt/kagent/bin/kagentd").display()
        );
        assert_eq!(exec_start_binary(&written), Some(PathBuf::from("/opt/kagent/bin/kagentd")));
        assert_eq!(exec_start_binary("ExecStart=-\"/opt/my kagent/kagentd\" serve\n"), Some(PathBuf::from("/opt/my kagent/kagentd")));
        assert_eq!(exec_start_binary("ExecStart=\"unterminated serve\n"), None, "unreadable: nothing is rewritten");
        assert_eq!(exec_start_binary("[Service]\nExecReload=/bin/true\n"), None, "only ExecStart counts");
    }
}
