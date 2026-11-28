//! How the web tools talk to the web: one `Transport` (the egress first, then a client
//! that pins the addresses the egress resolved and follows no redirect), its `Request`
//! and the `Answer` whose body is read on demand (web.md section 1).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use reqwest::{Method, Url};

use super::guard::{Egress, Refusal};
use super::Fail;

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
