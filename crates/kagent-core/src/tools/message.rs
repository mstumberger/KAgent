//! `message` tool (docs/specs/tools.md section 15): write a chat message to a person in
//! a desktop chat app (Signal, Slack, Teams, WhatsApp, Telegram, Element, Discord...)
//! by focusing its window and typing into the message box. Two steps by default:
//! type (not sent) -> read the exact text back -> send on the user's yes. Messages to
//! people cannot be taken back, so dictation mistakes must be caught before Enter.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::exec::exec;
use super::focus::activate_chain;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

pub struct MessageTool {
    contract: ToolContract,
}

impl MessageTool {
    pub fn new() -> Self {
        Self {
            contract: ToolContract::read_only(
                "message",
                "Write a chat message to a person in a desktop app (Signal, Slack, Teams, WhatsApp, Telegram, Element, Discord) whose conversation is open. `text` must be exactly the words the user dictated for the other person, never the instruction itself ('send this', 'yes', 'tell him'). Default send=false: the text is typed but NOT sent; read it back to the user word for word and ask. After a yes call again with send=true and empty text to press Enter. send=true with text types and sends at once (only when the user already confirmed that exact text).",
                json!({"type":"object","required":["app"],"properties":{
                    "app":{"type":"string","description":"part of the chat app's window title, e.g. Signal"},
                    "text":{"type":"string","description":"the exact message; empty = only press Enter to send what is typed"},
                    "send":{"type":"boolean","description":"press Enter after typing (default false)"}
                }}),
            )
            .mutating(SideEffect::System, Risk::Medium),
        }
    }
}

/// xdotool commands, run in order (pure, tested). `type` must be last in its command
/// (it takes all remaining arguments as text), so Enter is a separate command.
pub fn message_argv(app: &str, text: &str, send: bool) -> Result<Vec<Vec<String>>, String> {
    if app.trim().is_empty() || app.len() > 80 {
        return Err("app (window title, e.g. Signal) required".into());
    }
    if text.is_empty() && !send {
        return Err("give text to type, or send=true to send what is already typed".into());
    }
    if text.chars().count() > 4000 {
        return Err("message too long (max 4000 characters)".into());
    }
    let mut v = vec!["xdotool".to_string()];
    v.extend(activate_chain(app));
    // Let the app focus its message box after the window activates.
    v.extend(["sleep".into(), "0.3".into()]);
    if !text.is_empty() {
        // Newlines in dictated text would send early in most chat apps: Shift+Enter is
        // not portable, so join lines with spaces.
        let one_line = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        v.extend(["type".into(), "--delay".into(), "8".into(), "--".into(), one_line]);
    }
    let mut cmds = vec![v];
    if send {
        cmds.push(super::focus::enter_cmd());
    }
    Ok(cmds)
}

#[async_trait]
impl Tool for MessageTool {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let app = input.get("app").and_then(Value::as_str).unwrap_or("");
        let text = input.get("text").and_then(Value::as_str).unwrap_or("");
        let send = input.get("send").and_then(Value::as_bool).unwrap_or(false);
        let summary = match (text.is_empty(), send) {
            (true, _) => format!("{app}: send the typed message"),
            (false, false) => format!("{app}: type (not send) \"{}\"", crate::util::truncate_str(text, 200)),
            (false, true) => format!("{app}: type and SEND \"{}\"", crate::util::truncate_str(text, 200)),
        };
        // "Allow for this session" covers this app (each message still shows its text).
        CallAssessment::from_contract(&self.contract, summary, format!("message {app}"))
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        if crate::platform::wayland() {
            return Ok(ToolOutput::error("message needs X11 (xdotool); on Wayland use windows activate + input"));
        }
        if super::xdg::which("xdotool").is_none() {
            return Ok(ToolOutput::error(format!("xdotool is not installed: {}", crate::platform::install_cmd("xdotool"))));
        }
        let app = input.get("app").and_then(Value::as_str).unwrap_or("");
        let text = input.get("text").and_then(Value::as_str).unwrap_or("");
        let send = input.get("send").and_then(Value::as_bool).unwrap_or(false);
        let argv = match message_argv(app, text, send) {
            Ok(a) => a,
            Err(e) => return Ok(ToolOutput::error(e)),
        };
        let mut r = exec(&ctx, &argv[0]).run().await?;
        for cmd in argv.iter().skip(1) {
            if !r.success() {
                break;
            }
            r = exec(&ctx, cmd).run().await?;
        }
        if !r.success() {
            return Ok(ToolOutput::error(if r.stdout.trim().is_empty() && r.stderr.trim().is_empty() {
                format!("no window titled like '{app}' (is the chat open? windows list shows titles)")
            } else {
                r.render()
            }));
        }
        Ok(ToolOutput::ok(match (text.is_empty(), send) {
            (true, _) => format!("pressed Enter in {app}: the typed message was sent"),
            (false, false) => format!("typed into {app}, NOT sent yet: \"{text}\". Read this exact text back to the user and ask; on yes call message with send=true and empty text."),
            (false, true) => format!("sent to {app}: \"{text}\""),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_steps() {
        let a = message_argv("Signal", "Hello\nfrom KAgent", false).unwrap();
        assert_eq!(a.len(), 1, "not sent by default");
        assert_eq!(&a[0][1..8], &["search", "--limit", "1", "--name", "Signal", "windowactivate", "--sync"]);
        assert_eq!(a[0].last().unwrap(), "Hello from KAgent", "type is the last command of its chain");
        let s = message_argv("Signal", "hi", true).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1], vec!["xdotool", "sleep", "0.5", "key", "Return"], "Enter is its own command, never typed as text");
        assert!(message_argv("Signal", "", false).is_err());
        assert!(message_argv("", "hi", false).is_err());
        let t = MessageTool::new();
        assert!(t.assess(&json!({"app":"Signal","text":"hi","send":true})).summary.contains("SEND \"hi\""));
        assert_eq!(t.assess(&json!({"app":"Signal","text":"x"})).rule_key, "message Signal");
    }
}
