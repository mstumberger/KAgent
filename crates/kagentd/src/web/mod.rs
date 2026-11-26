//! The web tools (web.md): `web_fetch` reads a public page as text, `web_search` asks
//! an engine. Both live in the daemon, as the `screen` tool does, and every request
//! goes through the egress first (`guard.rs`).

#[cfg(test)]
mod fake;
mod fetch;
mod guard;
mod html;
pub(crate) mod http;
mod search;
pub(crate) mod settings;

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use futures::StreamExt;
use kagent_core::tool::Tool;
use reqwest::{Method, Url};

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

/// A request of one of the web tools.
pub struct Request<'a> {
    pub method: Method,
    pub url: &'a str,
    /// Added to the URL, encoded.
    pub query: Vec<(&'static str, &'a str)>,
    /// Sent as `application/x-www-form-urlencoded`.
    pub form: Vec<(&'static str, &'a str)>,
    /// Headers beyond the one every request carries (Brave's key is one).
    pub headers: Vec<(&'static str, String)>,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str) -> Self {
        Self::new(Method::GET, url)
    }

    pub fn post_form(url: &'a str, form: Vec<(&'static str, &'a str)>) -> Self {
        Self { form, ..Self::new(Method::POST, url) }
    }

    fn new(method: Method, url: &'a str) -> Self {
        Self { method, url, query: Vec::new(), form: Vec::new(), headers: Vec::new() }
    }
}

/// How the tools talk to the web. Every request goes through the egress first; it
/// carries no cookie and no credential beyond the headers named, follows no redirect
/// by itself (the tools follow by hand, hop by hop), and ends within `timeout`.
#[derive(Clone)]
pub struct Transport {
    pub egress: Arc<dyn Egress>,
    pub user_agent: String,
    pub accept: String,
    pub timeout: Duration,
}

impl Transport {
    /// One request. The addresses the egress resolved are pinned into the client of
    /// this request (`Guard`): the name is not looked up a second time.
    pub async fn send(&self, req: Request<'_>) -> Result<Answer, Fail> {
        let addresses = self.egress.admit(req.url).await.map_err(|refusal| Fail::Refused { refusal, redirected: false })?;
        let url = Url::parse(req.url).map_err(|_| Fail::Refused { refusal: Refusal::Scheme(req.url.chars().take(80).collect()), redirected: false })?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::ACCEPT, reqwest::header::HeaderValue::from_str(&self.accept).map_err(|_| Fail::Network("the Accept header could not be made".into()))?);
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(self.timeout)
            .user_agent(self.user_agent.as_str())
            .default_headers(headers);
        let host = url.host_str().unwrap_or_default().to_string();
        if !addresses.is_empty() {
            builder = builder.resolve_to_addrs(&host, &addresses);
        }
        let client = builder.build().map_err(|e| Fail::Network(format!("the HTTP client could not be made ({e})")))?;
        let mut request = client.request(req.method, url);
        if !req.query.is_empty() {
            request = request.query(&req.query);
        }
        if !req.form.is_empty() {
            request = request.form(&req.form);
        }
        for (name, value) in &req.headers {
            request = request.header(*name, value.as_str());
        }
        let response = request.send().await.map_err(|e| network(e, Some(&host)))?;
        Ok(Answer { status: response.status().as_u16(), headers: response.headers().clone(), response })
    }
}

/// A failure of the connection in a few words (the library's message repeats the URL).
fn network(error: reqwest::Error, host: Option<&str>) -> Fail {
    let who = host.unwrap_or("the host");
    if error.is_timeout() {
        Fail::Network(format!("{who} did not answer in time"))
    } else if error.is_connect() {
        Fail::Network(format!("could not connect to {who}"))
    } else {
        Fail::Network(format!("{who}: {}", error.without_url()))
    }
}

/// What a request came back with. The body is read on demand, so a type the tools do
/// not want costs nothing.
pub struct Answer {
    pub status: u16,
    headers: reqwest::header::HeaderMap,
    response: reqwest::Response,
}

impl Answer {
    /// The type of the `Content-Type`, lowercased and without its parameters (`text/html`); empty when there is none.
    pub fn media_type(&self) -> String {
        media_type(self.content_type().unwrap_or_default())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The length the server announced, when it did.
    pub fn announced_len(&self) -> Option<u64> {
        self.response.content_length()
    }

    /// The whole body, at most `cap` bytes: more is an error, and the stream is dropped there.
    pub async fn body(self, cap: usize) -> Result<Vec<u8>, Fail> {
        if self.response.content_length().is_some_and(|len| len > cap as u64) {
            return Err(Fail::TooLarge(cap));
        }
        let mut body = Vec::new();
        let mut stream = self.response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| network(e, None))?;
            if body.len() + chunk.len() > cap {
                return Err(Fail::TooLarge(cap));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

/// The type of a `Content-Type`, lowercased and without its parameters (`text/html`).
pub fn media_type(content_type: &str) -> String {
    content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase()
}

/// The charset a `Content-Type` names.
pub fn charset_of(content_type: &str) -> Option<String> {
    let at = content_type.to_ascii_lowercase().find("charset=")? + "charset=".len();
    let rest = content_type.get(at..)?;
    let end = rest.find([';', ' ', '\t']).unwrap_or(rest.len());
    Some(rest[..end].trim_matches(|c| c == '"' || c == '\'').to_string())
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
}

impl Default for Engines {
    fn default() -> Self {
        Self { duckduckgo: "https://html.duckduckgo.com/html/".into(), brave: "https://api.search.brave.com/res/v1/web/search".into() }
    }
}

/// The web tools of the daemon: one per process, behind the registry and the route of the key.
pub struct Web {
    settings: SettingsCell,
    keys: Arc<dyn KeyStore>,
    pub transport: Transport,
    pub engines: Engines,
    cache: Cache,
}

impl Web {
    /// The tools as the daemon runs them: the system resolver for the guard, the real
    /// engines and the key store of this home.
    pub fn new(settings: SettingsCell, config_dir: &Path) -> Self {
        Self::with(settings, Arc::new(BraveKeyStore::for_config(config_dir)), Arc::new(Guard::new(Arc::new(SystemResolver))), Engines::default())
    }

    /// The tools over other parts: the key store, the egress and the engines of a test.
    pub fn with(settings: SettingsCell, keys: Arc<dyn KeyStore>, egress: Arc<dyn Egress>, engines: Engines) -> Self {
        Self {
            settings,
            keys,
            transport: Transport { egress, user_agent: format!("KAgent/{}", env!("CARGO_PKG_VERSION")), accept: ACCEPT.into(), timeout: TIMEOUT },
            engines,
            cache: Cache::default(),
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
