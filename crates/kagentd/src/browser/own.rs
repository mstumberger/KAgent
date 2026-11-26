//! The `own` backend: KAgent's own browser window, with its own profile and a
//! localhost DevTools port (tools.md section 14). The tool as it was before S27: a tab
//! is one WebSocket (`cdp::Session`), found through the HTTP endpoints.

use std::time::Duration;

use kagent_core::tool::ToolOutput;
use serde_json::Value;

use super::{cdp, s, valid_url, Browser, BINARIES};

impl Browser {
    fn find_binary(&self) -> Option<String> {
        if !self.binary.is_empty() {
            return kagent_core::tools::xdg::which(&self.binary).map(|p| p.display().to_string());
        }
        BINARIES.iter().find_map(|b| kagent_core::tools::xdg::which(b)).map(|p| p.display().to_string())
    }

    async fn running(&self) -> bool {
        cdp::version(self.port).await.is_ok()
    }

    async fn launch(&self, url: &str) -> anyhow::Result<String> {
        if self.running().await {
            return Ok("browser already running".into());
        }
        let Some(bin) = self.find_binary() else { anyhow::bail!("no Chromium-family browser found (brave-browser, google-chrome, chromium)") };
        std::fs::create_dir_all(&self.profile)?;
        let mut cmd = tokio::process::Command::new(&bin);
        cmd.arg(format!("--remote-debugging-port={}", self.port))
            .arg("--remote-debugging-address=127.0.0.1")
            .arg(format!("--user-data-dir={}", self.profile.display()))
            .args(["--no-first-run", "--no-default-browser-check", "--class=KAgentBrowser"])
            .arg(if valid_url(url) { url } else { "about:blank" })
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd.spawn()?;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if self.running().await {
                return Ok(format!("started {bin} (KAgent profile)"));
            }
        }
        anyhow::bail!("browser started but the DevTools port {} did not open (is another instance using the profile?)", self.port)
    }

    async fn tab(&self, needle: &str) -> anyhow::Result<cdp::Tab> {
        let tabs = cdp::tabs(self.port).await?;
        cdp::pick(&tabs, needle).cloned().ok_or_else(|| anyhow::anyhow!(if tabs.is_empty() { "no open tabs".to_string() } else { format!("no tab matches '{needle}'") }))
    }

    /// KAgent's own window: the tool as it was before S27.
    pub(super) async fn run_own(&self, input: &Value) -> anyhow::Result<ToolOutput> {
        let action = s(input, "action");
        let url = s(input, "url").trim();
        if matches!(action, "status") {
            return Ok(ToolOutput::ok(match cdp::version(self.port).await {
                Ok(v) => format!("running: {} on port {}; {} tabs", s(&v, "Browser"), self.port, cdp::tabs(self.port).await.map(|t| t.len()).unwrap_or(0)),
                Err(_) => format!("not running (launch starts {})", self.find_binary().unwrap_or_else(|| "nothing: no browser found".into())),
            }));
        }
        if matches!(action, "open" | "navigate") && !valid_url(url) {
            return Ok(ToolOutput::error("url must be an http(s) address"));
        }
        if action == "launch" || (matches!(action, "open" | "navigate") && !self.running().await) {
            let msg = self.launch(url).await?;
            if url.is_empty() || action == "launch" {
                return Ok(ToolOutput::ok(msg));
            }
            if action == "open" {
                // The launch already opened the URL in the first tab.
                let tab = self.tab("").await?;
                let mut target = cdp::Session::connect(&tab).await?;
                return Ok(ToolOutput::ok(format!("{msg}\n{}", self.actions.after_load(&mut target, input).await?)));
            }
        }
        if !self.running().await {
            return Ok(ToolOutput::error("KAgent's browser is not running; use action launch (or open with a url)"));
        }
        let needle = s(input, "tab");
        match action {
            "tabs" => {
                let tabs = cdp::tabs(self.port).await?;
                return Ok(ToolOutput::ok(tabs.iter().map(|t| format!("{}  {}  {}", t.id, t.title, t.url)).collect::<Vec<_>>().join("\n")));
            }
            "open" => {
                let t = cdp::new_tab(self.port, url).await?;
                let _ = cdp::activate(self.port, &t.id).await;
                let mut target = cdp::Session::connect(&t).await?;
                return Ok(ToolOutput::ok(format!("opened in tab {}\n{}", t.id, self.actions.after_load(&mut target, input).await?)));
            }
            "activate" => {
                let t = self.tab(needle).await?;
                cdp::activate(self.port, &t.id).await?;
                return Ok(ToolOutput::ok(format!("showing {}", t.title)));
            }
            "close" => {
                let t = self.tab(needle).await?;
                cdp::close(self.port, &t.id).await?;
                return Ok(ToolOutput::ok(format!("closed {}", t.title)));
            }
            _ => {}
        }
        let tab = self.tab(needle).await?;
        let mut target = cdp::Session::connect(&tab).await?;
        self.actions.act(action, input, &mut target).await
    }
}
