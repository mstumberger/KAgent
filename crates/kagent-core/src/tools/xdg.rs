//! freedesktop helpers shared by desktop tools: .desktop application lookup, project
//! folders, recently used files (recently-used.xbel), user dirs, URI decoding.

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `~/x` -> `$HOME/x`.
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

/// Directories that hold .desktop files, most specific first.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("applications")];
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let data = if data.is_empty() { "/usr/local/share:/usr/share".to_string() } else { data };
    for d in data.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(d).join("applications"));
    }
    for extra in ["/var/lib/snapd/desktop/applications", "/var/lib/flatpak/exports/share/applications"] {
        dirs.push(PathBuf::from(extra));
    }
    dirs.push(home().join(".local/share/flatpak/exports/share/applications"));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

#[derive(Debug, Clone, PartialEq)]
pub struct DesktopApp {
    /// File stem, e.g. `org.kde.kate`, `code_code`, `vlc`.
    pub id: String,
    pub name: String,
    pub exec: String,
    pub path: PathBuf,
    pub hidden: bool,
}

impl DesktopApp {
    /// First token of `Exec=` (unquoted), the program to run.
    pub fn program(&self) -> String {
        let e = self.exec.trim();
        if let Some(rest) = e.strip_prefix('"') {
            return rest.split('"').next().unwrap_or("").to_string();
        }
        e.split_whitespace().next().unwrap_or("").to_string()
    }
}

/// Parse the `[Desktop Entry]` group of a .desktop file.
pub fn parse_desktop(id: &str, path: &Path, text: &str) -> Option<DesktopApp> {
    let mut in_entry = false;
    let (mut name, mut exec, mut hidden, mut is_app) = (String::new(), String::new(), false, true);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(v) = line.strip_prefix("Name=") {
            name = v.to_string();
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec = v.to_string();
        } else if line == "NoDisplay=true" || line == "Hidden=true" {
            hidden = true;
        } else if let Some(v) = line.strip_prefix("Type=") {
            is_app = v == "Application";
        }
    }
    (is_app && !exec.is_empty()).then(|| DesktopApp { id: id.to_string(), name, exec, path: path.to_path_buf(), hidden })
}

pub fn all_apps() -> Vec<DesktopApp> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in application_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("desktop") {
                continue;
            }
            let id = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
            if !seen.insert(id.clone()) {
                continue; // earlier dir wins (user overrides)
            }
            if let Some(app) = std::fs::read_to_string(&p).ok().and_then(|t| parse_desktop(&id, &p, &t)) {
                out.push(app);
            }
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Best application for a spoken/typed name: exact id, id suffix (`org.kde.kate`,
/// snap `code_code`), exact name, then name/id containing the query. Visible entries
/// win over hidden ones. Aliases cover common spoken names.
pub fn find_app_in(apps: &[DesktopApp], query: &str) -> Option<DesktopApp> {
    // Spoken names that map to a different app per desktop: first installed wins.
    let aliases: &[&str] = match norm(query).as_str() {
        "vscode" | "visualstudiocode" | "code" => &["code"],
        "intellij" | "intellijidea" | "idea" => &["jetbrains-idea"],
        "pycharm" => &["jetbrains-pycharm"],
        "files" | "filemanager" => &["org.kde.dolphin", "org.gnome.Nautilus", "nautilus", "thunar", "nemo", "pcmanfm"],
        "terminal" => &["org.kde.konsole", "org.gnome.Ptyxis", "org.gnome.Terminal", "gnome-terminal", "xfce4-terminal", "kitty", "alacritty"],
        "texteditor" | "editor" => &["org.kde.kate", "org.gnome.TextEditor", "org.gnome.gedit", "gedit"],
        "settings" | "systemsettings" => &["systemsettings", "org.gnome.Settings", "gnome-control-center"],
        _ => &[],
    };
    for a in aliases {
        if let Some(app) = best_match(apps, &norm(a)) {
            return Some(app);
        }
    }
    best_match(apps, &norm(query))
}

fn best_match(apps: &[DesktopApp], q: &str) -> Option<DesktopApp> {
    if q.is_empty() {
        return None;
    }
    let q = q.to_string();
    let score = |a: &DesktopApp| -> u32 {
        let id = norm(&a.id);
        let tail = a.id.rsplit(['.', '_']).next().map(norm).unwrap_or_default();
        let name = norm(&a.name);
        let s = if id == q { 100 } else if tail == q || id.ends_with(&q) && id.len() < q.len() + 12 { 90 } else if name == q { 85 } else if name.starts_with(&q) { 60 } else if name.contains(&q) || id.contains(&q) { 40 } else { 0 };
        if s > 0 && !a.hidden { s + 5 } else { s }
    };
    apps.iter().filter(|a| score(a) > 0).max_by_key(|a| (score(a), std::cmp::Reverse(a.id.len()))).cloned()
}

pub fn find_app(query: &str) -> Option<DesktopApp> {
    find_app_in(&all_apps(), query)
}

/// Is `bin` runnable (absolute path or on PATH)?
pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file()))
}

pub const DEFAULT_PROJECT_ROOTS: [&str; 10] = ["~/SystemInterface", "~/projects", "~/Projects", "~/src", "~/code", "~/dev", "~/git", "~/work", "~/IdeaProjects", "~/PycharmProjects"];
const MARKERS: [&str; 11] = [".git", "Cargo.toml", "package.json", "pyproject.toml", "go.mod", "pom.xml", "build.gradle", "build.gradle.kts", "CMakeLists.txt", "Makefile", "requirements.txt"];

pub fn is_project(dir: &Path) -> bool {
    MARKERS.iter().any(|m| dir.join(m).exists())
}

/// Child folders of the roots that look like projects.
pub fn projects(roots: &[String]) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = if roots.is_empty() { DEFAULT_PROJECT_ROOTS.iter().map(|r| expand(r)).collect() } else { roots.iter().map(|r| expand(r)).collect() };
    let mut out = Vec::new();
    for r in roots {
        let Ok(rd) = std::fs::read_dir(&r) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && !e.file_name().to_string_lossy().starts_with('.') && is_project(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Project folder for a name: exact (case-insensitive), ignoring separators, prefix,
/// then contains. Ties: shortest name.
pub fn find_project(all: &[PathBuf], name: &str) -> Option<PathBuf> {
    let q = norm(name);
    if q.is_empty() {
        return None;
    }
    let base = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let rank = |p: &PathBuf| -> u32 {
        let b = base(p);
        if b.eq_ignore_ascii_case(name.trim()) { 4 } else if norm(&b) == q { 3 } else if norm(&b).starts_with(&q) { 2 } else if norm(&b).contains(&q) { 1 } else { 0 }
    };
    all.iter().filter(|p| rank(p) > 0).max_by_key(|p| (rank(p), std::cmp::Reverse(base(p).len()))).cloned()
}

/// `%20` etc. -> bytes; invalid escapes are kept.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///a%20b` -> `/a b`; other schemes -> None.
pub fn uri_to_path(uri: &str) -> Option<String> {
    uri.strip_prefix("file://").map(percent_decode)
}

/// Recently used local files from recently-used.xbel, newest first: (path, modified).
pub fn parse_recent(xbel: &str, limit: usize) -> Vec<(String, String)> {
    // Attribute order differs between writers (GTK, KDE): read each tag's attributes.
    let tag = regex::Regex::new(r"<bookmark\s[^>]*>").expect("static regex");
    let attr = |t: &str, k: &str| regex::Regex::new(&format!(r#"\s{k}="([^"]*)""#)).ok().and_then(|r| r.captures(t).map(|c| c[1].to_string()));
    let mut v: Vec<(String, String)> = tag
        .find_iter(xbel)
        .filter_map(|m| {
            let t = m.as_str();
            let path = uri_to_path(&attr(t, "href")?.replace("&amp;", "&"))?;
            let when = attr(t, "modified").or_else(|| attr(t, "visited")).unwrap_or_default();
            Some((path, when))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.truncate(limit);
    v
}

/// XDG user dir (`DOWNLOAD`, `VIDEOS`, ...) from user-dirs.dirs, else `~/<fallback>`.
pub fn user_dir(key: &str, fallback: &str) -> PathBuf {
    let cfg = home().join(".config/user-dirs.dirs");
    if let Ok(t) = std::fs::read_to_string(cfg) {
        let k = format!("XDG_{key}_DIR=");
        if let Some(v) = t.lines().find_map(|l| l.trim().strip_prefix(&k)) {
            return PathBuf::from(v.trim_matches('"').replace("$HOME", &home().to_string_lossy()));
        }
    }
    home().join(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, hidden: bool) -> DesktopApp {
        DesktopApp { id: id.into(), name: name.into(), exec: format!("{id} %U"), path: PathBuf::from(format!("/a/{id}.desktop")), hidden }
    }

    #[test]
    fn finds_apps_by_spoken_name() {
        let apps = vec![app("vlc", "VLC media player", false), app("code_code", "Visual Studio Code", false), app("code_code-url-handler", "Visual Studio Code - URL Handler", true), app("org.kde.kate", "Kate", false), app("jetbrains-idea", "IntelliJ IDEA Ultimate", false), app("org.kde.haruna", "Haruna", false)];
        assert_eq!(find_app_in(&apps, "VLC").unwrap().id, "vlc");
        assert_eq!(find_app_in(&apps, "vs code").unwrap().id, "code_code");
        assert_eq!(find_app_in(&apps, "kate").unwrap().id, "org.kde.kate");
        assert_eq!(find_app_in(&apps, "IntelliJ").unwrap().id, "jetbrains-idea");
        assert_eq!(find_app_in(&apps, "haruna").unwrap().id, "org.kde.haruna");
        assert!(find_app_in(&apps, "spotify").is_none());
        let gnome = vec![app("org.gnome.Nautilus", "Files", false), app("org.gnome.Ptyxis", "Terminal", false)];
        assert_eq!(find_app_in(&gnome, "files").unwrap().id, "org.gnome.Nautilus");
        assert_eq!(find_app_in(&gnome, "terminal").unwrap().id, "org.gnome.Ptyxis");
    }

    #[test]
    fn desktop_entry_parsing() {
        let t = "[Desktop Entry]\nType=Application\nName=IDEA\nExec=\"/home/u/idea/bin/idea.sh\" %f\n[Desktop Action new]\nName=New\nExec=other\n";
        let a = parse_desktop("jetbrains-idea", Path::new("/x"), t).unwrap();
        assert_eq!((a.name.as_str(), a.program().as_str()), ("IDEA", "/home/u/idea/bin/idea.sh"));
        assert!(parse_desktop("l", Path::new("/x"), "[Desktop Entry]\nType=Link\nExec=x\n").is_none());
    }

    #[test]
    fn project_lookup() {
        let all = vec![PathBuf::from("/h/SI/ReelGen"), PathBuf::from("/h/SI/KAgent"), PathBuf::from("/h/SI/table_tennis_score"), PathBuf::from("/h/SI/ReelGen-old")];
        assert_eq!(find_project(&all, "reelgen").unwrap(), PathBuf::from("/h/SI/ReelGen"));
        assert_eq!(find_project(&all, "reel gen").unwrap(), PathBuf::from("/h/SI/ReelGen"));
        assert_eq!(find_project(&all, "table tennis").unwrap(), PathBuf::from("/h/SI/table_tennis_score"));
        assert!(find_project(&all, "zzz").is_none());
    }

    #[test]
    fn recent_files_and_uris() {
        let x = r#"<xbel><bookmark added="1" href="file:///home/u/a%20b.mkv" modified="2026-11-20T10:00:00Z" visited="1"></bookmark><bookmark href="https://x" added="1" modified="2026-11-23T10:00:00Z"></bookmark><bookmark href="file:///home/u/new.pdf" added="1" modified="2026-11-24T08:00:00Z"></bookmark></xbel>"#;
        let r = parse_recent(x, 5);
        assert_eq!(r.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), vec!["/home/u/new.pdf", "/home/u/a b.mkv"]);
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%C5%A1"), "š");
    }
}
