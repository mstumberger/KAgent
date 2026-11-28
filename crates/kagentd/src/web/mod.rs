//! The web tools (web.md): `web_fetch` reads a public page as text, `web_search` asks
//! an engine. Both live in the daemon, as the `screen` tool does, and every request
//! goes through the egress first (`guard.rs`).

#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod fetch;
mod guard;
mod html;
pub(crate) mod http;
pub(crate) mod search;
mod chain;
mod page_reader;
mod transport;
pub use transport::{charset_of, Answer, Request, Transport};
pub(crate) mod reader;
pub(crate) mod settings;

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use kagent_core::tool::Tool;
use reqwest::Url;

use crate::settings::Settings;
use fetch::{Cache, WebFetch};
use guard::{Egress, Guard, Refusal, SystemResolver};
use search::WebSearch;
use settings::{BraveKeyStore, KeyStore, SearchSection};

/// The whole of one fetch or one search, redirects included (web.md sections 2 and 3).
pub const TIMEOUT: Duration = Duration::from_secs(15);
/// One request reads at most this (web.md section 2).
pub const MAX_BODY: usize = 2 * 1024 * 1024;
/// What the tools ask for (web.md section 2).
const ACCEPT: &str = "text/html,application/xhtml+xml,application/json,application/xml,text/*;q=0.9";

/// Why a request did not give what was asked. Shown to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    /// The egress refused an address: the one asked for, or the target of a redirect.
    Refused { refusal: Refusal, redirected: bool },
    /// Not a success (a redirect nobody follows included).
    Status(u16),
    /// More redirects than the tool follows; the last target.
    TooManyRedirects(String),
    /// The whole request took longer than the timeout.
    Timeout,
    /// More than the cap bytes came; the download was stopped there.
    TooLarge(usize),
    /// A type the tools do not read (web.md section 2).
    NotText { media: String, size: String },
    Empty,
    /// Nothing came back.
    Network(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused { refusal, redirected: true } => write!(f, "redirect to a blocked address stopped: {refusal}"),
            Self::Refused { refusal, redirected: false } => write!(f, "refused: {refusal}"),
            Self::Status(code) => write!(f, "the page answered {code}{}", reason(*code)),
            Self::TooManyRedirects(last) => write!(f, "stopped after {} redirects (the last target: {last})", fetch::MAX_REDIRECTS),
            Self::Timeout => write!(f, "the page did not answer within {} s", TIMEOUT.as_secs()),
            Self::TooLarge(cap) => write!(f, "the page is larger than {} MB; the download was stopped at the cap", cap / (1024 * 1024)),
            Self::NotText { media, size } => write!(f, "not a text page ({media}, {size})"),
            Self::Empty => write!(f, "the page is empty"),
            Self::Network(what) => write!(f, "no answer: {what}"),
        }
    }
}

/// The usual name of a status code, for the model.
pub(crate) fn reason(code: u16) -> &'static str {
    match code {
        301 => " Moved Permanently",
        302 => " Found",
        303 => " See Other",
        307 => " Temporary Redirect",
        308 => " Permanent Redirect",
        400 => " Bad Request",
        401 => " Unauthorized",
        403 => " Forbidden",
        404 => " Not Found",
        405 => " Method Not Allowed",
        408 => " Request Timeout",
        410 => " Gone",
        413 => " Content Too Large",
        429 => " Too Many Requests",
        500 => " Internal Server Error",
        502 => " Bad Gateway",
        503 => " Service Unavailable",
        504 => " Gateway Timeout",
        _ => "",
    }
}

/// A number the model sent, kept inside `low..=high`; none gives the default.
pub fn clamp(value: Option<u64>, low: u64, high: u64, default: u64) -> u64 {
    value.map(|v| v.clamp(low, high)).unwrap_or(default)
}

/// A size for the model: `812 bytes`, `14.2 kB`, `2.4 MB`.
pub fn human_len(bytes: u64) -> String {
    match bytes {
        n if n < 1024 => format!("{n} bytes"),
        n if n < 1024 * 1024 => format!("{:.1} kB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// The daemon's settings, shared with the tools once they exist (the cell of the `screen` tool).
pub type SettingsCell = Arc<OnceLock<Arc<RwLock<Settings>>>>;

/// Where the engines are asked: the real ones, or a loopback fake in a test.
#[derive(Clone)]
pub struct Engines {
    /// DuckDuckGo's HTML endpoint (web.md section 3): a form POST, no key.
    pub duckduckgo: String,
    /// Brave's Search API: needs the key.
    pub brave: String,
    /// The page the reader reads as a search (web.md section 9): `?q=` is added.
    pub reader: String,
}

impl Default for Engines {
    fn default() -> Self {
        Self { duckduckgo: "https://html.duckduckgo.com/html/".into(), brave: "https://api.search.brave.com/res/v1/web/search".into(), reader: "https://html.duckduckgo.com/html/".into() }
    }
}

/// The web tools of the daemon: one per process, behind the registry and the route of the key.
pub struct Web {
    settings: SettingsCell,
    keys: Arc<dyn KeyStore>,
    pub transport: Transport,
    pub engines: Engines,
    cache: Cache,
    /// Cache, pacing and cooldowns of the search chain (web.md section 3.1).
    pub(crate) chain: chain::Chain,
    /// The reader, when there is one (web.md section 8).
    reader: Option<Arc<dyn page_reader::PageReader>>,
    /// The reader itself, for the routes of Settings -> Web (web.md section 9).
    pub(crate) reader_admin: Option<Arc<reader::Reader>>,
    /// Where notices go (a human check, web.md section 9).
    events: Option<tokio::sync::broadcast::Sender<kagent_core::protocol::AgentEvent>>,
}

impl Web {
    /// The tools as the daemon runs them: the system resolver for the guard, the real
    /// engines, the key store of this home, and the reader on `<data>/reader-profile`
    /// (it goes through the same guard).
    pub fn new(settings: SettingsCell, config_dir: &Path, data_dir: &Path, events: tokio::sync::broadcast::Sender<kagent_core::protocol::AgentEvent>) -> Self {
        let egress: Arc<dyn Egress> = Arc::new(Guard::new(Arc::new(SystemResolver)));
        let reader = reader::Reader::for_daemon(data_dir, egress.clone());
        Self::with(settings, Arc::new(BraveKeyStore::for_config(config_dir)), egress, Engines::default()).with_reader(reader).with_events(events)
    }

    /// The tools over other parts: the key store, the egress and the engines of a test.
    pub fn with(settings: SettingsCell, keys: Arc<dyn KeyStore>, egress: Arc<dyn Egress>, engines: Engines) -> Self {
        Self {
            settings,
            keys,
            transport: Transport { egress, user_agent: format!("KAgent/{}", env!("CARGO_PKG_VERSION")), accept: ACCEPT.into(), timeout: TIMEOUT },
            engines,
            cache: Cache::default(),
            chain: chain::Chain::default(),
            reader: None,
            reader_admin: None,
            events: None,
        }
    }

    /// The real reader: the tools fall back to it and the routes manage it.
    pub fn with_reader(mut self, reader: Arc<reader::Reader>) -> Self {
        self.reader = Some(reader.clone());
        self.reader_admin = Some(reader);
        self
    }

    /// The reader the tools fall back to (web.md section 8).
    pub fn with_page_reader(mut self, reader: Arc<dyn page_reader::PageReader>) -> Self {
        self.reader = Some(reader);
        self
    }

    pub fn with_events(mut self, events: tokio::sync::broadcast::Sender<kagent_core::protocol::AgentEvent>) -> Self {
        self.events = Some(events);
        self
    }

    /// A page asks for a human check and did not clear: the owner is told, with a button
    /// that opens the reader's window on the page (web.md section 9).
    pub(crate) fn note_challenge(&self, url: &str) {
        let Some(events) = &self.events else { return };
        let host = Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| "A site".into());
        let text = format!("{host} asks for a human check. Open it, pass the check once and close the window: the next read of the site rides that session.");
        let _ = events.send(kagent_core::protocol::AgentEvent::keyed_notice("warn", text, format!("reader:check:{host}")).with_action("Open it", "/api/web/reader/open", serde_json::json!({ "url": url })));
    }

    pub(crate) fn page_reader(&self) -> Option<Arc<dyn page_reader::PageReader>> {
        self.reader.clone()
    }

    /// settings.web, as the tools and the routes read it.
    pub fn web_config(&self) -> reader::config::WebSection {
        self.settings.get().and_then(|cell| cell.read().ok()).map(|s| s.web_config()).unwrap_or_default()
    }

    /// Whether the reader may be used (Settings -> Web).
    pub(crate) fn reader_enabled(&self) -> bool {
        self.web_config().reader
    }

    /// A page rendered by the reader, as the text the tool gives. The reader is off, or
    /// there is none: an error that says so.
    pub(crate) async fn read_rendered(&self, url: &str) -> Result<(String, fetch::Fetched), page_reader::ReaderError> {
        use page_reader::ReaderError;
        if !self.reader_enabled() {
            return Err(ReaderError::Off);
        }
        let reader = self.page_reader().ok_or(ReaderError::NoBrowser)?;
        let page = reader.render(url).await?;
        let base = Url::parse(&page.url).map_err(|_| ReaderError::Failed("the address of the page could not be read".into()))?;
        Ok((page.url.clone(), fetch::Fetched::Html(html::to_text(&page.html, &base))))
    }

    /// Close the reader's browser: the daemon is stopping.
    pub async fn shutdown(&self) {
        if let Some(reader) = &self.reader_admin {
            reader.shutdown().await;
        }
    }

    pub fn fetch_tool(self: &Arc<Self>) -> Arc<dyn Tool> {
        Arc::new(WebFetch::new(self.clone()))
    }

    pub fn search_tool(self: &Arc<Self>) -> Arc<dyn Tool> {
        Arc::new(WebSearch::new(self.clone()))
    }

    /// settings.search, as the tool and the routes read it.
    pub fn search_config(&self) -> SearchSection {
        self.settings.get().and_then(|cell| cell.read().ok()).map(|s| s.search_config()).unwrap_or_default()
    }

    pub async fn brave_key(&self) -> anyhow::Result<Option<String>> {
        self.keys.get().await
    }

    pub async fn set_brave_key(&self, key: &str) -> anyhow::Result<()> {
        self.keys.set(key).await
    }

    pub async fn clear_brave_key(&self) -> anyhow::Result<()> {
        self.keys.clear().await
    }
}

#[cfg(test)]
mod tests {
    use super::transport::media_type;
    use super::*;

    #[test]
    fn the_headers_of_a_content_type_are_read() {
        assert_eq!(media_type("Text/HTML; Charset=UTF-8"), "text/html");
        assert_eq!(media_type(""), "");
        assert_eq!(charset_of("text/html; charset=windows-1250"), Some("windows-1250".into()));
        assert_eq!(charset_of("text/html;charset=\"ISO-8859-1\""), Some("ISO-8859-1".into()));
        assert_eq!(charset_of("text/html"), None);
    }

    #[test]
    fn the_failures_say_what_happened() {
        let refused = Fail::Refused { refusal: Refusal::Address("127.0.0.1".parse().unwrap()), redirected: false };
        let redirected = Fail::Refused { refusal: Refusal::Address("127.0.0.1".parse().unwrap()), redirected: true };
        assert_eq!(refused.to_string(), "refused: \"127.0.0.1\" is a local or private address; the web tools reach public hosts only");
        assert_eq!(redirected.to_string(), "redirect to a blocked address stopped: \"127.0.0.1\" is a local or private address; the web tools reach public hosts only");
        assert_eq!(Fail::Status(404).to_string(), "the page answered 404 Not Found");
        assert_eq!(Fail::Status(599).to_string(), "the page answered 599");
        assert_eq!(Fail::TooManyRedirects("https://a.example/".into()).to_string(), "stopped after 5 redirects (the last target: https://a.example/)");
        assert_eq!(Fail::Timeout.to_string(), "the page did not answer within 15 s");
        assert_eq!(Fail::TooLarge(MAX_BODY).to_string(), "the page is larger than 2 MB; the download was stopped at the cap");
        assert_eq!(Fail::NotText { media: "image/png".into(), size: "14.2 kB".into() }.to_string(), "not a text page (image/png, 14.2 kB)");
        assert_eq!(Fail::Empty.to_string(), "the page is empty");
        assert_eq!(Fail::Network("1.1.1.1 did not answer in time".into()).to_string(), "no answer: 1.1.1.1 did not answer in time");
    }

    #[test]
    fn numbers_are_kept_in_their_range() {
        assert_eq!(clamp(None, 1, 10, 5), 5);
        assert_eq!(clamp(Some(0), 1, 10, 5), 1);
        assert_eq!(clamp(Some(99), 1, 10, 5), 10);
        assert_eq!(clamp(Some(7), 1, 10, 5), 7);
        assert_eq!(clamp(Some(600), 1000, 20_000, 6000), 1000);
        assert_eq!((human_len(812), human_len(14_600), human_len(2_490_000)), ("812 bytes".into(), "14.3 kB".into(), "2.4 MB".into()));
    }
}
