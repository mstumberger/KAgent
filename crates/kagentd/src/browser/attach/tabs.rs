//! Which tab a call acts in, and whose it is (browser.md section 4): the tabs the
//! agent opened in this daemon run (by target id), the default target, and what
//! `assess` told the engine about a call, for the check the call makes when it runs.

use std::time::Instant;

use anyhow::{bail, Result};

use super::super::cdp;
use super::super::target::{looks_like_target_id, pick_visible, TabKind, TabTarget};
use super::{AgentTab, BrowserSession, TargetInfo};

/// Pages looked at for the default target (browser.md section 4).
const VISIBILITY_CANDIDATES: usize = 12;
/// Ids are matched exactly, so tabs the agent opened long ago can be forgotten.
pub(super) const AGENT_TABS_KEEP: usize = 200;
/// The assessments of one batch of calls, kept for the check each call makes when it runs.
const ASSESSED_KEEP: usize = 64;

impl BrowserSession {
    /// The owner's visible tab: of the first 12 pages, the first whose
    /// `document.visibilityState` is `visible` (one evaluation per page).
    pub async fn visible_tab(&self, pages: &[TargetInfo]) -> Option<TargetInfo> {
        let mut checked: Vec<(String, Option<String>)> = Vec::new();
        let mut looked: Vec<TargetInfo> = Vec::new();
        for page in pages.iter().take(VISIBILITY_CANDIDATES) {
            let state = match self.attach(page).await {
                Ok(mut tab) => {
                    let state = tab.eval("document.visibilityState").await.ok().and_then(|v| v.as_str().map(String::from));
                    tab.detach().await;
                    state
                }
                Err(_) => None,
            };
            checked.push((page.target_id.clone(), state));
            looked.push(page.clone());
        }
        let names: Vec<(&str, Option<&str>)> = checked.iter().map(|(id, state)| (id.as_str(), state.as_deref())).collect();
        let id = pick_visible(&names)?;
        looked.into_iter().find(|page| page.target_id == id)
    }

    /// The tab a call acts in: the one `needle` names, else the agent's most recent tab,
    /// else the owner's visible one, else the first page.
    pub async fn resolve(&self, needle: &str) -> Result<(TargetInfo, TabKind)> {
        let pages = self.pages().await?;
        let n = needle.trim();
        let found = match n.is_empty() {
            true if self.recent_agent_tab(&pages).is_some() => self.recent_agent_tab(&pages).cloned(),
            true => self.visible_tab(&pages).await.or_else(|| pages.first().cloned()),
            false if looks_like_target_id(n) => pages.iter().find(|p| p.target_id == n).cloned(),
            _ => {
                let tabs: Vec<cdp::Tab> = pages.iter().map(cdp::Tab::from).collect();
                cdp::pick(&tabs, n).and_then(|t| pages.iter().find(|p| p.target_id == t.id)).cloned()
            }
        };
        let Some(tab) = found else { bail!(if pages.is_empty() { "no open tabs".to_string() } else { format!("no tab matches '{n}'") }) };
        Ok((tab.clone(), self.kind_of(&tab.target_id)))
    }

    /// The agent opened `id` now (the action `open`).
    pub fn note_agent_tab(&self, id: &str, title: &str, url: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.agent_tabs.push_front(AgentTab { id: id.to_string(), title: title.to_string(), url: url.to_string(), used: Instant::now(), seen: true });
    }

    /// The agent acted in a tab it opened: it is the default target from now on.
    pub(crate) fn touch(&self, tab: &TargetInfo) {
        let mut inner = self.inner.lock().unwrap();
        let used = Instant::now();
        if let Some(pos) = inner.agent_tabs.iter().position(|t| t.id == tab.target_id) {
            let mut t = inner.agent_tabs.remove(pos).expect("the tab is at its position");
            (t.used, t.seen, t.title, t.url) = (used, true, tab.title.clone(), tab.url.clone());
            inner.agent_tabs.push_front(t);
        }
    }

    fn kind_of(&self, id: &str) -> TabKind {
        if self.is_agent_tab(id) { TabKind::Agent } else { TabKind::Owner }
    }

    pub fn is_agent_tab(&self, id: &str) -> bool {
        self.inner.lock().unwrap().agent_tabs.iter().any(|t| t.id == id)
    }

    /// The agent's most recently used tab among the pages the browser has now.
    pub fn recent_agent_tab<'a>(&self, pages: &'a [TargetInfo]) -> Option<&'a TargetInfo> {
        let inner = self.inner.lock().unwrap();
        let mut best: Option<(Instant, &'a TargetInfo)> = None;
        for t in inner.agent_tabs.iter().filter(|t| t.seen) {
            if best.is_some_and(|(used, _)| t.used <= used) {
                continue;
            }
            if let Some(page) = pages.iter().find(|p| p.target_id == t.id) {
                best = Some((t.used, page));
            }
        }
        best.map(|(_, page)| page)
    }

    /// What a call will act in, as far as can be told before it runs: an agent tab only
    /// when the needle is empty and one is open, or is exactly the id of one. Part of a
    /// title or a URL may also name one of the owner's tabs: those count as the owner's.
    pub fn assume(&self, needle: &str) -> TabKind {
        let inner = self.inner.lock().unwrap();
        let n = needle.trim();
        let names_one = |t: &AgentTab| if n.is_empty() { t.seen } else { t.seen && t.id == n };
        if inner.agent_tabs.iter().any(names_one) { TabKind::Agent } else { TabKind::Owner }
    }

    /// The engine was shown `kind` for the call with this input; the call checks it when it runs.
    pub fn assessed(&self, key: String, kind: TabKind) {
        let mut inner = self.inner.lock().unwrap();
        inner.assessed.push_front((key, kind));
        while inner.assessed.len() > ASSESSED_KEEP {
            inner.assessed.pop_back();
        }
    }

    pub fn take_assessed(&self, key: &str) -> Option<TabKind> {
        let mut inner = self.inner.lock().unwrap();
        let pos = inner.assessed.iter().position(|(k, _)| k == key)?;
        inner.assessed.remove(pos).map(|(_, kind)| kind)
    }
}
