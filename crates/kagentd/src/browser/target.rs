//! The tab a browser action runs in (browser.md section 3): one trait for both backends,
//! so `Browser` runs the same action code on a tab of KAgent's own profile and on a tab
//! of the owner's running browser. Also the tab rules of attach mode (section 4) and
//! how its default target is chosen. The actions themselves are in `actions.rs`.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use kagent_core::tool::CallAssessment;
use serde_json::{json, Value};

use super::cdp;
use crate::sections::AttachTrust;

/// A tab as the actions see it: its id (the DevTools target id) and what it shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabInfo {
    pub id: String,
    pub title: String,
    pub url: String,
}

/// One tab under the DevTools protocol. `call` and `events` are the transport; the
/// rest is written once, here.
#[async_trait]
pub trait TabTarget: Send {
    async fn call(&mut self, method: &str, params: Value) -> Result<Value>;
    /// The tab's CDP events (`method`, `params`) that arrive in the next `listen`.
    async fn events(&mut self, listen: Duration) -> Vec<Value>;
    fn info(&self) -> TabInfo;

    /// Evaluate an expression and return its JSON value (awaits promises).
    async fn eval(&mut self, expr: &str) -> Result<Value> {
        let r = self.call("Runtime.evaluate", json!({"expression": expr, "returnByValue": true, "awaitPromise": true, "userGesture": true})).await?;
        if let Some(ex) = r.get("exceptionDetails") {
            let text = ex.pointer("/exception/description").or_else(|| ex.get("text")).and_then(Value::as_str).unwrap_or("exception");
            bail!("page script error: {}", text.lines().next().unwrap_or(text));
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    async fn click_at(&mut self, x: f64, y: f64) -> Result<()> {
        for t in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.call("Input.dispatchMouseEvent", json!({"type": t, "x": x, "y": y, "button": "left", "clickCount": 1})).await?;
        }
        Ok(())
    }

    async fn key(&mut self, key: &str) -> Result<()> {
        let (code, vk, text) = cdp::key_info(key).ok_or_else(|| anyhow!("unsupported key {key}"))?;
        let mut down = json!({"type": "keyDown", "key": key, "code": code, "windowsVirtualKeyCode": vk});
        if let Some(t) = text {
            down["text"] = json!(t);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk})).await?;
        Ok(())
    }

    /// Console messages, uncaught exceptions and browser log entries of the tab.
    /// Chrome replays what was logged before `Runtime.enable` / `Log.enable`, so this
    /// also returns messages from before the call; then it listens for `listen`.
    async fn console(&mut self, listen: Duration, max: usize) -> Result<Vec<String>> {
        for m in ["Runtime.enable", "Log.enable"] {
            self.call(m, json!({})).await?;
        }
        let mut out = Vec::new();
        for v in self.events(listen.max(Duration::from_millis(800))).await {
            if let Some(line) = cdp::console_line(&v) {
                out.push(line);
                if out.len() >= max {
                    break;
                }
            }
        }
        Ok(out)
    }
}

/// Whose tab it is in attach mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    /// Opened by the agent in this daemon run.
    Agent,
    /// One of the owner's own tabs.
    Owner,
}

/// What the permission engine must see for one action in one kind of tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Read-only: nothing to approve.
    ReadOnly,
    /// No approval: showing a page, or the agent's own tab.
    Free,
    /// Approval, medium, rememberable: the rule of the own profile.
    Ask,
    /// A card in every mode, including full control; never rememberable.
    Always,
    /// Refused when the call runs (there is nothing to approve).
    Refused,
}

/// The tab rules of attach mode (browser.md section 4), one row per action.
pub fn gate(kind: TabKind, trust: AttachTrust, action: &str) -> Gate {
    match action {
        "status" | "tabs" | "read" | "screenshot" | "console" => Gate::ReadOnly,
        "launch" => Gate::Refused,
        "open" | "activate" => Gate::Free,
        "close" if kind == TabKind::Agent => Gate::Free,
        "close" => Gate::Refused,
        // `back` leaves the owner's page as `navigate` does (decision of 2026-11-27).
        "navigate" | "scroll" | "back" if kind == TabKind::Owner && trust == AttachTrust::Strict => Gate::Always,
        "navigate" | "scroll" | "back" => Gate::Free,
        "click" | "type" | "key" if kind == TabKind::Owner && trust == AttachTrust::Strict => Gate::Always,
        "click" | "type" | "key" => Gate::Ask,
        "eval" => Gate::Always,
        // An action the schema does not know: the contract's rule.
        _ => Gate::Ask,
    }
}

/// The assessment `a` (from the contract) under gate `g`.
pub fn gated(mut a: CallAssessment, g: Gate) -> CallAssessment {
    match g {
        Gate::ReadOnly => a.as_read_only(),
        Gate::Free => {
            a.needs_approval = false;
            a
        }
        Gate::Ask => a,
        // `always_ask` alone is let through by `full` control. The engine's destructive
        // gate asks in every mode and takes no remembered rule and no override.
        Gate::Always => {
            a.always_ask = true;
            a.destructive()
        }
        Gate::Refused => {
            a.needs_approval = false;
            a
        }
    }
}

/// How much the engine had to see before the call may run.
fn level(g: Gate) -> u8 {
    match g {
        Gate::ReadOnly | Gate::Free | Gate::Refused => 0,
        Gate::Ask => 1,
        Gate::Always => 2,
    }
}

/// Whether a call that passed under `assessed` needs more for the tab it resolved to.
pub fn escalates(assessed: Gate, actual: Gate) -> bool {
    level(actual) > level(assessed)
}

/// The tab for the default target among the candidates whose `document.visibilityState`
/// was read: the first that answered `visible`.
pub fn pick_visible<'a>(checked: &[(&'a str, Option<&'a str>)]) -> Option<&'a str> {
    checked.iter().find(|(_, state)| *state == Some("visible")).map(|(id, _)| *id)
}

/// A DevTools target id (32 hex characters) is matched by id only, never as part of
/// a title or a URL: a needle of that shape can never slide over to another tab.
pub fn looks_like_target_id(needle: &str) -> bool {
    needle.len() == 32 && needle.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::browser::attach::BrowserSession;
    use crate::browser::Browser;
    use kagent_core::permission::{AllowRule, Decision, PermissionEngine, PermissionMode};
    use kagent_core::tool::{Risk, Tool};
    use std::sync::Arc;

    const ACTIONS: [&str; 16] = ["status", "launch", "tabs", "open", "navigate", "activate", "close", "back", "read", "click", "type", "key", "scroll", "screenshot", "console", "eval"];

    /// browser.md section 9: the tab rules table, row by row (section 4).
    #[test]
    fn tab_rules_table() {
        use AttachTrust::{Normal, Strict};
        use Gate::*;
        use TabKind::*;
        let row = |action: &str, agent: Gate, strict: Gate, normal: Gate| {
            assert_eq!(gate(Agent, Strict, action), agent, "agent tab: {action}");
            assert_eq!(gate(Agent, Normal, action), agent, "agent tab, normal: {action}");
            assert_eq!(gate(Owner, Strict, action), strict, "the owner's tab, strict: {action}");
            assert_eq!(gate(Owner, Normal, action), normal, "the owner's tab, normal: {action}");
        };
        for a in ["read", "screenshot", "console"] {
            row(a, ReadOnly, ReadOnly, ReadOnly);
        }
        for a in ["open", "activate"] {
            row(a, Free, Free, Free);
        }
        for a in ["navigate", "scroll", "back"] {
            row(a, Free, Always, Free);
        }
        for a in ["click", "type", "key"] {
            row(a, Ask, Always, Ask);
        }
        row("close", Free, Refused, Refused);
        row("eval", Always, Always, Always);
        row("launch", Refused, Refused, Refused);
        row("status", ReadOnly, ReadOnly, ReadOnly);
        row("tabs", ReadOnly, ReadOnly, ReadOnly);
        // Every action the tool knows has its row: the schema's enum is this list.
        let b = Browser::new(std::path::Path::new("/tmp/x"), 9333, "");
        let listed: Vec<String> = serde_json::from_value(b.contract().input_schema["properties"]["action"]["enum"].clone()).unwrap();
        assert_eq!(listed, ACTIONS.iter().map(|a| a.to_string()).collect::<Vec<_>>());
        // A stricter gate is never accepted under a lighter one; the other way round is fine.
        for light in [ReadOnly, Free, Ask, Always, Refused] {
            for heavy in [ReadOnly, Free, Ask, Always, Refused] {
                assert_eq!(escalates(light, heavy), level(heavy) > level(light), "{light:?} -> {heavy:?}");
            }
        }
        assert!(escalates(gate(Agent, Strict, "click"), gate(Owner, Strict, "click")));
        assert!(!escalates(gate(Owner, Strict, "click"), gate(Agent, Strict, "click")));
    }

    /// The rules as the permission engine sees them.
    #[test]
    fn what_the_engine_sees_in_each_tab() {
        let dir = tempfile::tempdir().unwrap();
        let session = Arc::new(BrowserSession::new(Some((dir.path().to_path_buf(), "Brave".into()))));
        let b = Browser::attach(dir.path(), session.clone(), AttachTrust::Strict);
        let owner = |v: Value| b.assess(&v);
        // The owner's tab, strict: read is read-only; acting asks in every mode; closing and launch are refused when run.
        let read = owner(json!({"action": "read"}));
        assert!(read.read_only && !read.needs_approval && !read.always_ask);
        for a in ["navigate", "scroll", "back"] {
            let x = owner(json!({"action": a}));
            assert!(!x.read_only && x.always_ask && x.destructive && x.needs_approval, "{a} in the owner's tab: {x:?}");
            assert_eq!(x.risk, Risk::Medium, "{a} keeps the risk of the contract");
            assert_eq!(x.rule_key, format!("browser@owner {a}"));
        }
        for a in ["click", "type", "key", "eval"] {
            let x = owner(json!({"action": a}));
            assert!(!x.read_only && x.always_ask && x.destructive && x.needs_approval, "{a} in the owner's tab: {x:?}");
            assert_eq!(x.risk, Risk::High, "{a}");
        }
        assert!(owner(json!({"action": "close"})).summary.contains("the owner's tab"));
        for a in ["close", "launch"] {
            let x = owner(json!({"action": a}));
            assert!(!x.read_only && !x.needs_approval, "{a} is refused when run, so there is nothing to approve: {x:?}");
        }
        for a in ["open", "activate"] {
            let x = owner(json!({"action": a}));
            assert!(!x.read_only && !x.needs_approval && !x.always_ask, "{a}: {x:?}");
            // A new tab is nobody's; a tab that is shown keeps its kind in the key.
            assert_eq!(x.rule_key, if a == "open" { "browser open".to_string() } else { format!("browser@owner {a}") });
        }
        // The engine: full control and a stored rule do not get past the owner's strict rules.
        let engine = PermissionEngine::new(PermissionMode::Full, vec![AllowRule { tool: "browser".into(), prefix: "browser".into() }]);
        assert!(matches!(engine.evaluate("s", "browser", &owner(json!({"action": "navigate", "url": "https://x.y"}))), Decision::Ask { .. }));
        assert!(matches!(engine.evaluate("s", "browser", &owner(json!({"action": "click", "element": 1}))), Decision::Ask { .. }));
        assert!(matches!(engine.evaluate("s", "browser", &owner(json!({"action": "eval", "script": "1"}))), Decision::Ask { .. }));
        assert!(matches!(engine.evaluate("s", "browser", &read), Decision::Allow { .. }));
        assert!(matches!(engine.evaluate("s", "browser", &owner(json!({"action": "close"}))), Decision::Allow { .. }), "nothing to approve: the call itself refuses");

        // A tab the agent opened: the rules of the own profile.
        session.note_agent_tab("T-AGENT", "", "about:blank");
        let agent = |v: Value| b.assess(&v);
        let click = agent(json!({"action": "click", "element": 2, "tab": "T-AGENT"}));
        assert!(click.needs_approval && !click.always_ask && !click.destructive && click.risk == Risk::Medium, "{click:?}");
        assert_eq!(click.rule_key, "browser click");
        assert!(click.summary.ends_with("(an agent tab)"), "{}", click.summary);
        for a in ["navigate", "scroll", "close"] {
            let x = agent(json!({"action": a, "tab": "T-AGENT"}));
            assert!(!x.needs_approval && !x.always_ask, "{a} in an agent tab: {x:?}");
        }
        let eval_ = agent(json!({"action": "eval", "script": "1", "tab": "T-AGENT"}));
        assert!(eval_.always_ask && eval_.destructive && eval_.risk == Risk::High, "eval asks in every mode");
        assert!(matches!(engine.evaluate("s", "browser", &eval_), Decision::Ask { .. }));
        assert!(matches!(engine.evaluate("s", "browser", &agent(json!({"action": "navigate", "url": "https://x.y", "tab": "T-AGENT"}))), Decision::Allow { .. }));
        // Without a tab, the call goes to the agent's most recent tab.
        let x = agent(json!({"action": "close"}));
        assert!(!x.needs_approval && x.summary.ends_with("(an agent tab)"), "{x:?}");
        // Part of a title can also be one of the owner's tabs: the strict rules.
        let x = agent(json!({"action": "navigate", "url": "https://x.y", "tab": "agent"}));
        assert!(x.always_ask && x.destructive, "{x:?}");

        // The owner's tab, normal: as the own profile, except closing.
        let b = Browser::attach(dir.path(), session, AttachTrust::Normal);
        let normal = |v: Value| b.assess(&v);
        for a in ["navigate", "scroll"] {
            let x = normal(json!({"action": a}));
            assert!(!x.needs_approval && !x.always_ask, "{a}: {x:?}");
        }
        for a in ["click", "type", "key"] {
            let x = normal(json!({"action": a, "tab": "Mail"}));
            assert!(x.needs_approval && !x.always_ask && !x.destructive && x.risk == Risk::Medium, "{a}: {x:?}");
            assert_eq!(x.rule_key, format!("browser@owner {a}"), "a rule remembered for a tab is for that kind of tab only");
        }
        assert!(!normal(json!({"action": "close"})).needs_approval, "refused when run");
        let x = normal(json!({"action": "eval"}));
        assert!(x.always_ask && x.destructive);
    }

    #[test]
    fn the_visible_tab_is_the_first_one_that_says_so() {
        let checked = [("A", Some("hidden")), ("B", None), ("C", Some("visible")), ("D", Some("visible"))];
        assert_eq!(pick_visible(&checked), Some("C"));
        assert_eq!(pick_visible(&[("A", Some("hidden")), ("B", None)]), None, "none is visible: the caller takes the first page");
        assert_eq!(pick_visible(&[]), None);
        assert!(looks_like_target_id("1A2B3C4D5E6F708192A3B4C5D6E7F809"));
        assert!(!looks_like_target_id("1a2b3c"), "a word of a title is not an id");
        assert!(!looks_like_target_id("1A2B3C4D5E6F708192A3B4C5D6E7F80G"), "not hex");
        assert!(!looks_like_target_id("1A2B3C4D5E6F708192A3B4C5D6E7F8091"), "too long");
    }
}
