//! Change tools (docs/specs/tools.md sections 4 and 8): package, power_profile,
//! write_file, edit_file, measure, undo_change, list_changes. Every mutation goes
//! through the change journal so it can be listed and undone.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::exec::exec;
use super::privop::{apply_root, query_root, record_user};
use super::parse;
use crate::agent::hooks::protect;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

/// Canonical path for a (possibly new) file under $HOME; `None` when outside.
fn home_path(p: &str) -> Option<PathBuf> {
    let path = expand(p);
    let parent = path.parent()?.canonicalize().ok()?;
    let full = parent.join(path.file_name()?);
    let home = home().canonicalize().ok()?;
    (full.starts_with(&home) && full != home).then_some(full)
}

/// Same allowlist as the root helper's write_file (priv-ops.md).
fn root_editable(p: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(/etc/sysctl\.d/99-kagent\.conf|/etc/modprobe\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/systemd/system/[A-Za-z0-9@_.:-]{1,128}\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/systemd/journald\.conf\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/NetworkManager/conf\.d/kagent-[A-Za-z0-9_-]{1,64}\.conf|/etc/udev/rules\.d/90-kagent-[A-Za-z0-9_-]{1,64}\.rules)$").unwrap()
    })
    .is_match(p)
}

fn backup_file(ctx: &ToolContext, content: &str) -> anyhow::Result<PathBuf> {
    let dir = ctx.services()?.journal.dir().to_path_buf();
    let p = dir.join(format!("bak_{}", uuid::Uuid::new_v4().simple()));
    std::fs::write(&p, content)?;
    Ok(p)
}

/// Minimal line diff of one replacement, for approval cards and results.
pub fn replacement_diff(content: &str, old: &str, new: &str) -> String {
    let start = content.find(old).map(|i| content[..i].matches('\n').count() + 1).unwrap_or(1);
    let mut out = format!("@@ line {start} @@\n");
    for l in old.lines() {
        out.push_str(&format!("-{l}\n"));
    }
    for l in new.lines() {
        out.push_str(&format!("+{l}\n"));
    }
    out
}

// ---------------------------------------------------------------- package

pub struct Package {
    contract: ToolContract,
}

impl Package {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "package",
                "Packages (apt, dnf or pacman, whatever this distro uses): search or show info (read-only), or install/remove through the root helper. The approval card shows the package manager's dry run first; installs are undoable by removing what was added, removals by reinstalling.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["search","info","install","remove"]},
                    "name":{"type":"string","description":"search term or package for info"},
                    "packages":{"type":"array","items":{"type":"string"},"description":"install/remove: package names (max 20)"}
                }}),
            )
            .mutating(SideEffect::System, Risk::High)
            .privileged()
            .timeout(600_000, 900_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct PackageArgs {
    action: Option<String>,
    name: Option<String>,
    packages: Option<Vec<String>>,
}

use crate::platform::packages::valid_name as valid_pkg;

/// Dry run without root where the package manager allows it.
fn local_simulation(pm: crate::platform::PackageManager, action: &str, pkgs: &[String]) -> Option<Vec<String>> {
    use crate::platform::PackageManager as P;
    let mut argv: Vec<String> = match (pm, action) {
        (P::Apt, _) => vec!["apt-get".into(), "-s".into(), action.into()],
        (P::Pacman, "install") => vec!["pacman".into(), "-S".into(), "--needed".into(), "--print".into(), "--print-format".into(), "%n %v".into()],
        (P::Pacman, _) => vec!["pacman".into(), "-R".into(), "--print".into(), "--print-format".into(), "%n %v".into()],
        _ => return None, // dnf needs root even to resolve
    };
    argv.push("--".into());
    argv.extend(pkgs.iter().cloned());
    Some(argv)
}

#[async_trait]
impl Tool for Package {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let pm = crate::platform::package_manager().name();
        let action = input.get("action").and_then(Value::as_str).unwrap_or("search");
        let pkgs = input.get("packages").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")).unwrap_or_default();
        let summary = match action {
            "install" | "remove" => format!("{pm} {action} {pkgs}"),
            _ => format!("{pm} {action} {}", input.get("name").and_then(Value::as_str).unwrap_or("")),
        };
        let mut a = CallAssessment::from_contract(&self.contract, summary.clone(), summary);
        if action == "search" || action == "info" {
            a = a.as_read_only();
        }
        a
    }
    async fn preview(&self, input: &Value, ctx: &ToolContext) -> Option<String> {
        let action = input.get("action").and_then(Value::as_str)?;
        let pkgs: Vec<String> = input.get("packages")?.as_array()?.iter().filter_map(Value::as_str).filter(|p| valid_pkg(p)).map(String::from).collect();
        if pkgs.is_empty() || !(action == "install" || action == "remove") {
            return None;
        }
        // The helper's dry run is the same code path as the real change.
        if let Ok(r) = super::privop::query_root(ctx, &json!({"op": "package", "action": action, "packages": pkgs, "dryRun": true})).await {
            if r.ok || !r.stdout.is_empty() {
                return Some(if r.stdout.trim().is_empty() { r.stderr } else { r.stdout });
            }
        }
        let pm = crate::platform::package_manager();
        let Some(argv) = local_simulation(pm, action, &pkgs) else {
            return Some(format!("{} {action} {} (dry run needs the root helper)", pm.name(), pkgs.join(" ")));
        };
        let r = exec(ctx, &argv).run().await.ok()?;
        let lines: Vec<&str> = r.stdout.lines().filter(|l| pm != crate::platform::PackageManager::Apt || l.starts_with("Inst ") || l.starts_with("Remv ") || l.contains("newly installed") || l.contains("to remove")).collect();
        Some(if lines.is_empty() { r.render() } else { lines.join("\n") })
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: PackageArgs = parse(input)?;
        let pm = crate::platform::package_manager();
        if pm == crate::platform::PackageManager::Unknown {
            return Ok(ToolOutput::error("no supported package manager (apt, dnf, pacman) found"));
        }
        match args.action.as_deref() {
            Some("search") => {
                let Some(name) = args.name.filter(|n| !n.trim().is_empty() && !n.trim().starts_with('-') && n.len() <= 100) else { return Ok(ToolOutput::error("search needs a name")) };
                let argv = crate::platform::packages::search_argv(pm, name.trim()).unwrap_or_default();
                let r = exec(&ctx, &argv).run().await?;
                Ok(search_reply(&r))
            }
            Some("info") => {
                let Some(name) = args.name.filter(|n| valid_pkg(n)) else { return Ok(ToolOutput::error("info needs a valid package name")) };
                let mut out = String::new();
                for argv in crate::platform::packages::info_argv(pm, &name) {
                    let r = exec(&ctx, &argv).run().await?;
                    if r.stdout.trim().is_empty() {
                        continue;
                    }
                    out.push_str(&format!("## {}\n{}\n{}", argv.join(" "), r.stdout.lines().take(30).collect::<Vec<_>>().join("\n"), crate::util::more_note(r.stdout.lines().count(), 30, "`shell` runs the same command for the rest")));
                }
                Ok(ToolOutput::ok(if out.is_empty() { format!("no package named {name}") } else { out }))
            }
            Some(action @ ("install" | "remove")) => {
                let pkgs = args.packages.unwrap_or_default();
                if pkgs.is_empty() || pkgs.len() > 20 || !pkgs.iter().all(|p| valid_pkg(p)) {
                    return Ok(ToolOutput::error("packages: 1-20 valid names required"));
                }
                let summary = format!("{} {action} {}", pm.name(), pkgs.join(" "));
                apply_root(&ctx, "package", &summary, json!({"op": "package", "action": action, "packages": pkgs})).await
            }
            _ => Ok(ToolOutput::error("action must be search, info, install or remove")),
        }
    }
}

// ---------------------------------------------------------------- power_profile

pub struct PowerProfile {
    contract: ToolContract,
}

impl PowerProfile {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "power_profile",
                "Get or set the power profile (power-profiles-daemon): power-saver, balanced, performance. No root needed; undoable.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["get","set"]},
                    "profile":{"type":"string","enum":["power-saver","balanced","performance"]}
                }}),
            )
            .mutating(SideEffect::System, Risk::Low),
        }
    }
}

#[async_trait]
impl Tool for PowerProfile {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("get");
        let profile = input.get("profile").and_then(Value::as_str).unwrap_or("");
        let summary = format!("power profile {action} {profile}");
        let a = CallAssessment::from_contract(&self.contract, summary.trim().to_string(), format!("power_profile {action}"));
        if action == "get" { a.as_read_only() } else { a }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let current = exec(&ctx, &["powerprofilesctl", "get"]).run().await?.stdout.trim().to_string();
        match input.get("action").and_then(Value::as_str) {
            Some("set") => {
                let Some(p) = input.get("profile").and_then(Value::as_str).filter(|p| ["power-saver", "balanced", "performance"].contains(p)) else {
                    return Ok(ToolOutput::error("profile must be power-saver, balanced or performance"));
                };
                let r = exec(&ctx, &["powerprofilesctl", "set", p]).run().await?;
                let after = exec(&ctx, &["powerprofilesctl", "get"]).run().await?.stdout.trim().to_string();
                let undo = (!current.is_empty() && current != p).then(|| json!({"kind": "user", "action": "command", "argv": ["powerprofilesctl", "set", current]}));
                record_user(&ctx, "power_profile", &format!("power profile -> {p}"), json!({"kind": "user", "action": "command", "argv": ["powerprofilesctl", "set", p]}), r.success(), undo, Some(current), Some(after), &r.render())
            }
            _ => Ok(ToolOutput::ok(format!("current power profile: {current}"))),
        }
    }
}

// ---------------------------------------------------------------- write_file / edit_file

pub struct WriteFile {
    contract: ToolContract,
}

impl WriteFile {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "write_file",
                "Write a text file under the user's home (overwrite, append or create). The previous content is backed up so the change can be undone. For system config files use edit_file on an allowed kagent-* path.",
                json!({"type":"object","required":["path","content"],"properties":{
                    "path":{"type":"string","description":"~/... or an absolute path inside $HOME"},
                    "content":{"type":"string"},
                    "mode":{"type":"string","enum":["overwrite","append","create"],"description":"default overwrite; create fails if the file exists"}
                }}),
            )
            .mutating(SideEffect::Workspace, Risk::Medium),
        }
    }
}

#[async_trait]
impl Tool for WriteFile {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let path = input.get("path").and_then(Value::as_str).unwrap_or("?");
        let canon = home_path(path).map(|p| p.display().to_string()).unwrap_or_else(|| path.to_string());
        let mut a = CallAssessment::from_contract(&self.contract, format!("write {canon}"), format!("write {canon}"));
        // KAgent's own configuration (hooks.md section 5): a card in every mode, never rememberable.
        if protect::is_protected(path) {
            a = a.destructive();
        }
        a
    }
    async fn preview(&self, input: &Value, _ctx: &ToolContext) -> Option<String> {
        let path = home_path(input.get("path")?.as_str()?)?;
        let content = input.get("content")?.as_str()?;
        let existing = tokio::fs::metadata(&path).await.ok().map(|m| m.len());
        let head: String = content.lines().take(15).collect::<Vec<_>>().join("\n");
        Some(format!(
            "{} {} ({} bytes{})\n---\n{head}",
            input.get("mode").and_then(Value::as_str).unwrap_or("overwrite"),
            path.display(),
            content.len(),
            existing.map(|n| format!(", replaces {n} bytes")).unwrap_or_else(|| ", new file".into())
        ))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path_s = input.get("path").and_then(Value::as_str).unwrap_or_default();
        let content = input.get("content").and_then(Value::as_str).unwrap_or_default();
        let mode = input.get("mode").and_then(Value::as_str).unwrap_or("overwrite");
        let Some(path) = home_path(path_s) else {
            return Ok(ToolOutput::error(format!("{path_s} is not inside your home directory (or its folder does not exist). System files: use edit_file on an allowed kagent-* config path.")));
        };
        if mode != "create" && super::seen::stale(&ctx.session_id, &path.display().to_string()) {
            return Ok(changed_on_disk(&path.display().to_string()));
        }
        let prev = tokio::fs::read_to_string(&path).await.ok();
        if mode == "create" && prev.is_some() {
            return Ok(ToolOutput::error(format!("{} already exists (mode create)", path.display())));
        }
        let backup = match &prev {
            Some(p) => Some(backup_file(&ctx, p)?),
            None => None,
        };
        let new_content = match (mode, &prev) {
            ("append", Some(p)) => format!("{p}{content}"),
            _ => content.to_string(),
        };
        tokio::fs::write(&path, &new_content).await?;
        super::seen::remember(&ctx.session_id, &path.display().to_string());
        let undo = json!({"kind": "user", "action": "restore_file", "path": path, "backup": backup});
        record_user(&ctx, "write_file", &format!("{mode} {}", path.display()), json!({"kind": "user", "action": "write_file", "path": path, "bytes": new_content.len()}), true, Some(undo), prev.map(|p| format!("{} bytes", p.len())), Some(format!("{} bytes", new_content.len())), "")
    }
}

pub struct EditFile {
    contract: ToolContract,
}

impl EditFile {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "edit_file",
                "Replace one exact text snippet in a file (must occur exactly once). Works on files in $HOME, and on system config files KAgent manages (/etc/sysctl.d/99-kagent.conf, /etc/modprobe.d/kagent-*.conf, /etc/systemd/system/*.d/kagent-*.conf, ...) through the root helper. Undoable.",
                json!({"type":"object","required":["path","old","new"],"properties":{
                    "path":{"type":"string"},
                    "old":{"type":"string","description":"exact existing text, including whitespace"},
                    "new":{"type":"string","description":"replacement text"}
                }}),
            )
            .mutating(SideEffect::Workspace, Risk::Medium),
        }
    }
}

async fn read_any(ctx: &ToolContext, path: &str) -> anyhow::Result<String> {
    if root_editable(path) {
        let r = query_root(ctx, &json!({"op": "read_root", "path": path, "tailLines": 2000})).await?;
        if !r.ok {
            anyhow::bail!("cannot read {path}: {}", r.message.unwrap_or_default());
        }
        return Ok(r.stdout);
    }
    let p = home_path(path).ok_or_else(|| anyhow::anyhow!("{path} is neither in $HOME nor an allowed kagent-* system config path"))?;
    Ok(tokio::fs::read_to_string(p).await?)
}

#[async_trait]
impl Tool for EditFile {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let path = input.get("path").and_then(Value::as_str).unwrap_or("?");
        let mut a = CallAssessment::from_contract(&self.contract, format!("edit {path}"), format!("edit {path}"));
        if root_editable(path) {
            a.privileged = true;
            a.always_ask = true;
            a.risk = Risk::High;
        }
        // KAgent's own configuration (hooks.md section 5): a card in every mode, never rememberable.
        if protect::is_protected(path) {
            a = a.destructive();
        }
        a
    }
    async fn preview(&self, input: &Value, ctx: &ToolContext) -> Option<String> {
        let path = input.get("path")?.as_str()?;
        let content = read_any(ctx, path).await.ok()?;
        Some(replacement_diff(&content, input.get("old")?.as_str()?, input.get("new")?.as_str()?))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let path = input.get("path").and_then(Value::as_str).unwrap_or_default().to_string();
        let old = input.get("old").and_then(Value::as_str).unwrap_or_default();
        let new = input.get("new").and_then(Value::as_str).unwrap_or_default();
        if old.is_empty() {
            return Ok(ToolOutput::error("old must not be empty"));
        }
        let content = match read_any(&ctx, &path).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::error(format!("{e:#}"))),
        };
        if !root_editable(&path) && home_path(&path).is_some_and(|p| super::seen::stale(&ctx.session_id, &p.display().to_string())) {
            return Ok(changed_on_disk(&path));
        }
        let count = content.matches(old).count();
        if count != 1 {
            return Ok(ToolOutput::error(format!("`old` occurs {count} times in {path}; it must occur exactly once. Include more surrounding text.")));
        }
        let updated = content.replacen(old, new, 1);
        let diff = replacement_diff(&content, old, new);
        if root_editable(&path) {
            let mut out = apply_root(&ctx, "edit_file", &format!("edit {path}"), json!({"op": "write_file", "path": path, "content": updated, "mode": "0644"})).await?;
            out.content.push_str(&diff);
            return Ok(out);
        }
        let p = home_path(&path).expect("checked by read_any");
        let backup = backup_file(&ctx, &content)?;
        tokio::fs::write(&p, &updated).await?;
        super::seen::remember(&ctx.session_id, &p.display().to_string());
        let undo = json!({"kind": "user", "action": "restore_file", "path": p, "backup": backup});
        record_user(&ctx, "edit_file", &format!("edit {}", p.display()), json!({"kind": "user", "action": "edit_file", "path": p}), true, Some(undo), None, None, &diff)
    }
}

/// A file the session read has changed since: nothing is written (agent-loop.md C.2).
fn changed_on_disk(path: &str) -> ToolOutput {
    ToolOutput::not_done(&format!("{path} changed on disk since you read it, so nothing was written"), "read it again with read_file, then make the change")
}

/// What `package search` answers: a failed search is not "no packages found" (agent-loop.md B.2, question 1)
/// and a cut list says so (question 4).
fn search_reply(r: &super::exec::ExecResult) -> ToolOutput {
    let all = r.stdout.lines().count();
    let lines: Vec<&str> = r.stdout.lines().take(40).collect();
    if lines.is_empty() && !r.success() {
        return ToolOutput::error(format!("the package search failed, so this is not \"no packages found\": {}", r.render()));
    }
    if lines.is_empty() {
        return ToolOutput::ok("no packages found");
    }
    ToolOutput::ok(format!("{}\n{}", lines.join("\n"), crate::util::more_note(all, 40, "search for a longer name")).trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(code: i32, stdout: &str, stderr: &str) -> super::super::exec::ExecResult {
        super::super::exec::ExecResult { code: Some(code), stdout: stdout.into(), stderr: stderr.into(), timed_out: false }
    }

    #[test]
    fn package_search_says_failed_and_says_cut() {
        let none = search_reply(&result(0, "", ""));
        assert_eq!((none.is_error, none.content.as_str()), (false, "no packages found"));
        let failed = search_reply(&result(100, "", "E: The repository is not signed"));
        assert!(failed.is_error && failed.content.contains("the package search failed, so this is not \"no packages found\"") && failed.content.contains("not signed"), "{}", failed.content);
        let many = (1..=45).map(|i| format!("pkg{i}")).collect::<Vec<_>>().join("\n");
        let cut = search_reply(&result(0, &many, ""));
        assert!(cut.content.ends_with("pkg40\n... 5 more not shown (search for a longer name)"), "{}", cut.content);
        assert!(!search_reply(&result(0, "pkg1\n", "")).content.contains("more not shown"));
    }

    #[test]
    fn root_allowlist_and_pkg_names() {
        assert!(root_editable("/etc/sysctl.d/99-kagent.conf"));
        assert!(root_editable("/etc/systemd/system/docker.service.d/kagent-limits.conf"));
        assert!(!root_editable("/etc/passwd"));
        assert!(!root_editable("/etc/modprobe.d/kagent-x.conf/../../passwd"));
        assert!(valid_pkg("htop") && valid_pkg("g++-13") && !valid_pkg("-o=x") && valid_pkg("NetworkManager") && !valid_pkg("a b"));
    }

    #[test]
    fn diff_shows_line_and_changes() {
        let d = replacement_diff("a\nb\nc\n", "b", "B");
        assert!(d.starts_with("@@ line 2 @@") && d.contains("-b\n+B"), "{d}");
    }

    #[test]
    fn home_paths_only() {
        let home = std::env::var("HOME").unwrap();
        assert!(home_path("~/x-kagent-test.txt").is_some());
        assert!(home_path(&format!("{home}/../../etc/passwd")).is_none());
        assert!(home_path("/etc/passwd").is_none());
    }

    /// agent-loop.md C.2: a file that changed on disk since the session read it is not written over.
    #[tokio::test]
    async fn a_file_changed_since_it_was_read_is_not_edited_or_overwritten() {
        let dir = tempfile::tempdir_in(home()).unwrap();
        let file = dir.path().join("config.txt");
        std::fs::write(&file, "alpha beta").unwrap();
        let path = file.display().to_string();
        let mut ctx = ToolContext::standalone(std::time::Duration::from_secs(5), 10_000);
        ctx.session_id = "change-test-stale".into();
        super::super::seen::remember(&ctx.session_id, &path);
        // Someone else changes it.
        std::fs::write(&file, "alpha gamma").unwrap();
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(30)).unwrap();
        let expected = format!("not done: {path} changed on disk since you read it, so nothing was written. Next: read it again with read_file, then make the change.");
        let edit = EditFile::new().call(json!({"path": path, "old": "alpha", "new": "omega"}), ctx.clone()).await.unwrap();
        assert_eq!((edit.kind, edit.content.as_str()), (crate::tool::ResultKind::NotDone, expected.as_str()));
        let write = WriteFile::new().call(json!({"path": path, "content": "x"}), ctx.clone()).await.unwrap();
        assert_eq!(write.content, expected);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alpha gamma", "nothing was written");
        // A file the session never read is not checked.
        let other = dir.path().join("other.txt");
        std::fs::write(&other, "x").unwrap();
        assert!(!super::super::seen::stale(&ctx.session_id, &other.display().to_string()));
    }
}
