//! `sessions` tool (docs/specs/tools.md section 15): the conversation history KAgent
//! keeps (chat and voice, with tool calls): list, read one as a transcript, or export
//! it to a Markdown file the user can share.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::{Message, Role};
use crate::session::SessionStore;
use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};

pub struct Sessions {
    contract: ToolContract,
    store: SessionStore,
}

impl Sessions {
    pub fn new(dir: std::path::PathBuf) -> anyhow::Result<Self> {
        let mut contract = ToolContract::read_only(
            "sessions",
            "KAgent's saved conversations (chat and voice, including tool calls). list: recent sessions; read: one session as a transcript (session id, or omit for the current one); export: write it as Markdown to ~/Documents/KAgent/<title>.md (or `path` under your home) and return the path.",
            json!({"type":"object","required":["action"],"properties":{
                "action":{"type":"string","enum":["list","read","export"]},
                "session":{"type":"string"},
                "path":{"type":"string"}
            }}),
        )
        .mutating(SideEffect::Workspace, Risk::Low);
        contract.needs_approval = false; // export writes a new file in the user's documents
        Ok(Self { contract, store: SessionStore::new(dir)? })
    }
}

/// Markdown transcript: who said what (and via voice), tool calls with a short result.
pub fn transcript(title: &str, msgs: &[Message]) -> String {
    let mut out = format!("# {title}\n\n");
    for m in msgs {
        let via = if m.via == crate::protocol::Via::Voice { " (voice)" } else { "" };
        match m.role {
            Role::User => out.push_str(&format!("**You{via}:** {}\n\n", m.content.trim())),
            Role::Assistant => {
                if !m.content.trim().is_empty() {
                    out.push_str(&format!("**KAgent{via}:** {}\n\n", plain_widgets(&m.content)));
                }
                for c in &m.tool_calls {
                    out.push_str(&format!("- tool `{}` {}\n", c.name, crate::util::truncate_str(&c.input.to_string(), 200)));
                }
            }
            Role::Tool => {
                let first = m.content.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                out.push_str(&format!("  - {} {}\n\n", if m.is_error { "failed:" } else { "->" }, crate::util::truncate_str(first, 160)));
            }
        }
    }
    out
}

/// The transcript without the panels: a line that is only a widget marker
/// (`::widget{id="<session>/<n>"}`, widgets.md section 2) becomes `[widget <n>]`. Any
/// other line stays as it is.
pub fn plain_widgets(text: &str) -> String {
    text.split('\n')
        .map(|line| match widget_number(line.trim()) {
            Some(n) => format!("[widget {n}]"),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The number in the marker line `line`, when the line is one: the marker as the tool
/// answers it (`::widget{id="<session>/<n>"}`), or the same braces as a JSON object.
fn widget_number(line: &str) -> Option<String> {
    let inner = line.strip_prefix("::widget{")?.strip_suffix('}')?.trim();
    let id = match serde_json::from_str::<Value>(&format!("{{{inner}}}")) {
        Ok(object) => object.get("id")?.as_str()?.to_string(),
        Err(_) => {
            let id = inner.strip_prefix("id")?.trim_start().strip_prefix('=')?.trim_start();
            id.strip_prefix('"')?.strip_suffix('"')?.to_string()
        }
    };
    let (_, n) = id.rsplit_once('/')?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| n.to_string())
}

fn safe_title(t: &str) -> String {
    let s: String = t.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let s = s.trim_matches('-').chars().take(60).collect::<String>();
    if s.is_empty() { "session".into() } else { s }
}

#[async_trait]
impl Tool for Sessions {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    fn assess(&self, input: &Value) -> CallAssessment {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let a = CallAssessment::from_contract(&self.contract, format!("sessions {action}"), format!("sessions {action}"));
        if action == "export" { a } else { a.as_read_only() }
    }
    async fn call(&self, input: Value, ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        let metas = self.store.list().await?;
        if action == "list" {
            let rows = metas.iter().take(30).map(|m| format!("{}  {}  {} messages  \"{}\"", m.id, m.updated_at.format("%Y-%m-%d %H:%M"), m.message_count, m.title)).collect::<Vec<_>>().join("\n");
            return Ok(ToolOutput::ok(format!("{rows}\n{}", crate::util::more_note(metas.len(), 30, "session_search finds an older conversation by its words")).trim_end().to_string()));
        }
        let wanted = input.get("session").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from).unwrap_or_else(|| ctx.session_id.clone());
        let Some(meta) = metas.iter().find(|m| m.id == wanted || m.id.starts_with(&wanted)) else {
            return Ok(ToolOutput::error(format!("no session {wanted} (see sessions list)")));
        };
        let text = transcript(&meta.title, &self.store.load_messages(&meta.id).await?);
        match action {
            "read" => Ok(ToolOutput::ok(crate::util::cut_with_note(&text, 20_000, "export the session to read all of it"))),
            "export" => {
                let home = super::xdg::home();
                let path = match input.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()) {
                    Some(p) => super::xdg::expand(p),
                    None => home.join("Documents/KAgent").join(format!("{}-{}.md", meta.updated_at.format("%Y-%m-%d"), safe_title(&meta.title))),
                };
                if !path.starts_with(&home) || path.components().any(|c| c == std::path::Component::ParentDir) {
                    return Ok(ToolOutput::error("export path must be inside your home folder"));
                }
                if let Some(dir) = path.parent() {
                    tokio::fs::create_dir_all(dir).await?;
                }
                tokio::fs::write(&path, &text).await?;
                Ok(ToolOutput::ok(format!("exported to {}", path.display())))
            }
            _ => Ok(ToolOutput::error("action must be list, read or export")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ToolCall, Via};

    #[test]
    fn markdown_transcript() {
        let call = ToolCall { id: "c1".into(), name: "doctor".into(), input: json!({}) };
        let msgs = vec![
            Message::user("what is using swap?").with_via(Via::Voice),
            Message::assistant("", vec![call.clone()]),
            Message::tool_result(&call, "health score: 85/100\n...", false),
            Message::assistant("IntelliJ and PyCharm.", vec![]).with_via(Via::Voice),
        ];
        let t = transcript("Voice session", &msgs);
        assert!(t.starts_with("# Voice session"));
        assert!(t.contains("**You (voice):** what is using swap?"), "{t}");
        assert!(t.contains("- tool `doctor` {}") && t.contains("-> health score: 85/100"));
        assert!(t.contains("**KAgent (voice):** IntelliJ and PyCharm."));
        assert_eq!(safe_title("Voice session: swap?"), "Voice-session--swap");
    }

    /// Scenario S33 item 1: an export replaces the markers, and so does `sessions read`.
    #[test]
    fn the_transcript_replaces_the_widget_markers() {
        let marker = "::widget{id=\"0f1e2d3c-aaaa-4bbb-8ccc-1234567890ab/7\"}";
        let answer = format!("Your doctor score:\n\n{marker}\n\n::widget{{ \"id\": \"s/12\" }}\n  ::widget{{id=\"s/3\"}}  \nstill text: {marker}\n::widget{{id=\"no-number\"}}\n::todo{{id=\"s/1\"}}");
        let back = plain_widgets(&answer);
        assert_eq!(back.lines().filter(|l| l.starts_with("[widget")).collect::<Vec<_>>(), ["[widget 7]", "[widget 12]", "[widget 3]"]);
        assert!(back.contains(&format!("still text: {marker}")), "only a line that is the marker: {back}");
        assert!(back.contains("::widget{id=\"no-number\"}") && back.contains("::todo{id=\"s/1\"}"), "{back}");
        let msgs = vec![Message::user("how is my machine?"), Message::assistant(answer, vec![])];
        let t = transcript("gauge", &msgs);
        assert!(t.contains("[widget 7]\n\n"), "{t}");
        assert_eq!(t.matches("[widget ").count(), 3, "the three lines that are the marker: {t}");
        assert!(t.contains(&format!("still text: {marker}")), "a marker inside a line of text is text: {t}");
    }

    /// agent-loop.md B.2 question 4: a cut list or transcript says so and how to get the rest.
    #[tokio::test]
    async fn a_long_list_and_a_long_transcript_say_what_was_cut() {
        let dir = tempfile::tempdir().unwrap();
        let tool = Sessions::new(dir.path().to_path_buf()).unwrap();
        let store = crate::session::SessionStore::new(dir.path()).unwrap();
        let mut last = String::new();
        for i in 0..32 {
            last = store.create(&format!("s{i}")).await.unwrap().id;
        }
        let ctx = ToolContext::standalone(std::time::Duration::from_secs(5), 100_000);
        let list = tool.call(json!({"action": "list"}), ctx.clone()).await.unwrap();
        assert!(list.content.ends_with("... 2 more not shown (session_search finds an older conversation by its words)"), "{}", list.content);
        store.append(&last, &[Message::user("x".repeat(25_000))]).await.unwrap();
        let read = tool.call(json!({"action": "read", "session": last}), ctx).await.unwrap();
        assert!(read.content.contains("\n[cut: ") && read.content.ends_with("(export the session to read all of it)]"), "{}", &read.content[read.content.len() - 120..]);
    }
}
