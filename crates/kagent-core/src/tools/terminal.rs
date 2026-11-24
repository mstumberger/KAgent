//! `terminal` tool (docs/specs/tools.md section 15): find the user's terminal tabs
//! (Konsole over D-Bus, tmux panes) with the program running in each and its folder,
//! and type text into one, e.g. "tell the Claude Code session in KAgent: ...".
//! Every send asks (typing into a shell runs commands).

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

#[derive(Debug, Clone, PartialEq)]
pub struct Term {
    /// `konsole:<service>:<path>` or `tmux:<pane>`.
    pub id: String,
    pub app: String,
    pub title: String,
    /// Foreground program (`claude`, `bash`, `vim`, ...).
    pub program: String,
    pub cwd: String,
}

impl Term {
    pub fn is_shell(&self) -> bool {
        matches!(self.program.as_str(), "bash" | "zsh" | "fish" | "sh" | "dash" | "ksh" | "tcsh" | "nu")
    }
    fn render(&self) -> String {
        format!("{}  [{}] {}  in {}  ({})", self.id, self.app, self.program, self.cwd, self.title)
    }
}

fn comm(pid: &str) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn cwd(pid: &str) -> String {
    std::fs::read_link(format!("/proc/{pid}/cwd")).map(|p| p.display().to_string()).unwrap_or_default()
}

async fn konsole_tabs(ctx: &ToolContext) -> Vec<Term> {
    let mut out = Vec::new();
    let Ok(names) = exec(ctx, &["qdbus6"]).run().await else { return out };
    for svc in names.stdout.lines().map(str::trim).filter(|l| l.starts_with("org.kde.konsole-")) {
        let Ok(paths) = exec(ctx, &["qdbus6", svc]).run().await else { continue };
        for path in paths.stdout.lines().map(str::trim).filter(|p| p.strip_prefix("/Sessions/").is_some_and(|n| n.parse::<u32>().is_ok())) {
            let q = |m: &str| {
                let m = format!("org.kde.konsole.Session.{m}");
                let (svc, path) = (svc.to_string(), path.to_string());
                async move { exec(ctx, &["qdbus6", &svc, &path, &m]).run().await.map(|r| r.stdout.trim().to_string()).unwrap_or_default() }
            };
            let fg = q("foregroundProcessId").await;
            let title = {
                let (svc, path) = (svc.to_string(), path.to_string());
                exec(ctx, &["qdbus6", &svc, &path, "org.kde.konsole.Session.title", "1"]).run().await.map(|r| r.stdout.trim().to_string()).unwrap_or_default()
            };
            out.push(Term { id: format!("konsole:{svc}:{path}"), app: "konsole".into(), title, program: comm(&fg), cwd: cwd(&fg) });
        }
    }
    out
}

/// `tmux list-panes -a -F '#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{session_name}:#{window_name}'`.
pub fn parse_tmux(out: &str) -> Vec<Term> {
    out.lines()
        .filter_map(|l| {
            let mut it = l.split('\t');
            let (id, prog, path, title) = (it.next()?, it.next()?, it.next()?, it.next().unwrap_or(""));
            Some(Term { id: format!("tmux:{id}"), app: "tmux".into(), title: title.into(), program: prog.into(), cwd: path.into() })
        })
        .collect()
}

pub async fn list(ctx: &ToolContext) -> Vec<Term> {
    let mut v = konsole_tabs(ctx).await;
    if super::xdg::which("tmux").is_some() {
        if let Ok(r) = exec(ctx, &["tmux", "list-panes", "-a", "-F", "#{pane_id}\t#{pane_current_command}\t#{pane_current_path}\t#{session_name}:#{window_name}"]).run().await {
            v.extend(parse_tmux(&r.stdout));
        }
    }
    v
}

/// Target by exact id, else every word of the query must match program, folder or title
/// ("claude KAgent" -> the claude tab whose folder is .../KAgent). Ambiguity is an error.
pub fn pick<'a>(terms: &'a [Term], query: &str) -> Result<&'a Term, String> {
    if let Some(t) = terms.iter().find(|t| t.id == query) {
        return Ok(t);
    }
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Err("target required (see action list)".into());
    }
    let hay = |t: &Term| format!("{} {} {} {}", t.program, t.cwd, t.title, t.app).to_lowercase();
    let hits: Vec<&Term> = terms.iter().filter(|t| words.iter().all(|w| hay(t).contains(w.as_str()))).collect();
    match hits.len() {
        0 => Err(format!("no terminal matches '{query}'")),
        1 => Ok(hits[0]),
        n => {
            // Prefer the one whose folder name equals a query word ("KAgent" vs "KAgent-old").
            let exact: Vec<&&Term> = hits.iter().filter(|t| words.iter().any(|w| t.cwd.to_lowercase().rsplit('/').next() == Some(w.as_str()))).collect();
            if exact.len() == 1 {
                return Ok(exact[0]);
            }
            Err(format!("{n} terminals match '{query}': {}", hits.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", ")))
        }
    }
}

/// A command line that runs something as root (sudo/pkexec/su/doas), anywhere in it.
pub fn is_sudo(text: &str) -> bool {
    text.split(|c: char| c == ';' || c == '|' || c == '&' || c == '\n' || c == '(' || c == '`' || c == '$')
        .filter_map(|part| part.split_whitespace().next())
        .any(|w| matches!(w, "sudo" | "pkexec" | "su" | "doas" | "run0"))
}

pub struct Terminal {
    contract: ToolContract,
}

impl Terminal {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "terminal",
                "The user's terminal tabs (Konsole, tmux). list: every tab with the program running in it (claude, bash, vim...) and its folder. send: type text into one tab and press Enter (enter=false to leave it unsent; empty text + enter=true only presses Enter, use that instead of re-sending); target = id from list, or words matching program/folder/title, e.g. \"claude KAgent\" for the Claude Code session in the KAgent folder. Asks first.",
                json!({"type":"object","required":["action"],"properties":{
                    "action":{"type":"string","enum":["list","send"]},
                    "target":{"type":"string"},
                    "text":{"type":"string"},
                    "enter":{"type":"boolean"}
                }}),
            )
            .mutating(SideEffect::System, Risk::Medium),
        }
    }
}

#[async_trait]
impl Tool for Terminal {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "list" {
            return CallAssessment::from_contract(&self.contract, "list terminals".into(), "terminal list".into()).as_read_only();
        }
        let target = input.get("target").and_then(Value::as_str).unwrap_or("");
        let text = input.get("text").and_then(Value::as_str).unwrap_or("");
        let mut a = CallAssessment::from_contract(&self.contract, format!("type into terminal '{target}': {}", crate::util::truncate_str(text, 120)), format!("terminal send {target}"));
        // Each message is different: never covered by a remembered rule.
        a.always_ask = true;
        // Typing a destructive command into a terminal is the same as running it.
        if crate::safety::looks_destructive(text) {
            a = a.destructive();
        }
        a
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let terms = list(&ctx).await;
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "list" {
            return Ok(if terms.is_empty() { ToolOutput::ok("no Konsole tabs or tmux panes found (other terminals: use input with windows activate)") } else { ToolOutput::ok(terms.iter().map(Term::render).collect::<Vec<_>>().join("\n")) });
        }
        if action != "send" {
            return Ok(ToolOutput::error("action must be list or send"));
        }
        let text = input.get("text").and_then(Value::as_str).unwrap_or("");
        let enter = input.get("enter").and_then(Value::as_bool).unwrap_or(true);
        // Empty text + enter = just press Enter (submit what is already typed); never
        // re-send the whole text to "press Enter again".
        if (text.is_empty() && !enter) || text.len() > 8_000 {
            return Ok(ToolOutput::error("text required (max 8000 bytes), or empty text with enter=true to only press Enter"));
        }
        let t = match pick(&terms, input.get("target").and_then(Value::as_str).unwrap_or("")) {
            Ok(t) => t.clone(),
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        if t.is_shell() && is_sudo(text) {
            return Ok(ToolOutput::error("KAgent does not type sudo commands into a shell. Put the command on the clipboard (clipboard set) and let the owner paste and run it."));
        }
        let ok = if let Some(rest) = t.id.strip_prefix("konsole:") {
            let (svc, path) = rest.split_once(':').unwrap_or((rest, "/Sessions/1"));
            let first = exec(&ctx, &["qdbus6", svc, path, "org.kde.konsole.Session.sendText", if text.is_empty() { "\r" } else { text }]).run().await?;
            if !first.success() {
                if first.render().contains("Security sensitive DBus API is disabled") {
                    // Fallback: focus that Konsole window and type with xdotool.
                    let pid = svc.trim_start_matches("org.kde.konsole-");
                    if super::xdg::which("xdotool").is_some() && pid.chars().all(|c| c.is_ascii_digit()) {
                        let win = exec(&ctx, &["xdotool", "search", "--pid", pid]).run().await?.stdout.lines().last().unwrap_or("").to_string();
                        if !win.is_empty() {
                            let mut argv = vec!["xdotool".to_string(), "windowactivate".into(), "--sync".into(), win];
                            if !text.is_empty() {
                                argv.extend(["type".into(), "--delay".into(), "8".into(), "--".into(), text.to_string()]);
                            }
                            let mut r = exec(&ctx, &argv).run().await?;
                            if r.success() && enter {
                                // Separate command: `type` would type "key Return" as text.
                                r = exec(&ctx, &super::focus::enter_cmd()).run().await?;
                            }
                            return Ok(if r.success() {
                                ToolOutput::ok(format!("typed into the Konsole window of {} (its current tab) with xdotool{}", t.cwd, if enter { " and pressed Enter" } else { "" }))
                            } else {
                                ToolOutput::error(r.render())
                            });
                        }
                    }
                    return Ok(ToolOutput::error(
                        "Konsole blocks typing over D-Bus. The owner enables it once: Konsole -> Settings -> Configure Konsole -> General -> 'Enable the security sensitive parts of the DBus API' (then restart Konsole). Alternative: install xdotool, then KAgent types into the focused Konsole window.",
                    ));
                }
                return Ok(ToolOutput::error(format!("could not send to {}: {}", t.id, first.render())));
            }
            let mut ok = true;
            if enter && !text.is_empty() {
                // A separate carriage return: TUIs (Claude Code) treat a pasted "\n" as a newline in the prompt.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                ok = exec(&ctx, &["qdbus6", svc, path, "org.kde.konsole.Session.sendText", "\r"]).run().await?.success();
            }
            ok
        } else if let Some(pane) = t.id.strip_prefix("tmux:") {
            let mut ok = exec(&ctx, &["tmux", "send-keys", "-t", pane, "-l", "--", text]).run().await?.success();
            if ok && enter {
                ok = exec(&ctx, &["tmux", "send-keys", "-t", pane, "Enter"]).run().await?.success();
            }
            ok
        } else {
            false
        };
        Ok(if ok {
            ToolOutput::ok(format!("sent to {} ({} in {}){}", t.id, t.program, t.cwd, if enter { " and pressed Enter" } else { "" }))
        } else {
            ToolOutput::error(format!("could not send to {}", t.id))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, prog: &str, cwd: &str) -> Term {
        Term { id: id.into(), app: "konsole".into(), title: format!("{} : {prog}", cwd.rsplit('/').next().unwrap_or("")), program: prog.into(), cwd: cwd.into() }
    }

    #[test]
    fn picks_the_right_tab() {
        let terms = vec![t("konsole:a:/Sessions/1", "claude", "/home/u/SI/KAgent"), t("konsole:b:/Sessions/1", "bash", "/home/u"), t("konsole:c:/Sessions/1", "claude", "/home/u/SI/ReelGen")];
        assert_eq!(pick(&terms, "claude KAgent").unwrap().id, "konsole:a:/Sessions/1");
        assert_eq!(pick(&terms, "reelgen").unwrap().id, "konsole:c:/Sessions/1");
        assert!(pick(&terms, "claude").unwrap_err().contains("2 terminals match"));
        assert_eq!(pick(&terms, "konsole:b:/Sessions/1").unwrap().program, "bash");
        assert!(pick(&terms, "vim").is_err());
        assert!(terms[1].is_shell() && !terms[0].is_shell());
        assert!(is_sudo("sudo apt install x") && is_sudo("cd /tmp && sudo rm -rf x") && is_sudo("echo hi | sudo tee /etc/x") && is_sudo("pkexec bash"));
        assert!(!is_sudo("echo sudo is fine as an argument") && !is_sudo("ls -la"));
    }

    #[test]
    fn tmux_and_assessment() {
        let p = parse_tmux("%1\tclaude\t/home/u/KAgent\twork:0\n%2\tzsh\t/tmp\twork:1\n");
        assert_eq!((p[0].id.as_str(), p[0].program.as_str(), p[1].cwd.as_str()), ("tmux:%1", "claude", "/tmp"));
        let tool = Terminal::new();
        assert!(tool.assess(&json!({"action": "list"})).read_only);
        let a = tool.assess(&json!({"action": "send", "target": "claude", "text": "hi"}));
        assert!(a.always_ask && !a.read_only);
    }
}
