//! The protection of KAgent's own configuration (hooks.md section 5). Hooks live in
//! `settings.json` and run with the owner's privileges, so the agent must not write
//! them, nor the files that hold its other rules, without a card. The card is the
//! engine's `destructive` gate: it asks in every mode (`full` control lets `always_ask`
//! through, the finding of S27) and is never remembered.

use std::path::{Component, Path, PathBuf};

/// The files a call may not change without a card, and the folder of the profiles.
struct Protected {
    files: Vec<PathBuf>,
    /// `<config>/agents`: every file in it is a profile the agent runs with.
    agents: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

/// `<config>` and `<data>` (config.md section 1), by the rule of `kagentd/src/paths.rs`:
/// `$KAGENT_HOME` names both folders, else the XDG folders.
pub fn dirs() -> (PathBuf, PathBuf) {
    if let Some(root) = std::env::var_os("KAGENT_HOME") {
        let root = PathBuf::from(root);
        return (root.join("config"), root.join("data"));
    }
    let xdg = |name: &str, fallback: &str| {
        std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
    };
    (xdg("XDG_CONFIG_HOME", ".config").join("kagent"), xdg("XDG_DATA_HOME", ".local/share").join("kagent"))
}

fn protected() -> Protected {
    let (config, data) = dirs();
    Protected {
        files: [config.join("settings.json"), config.join("telegram.json"), data.join("audit.jsonl"), data.join("tasks.json")].map(|p| lexical(&p)).to_vec(),
        agents: lexical(&config.join("agents")),
    }
}

impl Protected {
    /// `path` is one of the files, or inside the folder of the profiles.
    fn holds(&self, path: &Path) -> bool {
        self.files.iter().any(|f| path == f) || path.starts_with(&self.agents)
    }

    /// `holds`, or `path` is a folder above one of them: removing it takes the file too.
    fn holds_or_above(&self, path: &Path) -> bool {
        self.holds(path) || self.files.iter().any(|f| f.starts_with(path))
    }

    /// A name with a wildcard (`settings*`, `kagent/*`) covers one of them.
    fn wildcard_covers(&self, prefix: &Path) -> bool {
        let prefix = prefix.to_string_lossy();
        self.files.iter().chain(std::iter::once(&self.agents)).any(|n| n.to_string_lossy().starts_with(prefix.as_ref()))
    }
}

/// `~`, `$HOME` and `${HOME}` expanded; the quotes and the brackets of the line taken off.
fn expand(text: &str) -> String {
    let text = text.trim_matches(|c| matches!(c, '"' | '\'' | ')' | '}' | ';')).replace('"', "");
    if text == "~" {
        return home().to_string_lossy().to_string();
    }
    let home = home().to_string_lossy().to_string();
    let text = text.replace("${HOME}", &home).replace("$HOME", &home);
    match text.strip_prefix("~/") {
        Some(rest) => Path::new(&home).join(rest).to_string_lossy().to_string(),
        None => text,
    }
}

/// `a/./b` and `a/../b` resolved without touching the disk.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in p.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Where a written path points: relative to `base`.
fn resolve(text: &str, base: &Path) -> PathBuf {
    let p = PathBuf::from(expand(text));
    lexical(&if p.is_absolute() { p } else { base.join(p) })
}

/// Where a written path can point: as written, and through the links that exist now
/// (a folder linked to KAgent's config, a file linked to its settings).
fn forms(text: &str, base: &Path) -> Vec<PathBuf> {
    let written = resolve(text, base);
    let mut out = vec![written.clone()];
    if let (Some(parent), Some(name)) = (written.parent(), written.file_name()) {
        out.extend(std::fs::canonicalize(parent).ok().map(|p| p.join(name)));
    }
    out.extend(std::fs::canonicalize(&written).ok());
    out
}

/// Where a relative path of a shell command starts: the call's `cwd`, else this process's.
fn base(cwd: Option<&str>) -> PathBuf {
    cwd.map(|c| resolve(c, &Path::new("."))).or_else(|| std::env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."))
}

/// Whether a name a command writes names one of the protected files, a folder of them,
/// or (with a wildcard) something that covers one.
fn hits(text: &str, base: &Path, protected: &Protected) -> bool {
    if let Some(i) = text.find(['*', '?']) {
        return protected.wildcard_covers(&resolve(&text[..i], base));
    }
    forms(text, base).iter().any(|p| protected.holds_or_above(p))
}

/// Whether a file the file tools may write is KAgent's own configuration: `settings.json`
/// (the hooks, the rules), a subagent profile, `telegram.json`, the audit log or the automations.
pub fn is_protected(path: &str) -> bool {
    let protected = protected();
    forms(path, &Path::new(".")).iter().any(|p| protected.holds(p))
}

/// Whether a shell command line changes one of those files (hooks.md section 5): a part
/// of it redirects into such a file (`>`, `>>`), edits it in place (`tee`, `sed -i`),
/// removes it or links to it (`rm`, `ln`), moves it or something into its place (`mv`),
/// or copies into it (`cp`). A `cd` in the line is followed, from `cwd`.
pub fn protected_command(command: &str, cwd: Option<&str>) -> bool {
    let protected = protected();
    let mut base = base(cwd);
    for part in command.split([';', '|', '&', '\n']) {
        if part_changes(part, &base, &protected) {
            return true;
        }
        if let Some(dir) = cd_target(part) {
            base = resolve(dir, &base);
        }
    }
    false
}

/// The programs that wrap the one that changes a file.
const WRAPPERS: [&str; 7] = ["sudo", "command", "nohup", "time", "builtin", "exec", "env"];

/// One command of a line (between `;`, `|`, `&&` and a new line): does it change a protected file?
fn part_changes(part: &str, base: &Path, protected: &Protected) -> bool {
    let part = part.trim_start_matches(['(', '{', '!', ' ']);
    let tokens: Vec<&str> = part.split_whitespace().flat_map(|w| split_redirects(w)).collect();
    // The program: past variables set for the call, options of a wrapper, and the wrappers.
    let mut i = 0;
    while let Some(t) = tokens.get(i) {
        let assignment = t.contains('=') && !t.starts_with('-') && !t.contains('/');
        if !(assignment || t.starts_with('-') || WRAPPERS.contains(t)) {
            break;
        }
        i += 1;
    }
    let program = tokens.get(i).and_then(|t| t.rsplit('/').next()).unwrap_or("");
    let args = &tokens[(i + 1).min(tokens.len())..];
    let any = |names: &[&str]| names.contains(&program);
    // A redirect: the file it writes to.
    if tokens.windows(2).any(|w| matches!(w[0], ">" | ">>") && hits(w[1], base, protected)) {
        return true;
    }
    let operands = || args.iter().copied().filter(|a| !a.starts_with('-'));
    let files = || operands().any(|a| hits(a, base, protected));
    let inplace = args.iter().any(|a| *a == "-i" || a.starts_with("--in-place") || (a.starts_with('-') && a.contains('i')));
    if any(&["rm", "rmdir", "tee", "ln"]) {
        files()
    } else if program == "sed" {
        inplace && files()
    } else if any(&["mv", "cp"]) {
        // What it moves or copies into: the last operand, or `-t` / `--target-directory`.
        let target = args
            .iter()
            .position(|a| *a == "-t" || *a == "--target-directory")
            .and_then(|j| args.get(j + 1))
            .copied()
            .or_else(|| args.iter().find_map(|a| a.strip_prefix("--target-directory=")));
        target.is_some_and(|t| hits(t, base, protected)) || operands().last().is_some_and(|a| hits(a, base, protected)) || (program == "mv" && files())
    } else {
        false
    }
}

/// `a>b` and `2>>c` are three words to the shell: the pieces up to a redirect.
fn split_redirects(word: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = word;
    while let Some(i) = rest.find('>') {
        let op = if rest[i..].starts_with(">>") { 2 } else { 1 };
        if i > 0 {
            out.push(&rest[..i]);
        }
        out.push(&rest[i..i + op]);
        rest = &rest[i + op..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// `cd` at the start of a command: the folder it names, or the home folder when alone.
fn cd_target(part: &str) -> Option<&str> {
    let part = part.trim_start_matches(['(', '{', '!', ' ']);
    if part == "cd" {
        return Some("~");
    }
    let rest = part.strip_prefix("cd")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim();
    (!rest.is_empty() && !rest.starts_with('-')).then_some(rest)
}

#[cfg(test)]
mod tests;
