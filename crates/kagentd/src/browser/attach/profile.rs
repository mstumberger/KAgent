//! Where the owner's browser is: its user data directory (the profile folder), and
//! the `DevToolsActivePort` file it writes there once remote debugging is allowed
//! (browser.md sections 1 and 2).

use std::path::{Path, PathBuf};

/// The profile folders of `browser.attachProfile` when it is empty, in order.
pub const PROFILES: [(&str, &str); 4] = [("Brave", "BraveSoftware/Brave-Browser"), ("Chrome", "google-chrome"), ("Edge", "microsoft-edge"), ("Chromium", "chromium")];

/// Lines 1 and 2 of `DevToolsActivePort`: the port and the browser's WebSocket path.
/// `None` unless the port is 1..=65535 and the path lies under `/devtools/browser/`.
pub fn parse_devtools_active_port(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse::<u16>().ok().filter(|p| *p > 0)?;
    let path = lines.next()?.trim_end();
    let id = path.strip_prefix("/devtools/browser/")?;
    (!id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c.is_control())).then(|| (port, path.to_string()))
}

/// The folder `DevToolsActivePort` is read from, and the browser's name: the configured
/// folder when there is one (it must exist), else the first of `PROFILES` under `config`.
pub fn find_profile(home: &Path, config: &Path, configured: &str) -> Option<(PathBuf, String)> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let dir = expand_home(home, configured);
        return dir.is_dir().then(|| (dir.clone(), browser_name(&dir)));
    }
    PROFILES.iter().find_map(|(name, rel)| {
        let dir = config.join(rel);
        dir.is_dir().then(|| (dir, (*name).to_string()))
    })
}

/// `~` and `~/x` name the home directory; anything else is taken as it is.
fn expand_home(home: &Path, path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home.to_path_buf(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

/// The browser a profile folder belongs to: by its last folders, else by its name.
fn browser_name(dir: &Path) -> String {
    if let Some((name, _)) = PROFILES.iter().find(|(_, rel)| dir.ends_with(rel)) {
        return (*name).to_string();
    }
    dir.file_name().and_then(|n| n.to_str()).unwrap_or("browser").to_string()
}

