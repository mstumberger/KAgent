//! The reader's browser process (web.md section 8): headless, on the reader's profile,
//! in a systemd scope of its own when the daemon runs under systemd; or visible, for the
//! owner's window (web.md section 9). Nothing here hides that it is automated.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::{Child, Command};

use crate::appwin::scope;
use crate::browser::cdp;

/// `<data>/reader-profile`: the reader's own. Never the owner's browser profile, never
/// the `browser` tool's `<data>/browser-profile` and never the app window's.
pub fn profile_dir(data: &Path) -> PathBuf {
    data.join("reader-profile")
}

/// The arguments of the browser. No flag here changes what the page can tell about the
/// browser (web.md section 10): they are about the profile, the first-run pages, the
/// keyring and the sound.
pub fn args(profile: &Path, headless: bool, url: Option<&str>) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--password-store=basic".into(),
        "--mute-audio".into(),
    ];
    if headless {
        args.extend(["--headless=new".into(), "--remote-debugging-port=0".into(), "--remote-debugging-address=127.0.0.1".into(), "--disable-extensions".into()]);
    } else {
        args.push("--class=KAgentReader".into());
    }
    args.push(url.unwrap_or("about:blank").to_string());
    args
}

/// A running browser the reader drives.
pub struct Running {
    pub child: Child,
    pub port: u16,
}

impl Running {
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Ask the browser to close (it writes its cookies), then end it if it does not.
    pub async fn close(mut self) {
        if let Ok(version) = cdp::version(self.port).await {
            if let Some(url) = version.get("webSocketDebuggerUrl").and_then(|u| u.as_str()) {
                let tab = cdp::Tab { id: "browser".into(), kind: "browser".into(), title: String::new(), url: String::new(), web_socket_debugger_url: url.to_string() };
                if let Ok(mut session) = cdp::Session::connect(&tab).await {
                    let _ = session.call("Browser.close", serde_json::json!({})).await;
                }
            }
        }
        if tokio::time::timeout(Duration::from_secs(3), self.child.wait()).await.is_err() {
            let _ = self.child.kill().await;
        }
    }
}

fn command(bin: &str, args: Vec<String>, description: &str) -> Command {
    let mut argv = vec![bin.to_string()];
    argv.extend(args);
    let argv = scope::in_scope_named(argv, scope::under_systemd(), kagent_core::tools::xdg::which("systemd-run").as_deref(), description);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd
}

/// Start the headless browser and wait for its DevTools port (`DevToolsActivePort` of the profile).
pub async fn start_headless(bin: &str, profile: &Path) -> Result<Running, String> {
    std::fs::create_dir_all(profile).map_err(|e| format!("the profile folder could not be made: {e}"))?;
    let port_file = profile.join("DevToolsActivePort");
    let _ = std::fs::remove_file(&port_file);
    let mut cmd = command(bin, args(profile, true, None), "KAgent reader");
    cmd.kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("{bin} did not start: {e}"))?;
    for _ in 0..150 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("the browser ended at once ({status}); is another instance using the reader's profile?"));
        }
        let port = std::fs::read_to_string(&port_file).ok().and_then(|text| text.lines().next().and_then(|l| l.trim().parse::<u16>().ok()));
        if let Some(port) = port {
            if cdp::version(port).await.is_ok() {
                return Ok(Running { child, port });
            }
        }
    }
    let _ = child.kill().await;
    Err("the browser started but its DevTools port did not open".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_is_the_readers_own_and_no_flag_hides_automation() {
        let data = Path::new("/home/owner/.local/share/kagent");
        let profile = profile_dir(data);
        assert_eq!(profile, data.join("reader-profile"));
        assert_ne!(profile, data.join("browser-profile"), "not the browser tool's");
        for owner in ["BraveSoftware", ".config/google-chrome", ".config/chromium", "chrome-profile", "app-profile"] {
            assert!(!profile.to_string_lossy().contains(owner), "{owner}");
        }
        let headless = args(&profile, true, None);
        assert!(headless.contains(&format!("--user-data-dir={}", profile.display())) && headless.contains(&"--headless=new".to_string()));
        let window = args(&profile, false, Some("https://example.com/"));
        assert!(!window.iter().any(|a| a.contains("headless")) && window.last().unwrap() == "https://example.com/");
        for flag in headless.iter().chain(&window) {
            let lower = flag.to_lowercase();
            for stealth in ["automationcontrolled", "user-agent", "disable-blink", "enable-automation", "disable-infobars", "stealth", "fingerprint"] {
                assert!(!lower.contains(stealth), "{flag}");
            }
        }
    }
}
