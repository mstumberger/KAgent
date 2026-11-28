//! The loopback fake the tests of the web tools talk to, and the egress that lets it
//! through and nothing else on this machine.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures::stream;
use serde_json::Value;

use super::guard::{check_host, check_shape, host_of, Egress, Refusal};
use super::page_reader::{PageReader, ReaderError, Rendered};
use super::settings::KeyStore;

/// A name that resolves to the fake: the test of pinned addresses.
pub const ALIAS: &str = "pinned.test";

/// What a path answers.
#[derive(Clone)]
pub struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    /// Sent in chunks without a length: the size is not known before reading.
    chunked: bool,
    /// Never answers: a client has to give up on it.
    never: bool,
}

impl Reply {
    pub fn ok(content_type: &str, body: &[u8]) -> Self {
        Self::with(200, vec![("content-type", content_type.to_string())], body)
    }

    pub fn json(status: u16, body: &Value) -> Self {
        Self::with(status, vec![("content-type", "application/json".into())], body.to_string().as_bytes())
    }

    pub fn redirect(status: u16, location: &str) -> Self {
        Self::with(status, vec![("location", location.to_string())], b"")
    }

    pub fn with(status: u16, headers: Vec<(&'static str, String)>, body: &[u8]) -> Self {
        Self { status, headers, body: body.to_vec(), chunked: false, never: false }
    }

    pub fn chunked(content_type: &str, body: Vec<u8>) -> Self {
        Self { headers: vec![("content-type", content_type.to_string())], body, chunked: true, status: 200, never: false }
    }

    pub const NEVER: Self = Self { status: 0, headers: Vec::new(), body: Vec::new(), chunked: false, never: true };
}

/// One request the fake saw.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    /// The path with its query.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> &str {
        self.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()).unwrap_or_default()
    }
}

#[derive(Clone, Default)]
struct Served {
    routes: Arc<HashMap<String, Reply>>,
    /// What was asked of the egress, in order.
    admitted: Arc<Mutex<Vec<String>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

/// A server on 127.0.0.1, an ephemeral port, for one test.
pub struct Fake {
    pub addr: SocketAddr,
    served: Served,
}

impl Fake {
    /// Answer `routes` (a path and what it says) until the test ends.
    pub async fn start(routes: &[(&str, Reply)]) -> Self {
        let served = Served { routes: Arc::new(routes.iter().map(|(path, reply)| (path.to_string(), reply.clone())).collect()), ..Served::default() };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(serve).with_state(served.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        Self { addr, served }
    }

    /// The address of a path on the fake (`/html/` gives `http://127.0.0.1:<port>/html/`).
    pub fn url(&self, path: &str) -> String {
        format!("http://{self}{path}", self = self.addr)
    }

    /// The egress of a test: the rules are the daemon's, and the one address let
    /// through on this machine is the fake's own (`ALIAS` names it).
    pub fn egress(&self) -> Arc<dyn Egress> {
        Arc::new(ToFake { fake: self.addr, admitted: self.served.admitted.clone() })
    }

    /// What was asked of the egress, in order.
    pub fn admitted(&self) -> Vec<String> {
        self.served.admitted.lock().unwrap().clone()
    }

    /// Every request, in order.
    pub fn requests(&self) -> Vec<Seen> {
        self.served.seen.lock().unwrap().clone()
    }

    /// The requests for a path, in order.
    pub fn seen(&self, path: &str) -> Vec<Seen> {
        self.served.seen.lock().unwrap().iter().filter(|seen| seen.target.split('?').next() == Some(path)).cloned().collect()
    }
}

async fn serve(State(st): State<Served>, req: Request) -> Response {
    let method = req.method().to_string();
    let target = req.uri().path_and_query().map(|t| t.to_string()).unwrap_or_default();
    let headers = req.headers().iter().map(|(key, value)| (key.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned())).collect();
    let body = axum::body::to_bytes(req.into_body(), 4 << 20).await.map(|b| b.to_vec()).unwrap_or_default();
    let path = target.split('?').next().unwrap_or("").to_string();
    st.seen.lock().unwrap().push(Seen { method, target, headers, body });
    let Some(reply) = st.routes.get(&path).cloned() else {
        return (StatusCode::NOT_FOUND, "no such path on the fake").into_response();
    };
    if reply.never {
        return std::future::pending::<Response>().await;
    }
    let Reply { status, headers, body, chunked, .. } = reply;
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    let mut response = if chunked {
        let chunks: Vec<Vec<u8>> = body.chunks(16 * 1024).map(<[u8]>::to_vec).collect();
        (status, Body::from_stream(stream::iter(chunks.into_iter().map(Ok::<_, std::convert::Infallible>)))).into_response()
    } else {
        (status, body).into_response()
    };
    for (name, value) in headers {
        // An empty value takes the header away (a server that names no content type).
        if value.is_empty() {
            response.headers_mut().remove(name);
        } else {
            response.headers_mut().insert(name, HeaderValue::from_str(&value).unwrap());
        }
    }
    response
}

/// The guard as a test uses it: every rule of the daemon's (`check_shape`, then the
/// name and address rules), and one exception, the fake's own address.
struct ToFake {
    fake: SocketAddr,
    admitted: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Egress for ToFake {
    async fn admit(&self, url: &str) -> Result<Vec<SocketAddr>, Refusal> {
        self.admitted.lock().unwrap().push(url.to_string());
        let parsed = check_shape(url)?;
        let host = host_of(&parsed).ok_or(Refusal::NoHost)?;
        let port = parsed.port_or_known_default().unwrap_or(80);
        match host.parse::<std::net::IpAddr>() {
            Ok(ip) if SocketAddr::new(ip, port) == self.fake => Ok(Vec::new()),
            Ok(_) => Err(Refusal::Address(host.parse().unwrap())),
            _ if host == ALIAS => Ok(vec![self.fake]),
            _ => {
                check_host(&host)?;
                Err(Refusal::Resolve(format!("{host}: a test resolves no name")))
            }
        }
    }
}

/// A key store in memory.
#[derive(Default)]
pub struct MemoryKeys(pub Mutex<Option<String>>);

#[async_trait]
impl KeyStore for MemoryKeys {
    async fn get(&self) -> anyhow::Result<Option<String>> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn set(&self, key: &str) -> anyhow::Result<()> {
        *self.0.lock().unwrap() = Some(key.to_string());
        Ok(())
    }

    async fn clear(&self) -> anyhow::Result<()> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

/// A reader that answers what a test says, and keeps the addresses it was asked for.
pub struct FakePageReader {
    pub asked: Mutex<Vec<String>>,
    answer: Mutex<Result<Rendered, ReaderError>>,
}

impl FakePageReader {
    pub fn html(html: &str) -> Arc<Self> {
        Self::answering(Ok(Rendered { url: String::new(), html: html.to_string() }))
    }

    pub fn failing(error: ReaderError) -> Arc<Self> {
        Self::answering(Err(error))
    }

    fn answering(answer: Result<Rendered, ReaderError>) -> Arc<Self> {
        Arc::new(Self { asked: Mutex::new(Vec::new()), answer: Mutex::new(answer) })
    }
}

#[async_trait]
impl PageReader for FakePageReader {
    async fn render(&self, url: &str) -> Result<Rendered, ReaderError> {
        self.asked.lock().unwrap().push(url.to_string());
        self.answer.lock().unwrap().clone().map(|r| Rendered { url: if r.url.is_empty() { url.to_string() } else { r.url }, html: r.html })
    }
}
