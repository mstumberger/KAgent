//! File tools: read_file, list_dir, search_files (tools.md section 4).

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use super::exec::exec;
use super::parse;
use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput};

fn expand(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home).join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------- read_file

pub struct ReadFile {
    contract: ToolContract,
    /// Whether a path is one of KAgent's credential files, which the agent never reads
    /// (token-guard.md).
    credential: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl ReadFile {
    pub fn new() -> Self {
        Self::with_credential_check(Arc::new(crate::agent::hooks::protect::is_credential))
    }

    /// With a check of the caller's (a test's).
    pub fn with_credential_check(credential: Arc<dyn Fn(&str) -> bool + Send + Sync>) -> Self {
        Self {
            contract: ToolContract::read_only(
                "read_file",
                "Read a text file with line numbers. Use list_dir or search_files first when you do not know the exact path.",
                json!({"type":"object","required":["path"],"properties":{
                    "path":{"type":"string","description":"absolute or ~/..."},
                    "start_line":{"type":"integer","description":"1-based, default 1"},
                    "max_lines":{"type":"integer","description":"default 200, max 400"}
                }}),
            )
            .timeout(10_000, 30_000),
            credential,
        }
    }
}

#[derive(Deserialize, Default)]
struct ReadArgs {
    path: Option<String>,
    start_line: Option<usize>,
    max_lines: Option<usize>,
}

#[async_trait]
impl Tool for ReadFile {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: ReadArgs = parse(input)?;
        let Some(path) = args.path.as_deref().filter(|p| !p.is_empty()) else {
            return Ok(ToolOutput::error("path is required"));
        };
        let path = expand(path);
        if (self.credential)(&path) {
            return Ok(ToolOutput::error(format!(
                "{path}: refused, the file holds a KAgent credential (the API token or a secret). The agent never reads it."
            )));
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ToolOutput::error(format!("{path}: not found")))
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(ToolOutput::error(format!(
                    "{path}: permission denied (root-only paths need the read_root helper, stage S8)"
                )))
            }
            Err(e) => return Ok(ToolOutput::error(format!("{path}: {e}"))),
        };
        let head: &[u8] = &bytes[..bytes.len().min(8192)];
        if head.contains(&0) {
            let kind = file_type(&path).await;
            return Ok(ToolOutput::ok(format!("binary file, {} bytes, type: {}", bytes.len(), kind)));
        }
        let text = String::from_utf8_lossy(&bytes);
        let total = text.lines().count().max(1);
        let start = args.start_line.unwrap_or(1).max(1);
        let max = args.max_lines.unwrap_or(200).clamp(1, 400);
        let mut out = format!("{path} ({total} lines total)\n");
        for (i, line) in text.lines().enumerate().skip(start - 1).take(max) {
            out.push_str(&format!("{}\t{}\n", i + 1, crate::util::truncate_str(line, 2000)));
        }
        if start + max <= total {
            out.push_str(&format!("... {} more lines below; raise start_line\n", total - start - max + 1));
        }
        Ok(ToolOutput::ok(out))
    }
}

async fn file_type(path: &str) -> String {
    let out = tokio::process::Command::new("file")
        .arg("-b")
        .arg(path)
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    out
}

// ---------------------------------------------------------------- list_dir

pub struct ListDir {
    contract: ToolContract,
}

impl ListDir {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "list_dir",
                "List a directory: type, size, mtime, name; directories first.",
                json!({"type":"object","required":["path"],"properties":{
                    "path":{"type":"string"},
                    "hidden":{"type":"boolean","description":"include dotfiles, default false"},
                    "limit":{"type":"integer","description":"default 200, max 500"}
                }}),
            )
            .timeout(10_000, 30_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct ListArgs {
    path: Option<String>,
    hidden: Option<bool>,
    limit: Option<usize>,
}

#[async_trait]
impl Tool for ListDir {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: ListArgs = parse(input)?;
        let Some(path) = args.path.as_deref().filter(|p| !p.is_empty()) else {
            return Ok(ToolOutput::error("path is required"));
        };
        let path = expand(path);
        let mut rd = match tokio::fs::read_dir(&path).await {
            Ok(rd) => rd,
            Err(e) => return Ok(ToolOutput::error(format!("{path}: {e}"))),
        };
        let hidden = args.hidden.unwrap_or(false);
        let mut rows = Vec::new();
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let kind = if meta.is_dir() { 'd' } else if meta.is_symlink() { 'l' } else { 'f' };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let date = chrono::DateTime::from_timestamp(mtime as i64, 0)
                .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "?".into());
            let size = if kind == 'd' { String::new() } else { human_size(meta.len()) };
            rows.push((kind, name.clone(), size, date, name.to_lowercase()));
        }
        rows.sort_by(|a, b| (a.0 != 'd', &a.4).cmp(&(b.0 != 'd', &b.4)));
        let limit = args.limit.unwrap_or(200).clamp(1, 500);
        let total = rows.len();
        let mut out = format!("{path} ({total} entries)\n");
        for (kind, name, size, date, _) in rows.into_iter().take(limit) {
            out.push_str(&format!("{kind} {size:>9} {date} {name}\n"));
        }
        if total > limit {
            out.push_str(&format!("... {} more entries\n", total - limit));
        }
        Ok(ToolOutput::ok(out))
    }
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n}B")
    } else if n < 1024 * 1024 {
        format!("{:.1}K", n as f64 / 1024.0)
    } else if n < 1024 * 1024 * 1024 {
        format!("{:.1}M", n as f64 / 1048576.0)
    } else {
        format!("{:.1}G", n as f64 / 1073741824.0)
    }
}

// ---------------------------------------------------------------- search_files

pub struct SearchFiles {
    contract: ToolContract,
}

impl SearchFiles {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "search_files",
                "Search file contents with a regex (ripgrep when installed, else grep -rnE). Returns file:line:match rows.",
                json!({"type":"object","required":["pattern"],"properties":{
                    "pattern":{"type":"string","description":"regex"},
                    "path":{"type":"string","description":"default ~"},
                    "glob":{"type":"string","description":"file glob, e.g. *.conf"},
                    "max_results":{"type":"integer","description":"default 100, max 200"}
                }}),
            )
            .timeout(30_000, 120_000),
        }
    }
}

#[derive(Deserialize, Default)]
struct SearchArgs {
    pattern: Option<String>,
    path: Option<String>,
    glob: Option<String>,
    max_results: Option<usize>,
}

#[async_trait]
impl Tool for SearchFiles {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: SearchArgs = parse(input)?;
        let Some(pattern) = args.pattern.as_deref().filter(|p| !p.is_empty()) else {
            return Ok(ToolOutput::error("pattern is required"));
        };
        let path = expand(args.path.as_deref().unwrap_or("~"));
        if !pattern.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            return Ok(ToolOutput::error("pattern must be plain ASCII"));
        }
        let max = args.max_results.unwrap_or(100).clamp(1, 200);
        let r = if super::exec::which("rg") {
            let mut argv = vec!["rg".to_string(), "-n".into(), "--no-heading".into(), "-m".into(), "5".into()];
            if let Some(g) = &args.glob {
                argv.push("-g".into());
                argv.push(g.clone());
            }
            argv.push("--".into());
            argv.push(pattern.to_string());
            argv.push(path.clone());
            exec(&ctx, &argv).run().await
        } else {
            let mut argv = vec!["grep".to_string(), "-rnE".into()];
            if let Some(g) = &args.glob {
                argv.push("--include".into());
                argv.push(g.clone());
            }
            argv.push("--".into());
            argv.push(pattern.to_string());
            argv.push(path.clone());
            exec(&ctx, &argv).run().await
        };
        let r = match r {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(format!("error: {e:#}"))),
        };
        if !r.success() && r.stdout.trim().is_empty() {
            return Ok(ToolOutput::error(format!("no matches for /{pattern}/ (exit {})", r.code.unwrap_or(1))));
        }
        // Rows from one of KAgent's credential files are left out (token-guard.md).
        let lines: Vec<&str> = r.stdout.lines().filter(|l| !credential_line(l)).take(max).collect();
        let mut out = format!("{} matches for /{pattern}/ in {}\n", lines.len(), path);
        for l in lines {
            out.push_str(crate::util::truncate_str(l, 400));
            out.push('\n');
        }
        Ok(ToolOutput::ok(out))
    }
}

/// Whether a `file:line:match` row of `search_files` comes from one of KAgent's credential files.
fn credential_line(line: &str) -> bool {
    line.split(':').next().is_some_and(crate::agent::hooks::protect::is_credential)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_file_refuses_a_credential_file() {
        let dir = tempfile::tempdir().unwrap();
        let secret = dir.path().join("token");
        std::fs::write(&secret, "not-a-real-token\n").unwrap();
        let other = dir.path().join("notes.txt");
        std::fs::write(&other, "hello\n").unwrap();
        let tool = ReadFile::with_credential_check(Arc::new(|p: &str| p.ends_with("/token")));
        let ctx = ToolContext::standalone(std::time::Duration::from_secs(5), 4_000);
        let out = tool.call(json!({"path": secret.display().to_string()}), ctx.clone()).await.unwrap();
        assert!(out.is_error && out.content.contains("refused"), "{}", out.content);
        assert!(!out.content.contains("not-a-real-token"), "the content must not leak");
        let out = tool.call(json!({"path": other.display().to_string()}), ctx).await.unwrap();
        assert!(!out.is_error && out.content.contains("hello"), "{}", out.content);
    }

    #[test]
    fn search_rows_from_a_credential_file_are_recognised() {
        let (config, _) = crate::agent::hooks::protect::dirs();
        let token = config.join("token").display().to_string();
        assert!(credential_line(&format!("{token}:1:abc")));
        assert!(!credential_line("/etc/hostname:1:box"));
        assert!(!credential_line(&format!("{}/skills/x/SKILL.md:1:token", config.display())));
    }
}
