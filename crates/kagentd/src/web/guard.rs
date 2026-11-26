//! The egress guard of the web tools (web.md section 1). The rules are pure; the one
//! step that is not is the lookup of a name (`Resolver`: the system resolver, or a
//! fake in a test). The daemon's own API, the Ollama server and the DevTools port (no
//! authentication, runs scripts) listen on loopback, so the tools connect to public
//! addresses only: before the request, and again before every hop of a redirect.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Url;

/// Why a URL may not be requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not a URL at all, or not `http` or `https`.
    Scheme(String),
    /// A `user:password@` part.
    UserInfo,
    /// No host.
    NoHost,
    /// A name that is local by itself: `localhost`, `.localhost`, `.local`, `.internal`.
    Name(String),
    /// A loopback, private or link-local address: written in the URL, or answered for a name.
    Address(IpAddr),
    /// The name has no address, or the lookup failed.
    Resolve(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const LOCAL: &str = "; the web tools reach public hosts only";
        match self {
            Self::Scheme(raw) => write!(f, "only http and https URLs are fetched, not \"{}\"", raw.trim().chars().take(80).collect::<String>()),
            Self::UserInfo => write!(f, "a \"user:password@\" part is refused{LOCAL}"),
            Self::NoHost => write!(f, "the URL has no host"),
            Self::Name(name) => write!(f, "\"{name}\" is a name of a local network{LOCAL}"),
            Self::Address(ip) => write!(f, "\"{ip}\" is a local or private address{LOCAL}"),
            Self::Resolve(what) => write!(f, "the host could not be resolved: {what}"),
        }
    }
}

/// The rules about the URL itself: `http` or `https`, no `user:password@`, a host. The
/// parser is the one the client uses, so `http://127.1`, `http://2130706433` and
/// `http://0x7f.0.0.1` are read as the loopback address they are.
pub fn check_shape(raw: &str) -> Result<Url, Refusal> {
    let url = Url::parse(raw.trim()).map_err(|_| Refusal::Scheme(raw.to_string()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(Refusal::Scheme(raw.to_string()));
    }
    let authority = raw.split_once("://").map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""));
    if authority.is_some_and(|a| a.contains('@')) || !url.username().is_empty() || url.password().is_some() {
        return Err(Refusal::UserInfo);
    }
    if url.host_str().is_none() {
        return Err(Refusal::NoHost);
    }
    Ok(url)
}

/// The rules that need no lookup (`check_shape`, and the host written as a name or an address).
pub fn check_url(raw: &str) -> Result<Url, Refusal> {
    let url = check_shape(raw)?;
    check_host(&host_of(&url).ok_or(Refusal::NoHost)?)?;
    Ok(url)
}

/// The host of a parsed URL, as `check_host` reads it. An IPv6 address keeps its
/// brackets in `Url::host_str`: without them, or it reads as a name and passes.
pub fn host_of(url: &Url) -> Option<String> {
    url.host_str().map(|host| host.trim_start_matches('[').trim_end_matches(']').to_string())
}

/// A host as `Url::host_str` gives it: a name, or an address (an IPv6 one without its brackets).
pub fn check_host(host: &str) -> Result<(), Refusal> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if address_blocked(ip) { Err(Refusal::Address(ip)) } else { Ok(()) };
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    if name == "localhost" || [".localhost", ".local", ".internal"].iter().any(|end| name.ends_with(end)) {
        return Err(Refusal::Name(name));
    }
    Ok(())
}

/// Whether an address is one the web tools may never connect to (web.md section 1):
/// loopback, private, link-local, unique local, multicast, unspecified, broadcast and
/// shared address space; the same IPv4 ranges inside an IPv6 address (`::ffff:a.b.c.d`).
pub fn address_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let o = a.octets();
            a.is_loopback() || a.is_private() || a.is_link_local() || a.is_multicast() || a.is_broadcast() || a.is_unspecified()
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
        }
        IpAddr::V6(a) => {
            if a.is_loopback() || a.is_unspecified() {
                return true;
            }
            // IPv4-mapped, and the older IPv4-compatible form: the IPv4 address decides.
            if let Some(v4) = a.to_ipv4_mapped().or_else(|| a.to_ipv4()) {
                return address_blocked(IpAddr::V4(v4));
            }
            let s = a.segments();
            (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || (s[0] & 0xff00) == 0xff00
        }
    }
}

/// How a name becomes addresses: the system resolver of the daemon, or a fake in a test.
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>>;
}

pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        Ok(tokio::net::lookup_host((host, port)).await?.collect())
    }
}

/// What the tools ask before every request (`mod.rs`). `Ok` gives the addresses the
/// request may connect to, which are pinned into the client of that request: the name
/// is not looked up a second time, so it cannot answer a local address to the client
/// after a public one to the guard (DNS rebinding). Empty means "as the URL says".
#[async_trait]
pub trait Egress: Send + Sync {
    async fn admit(&self, url: &str) -> Result<Vec<SocketAddr>, Refusal>;
}

/// The guard of the daemon: the rules, then the lookup. One blocked address refuses the name.
pub struct Guard {
    resolver: Arc<dyn Resolver>,
}

impl Guard {
    pub fn new(resolver: Arc<dyn Resolver>) -> Self {
        Self { resolver }
    }

    pub async fn check(&self, raw: &str) -> Result<Vec<SocketAddr>, Refusal> {
        let url = check_url(raw)?;
        let host = host_of(&url).ok_or(Refusal::NoHost)?;
        let port = url.port_or_known_default().unwrap_or(80);
        if host.parse::<IpAddr>().is_ok() {
            return Ok(Vec::new());
        }
        let addresses = self.resolver.resolve(&host, port).await.map_err(|e| Refusal::Resolve(format!("{host}: {e}")))?;
        if addresses.is_empty() {
            return Err(Refusal::Resolve(format!("{host}: no address")));
        }
        for address in &addresses {
            if address_blocked(address.ip()) {
                return Err(Refusal::Address(address.ip()));
            }
        }
        Ok(addresses)
    }
}

#[async_trait]
impl Egress for Guard {
    async fn admit(&self, url: &str) -> Result<Vec<SocketAddr>, Refusal> {
        self.check(url).await
    }
}

/// No rule and no lookup: for an address the owner set as a setting (a SearXNG
/// instance may run on this machine). Never for a URL the model chose: that goes
/// through `Guard`, and a request through `Unchecked` follows no redirect.
pub struct Unchecked;

#[async_trait]
impl Egress for Unchecked {
    async fn admit(&self, _url: &str) -> Result<Vec<SocketAddr>, Refusal> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Scenario S30 item 1: the guard table of web.md section 6 (25 URLs and addresses at least).
    #[test]
    fn public_addresses_pass_and_local_ones_are_refused() {
        let scheme = |u: &'static str| (u, Refusal::Scheme(u.into()));
        let user = |u: &'static str| (u, Refusal::UserInfo);
        let name = |u: &'static str, n: &'static str| (u, Refusal::Name(n.into()));
        let addr = |u: &'static str, ip: std::net::Ipv4Addr| (u, Refusal::Address(IpAddr::V4(ip)));
        let addr6 = |u: &'static str, ip: std::net::Ipv6Addr| (u, Refusal::Address(IpAddr::V6(ip)));

        let public = [
            "https://www.kernel.org/",
            "http://example.com/page?a=1&b=2#top",
            "https://example.com:8443/",
            "https://example.org./",
            "  https://example.com/path  ",
            "https://1.1.1.1/",
            "https://8.8.8.8:53/",
            "http://11.0.0.1/",
            "http://100.63.255.255/",
            "http://100.128.0.1/",
            "http://172.15.255.255/",
            "http://172.32.0.1/",
            "http://192.169.1.1/",
            "http://169.253.1.1/",
            "https://[2606:4700:4700::1111]/",
            "https://[::ffff:8.8.8.8]/",
        ];
        for url in public {
            check_url(url).unwrap_or_else(|e| panic!("{url} must pass: {e}"));
        }

        let local = [
            scheme("ftp://example.com/file"),
            scheme("file:///etc/passwd"),
            scheme("gopher://example.com/"),
            scheme("not a url"),
            scheme("http://"),
            user("http://user:password@example.com/"),
            user("http://user@example.com/"),
            user("http://@example.com/"),
            user("http://example.com@127.0.0.1/"),
            name("http://localhost/", "localhost"),
            name("http://LOCALHOST:7878/api/state", "localhost"),
            name("http://localhost.:7878/", "localhost"),
            name("http://kagent.localhost/", "kagent.localhost"),
            name("http://printer.local/", "printer.local"),
            name("http://service.internal/", "service.internal"),
            // Scenario S30 item 5: the DevTools port, in every way of writing it.
            addr("http://127.0.0.1:9333/json/list", "127.0.0.1".parse().unwrap()),
            addr("http://127.8.8.8/", "127.8.8.8".parse().unwrap()),
            addr("http://2130706433/", "127.0.0.1".parse().unwrap()),
            addr("http://0x7f.1/", "127.0.0.1".parse().unwrap()),
            addr("http://127.1/", "127.0.0.1".parse().unwrap()),
            addr("http://10.1.2.3/", "10.1.2.3".parse().unwrap()),
            addr("http://172.16.0.1/", "172.16.0.1".parse().unwrap()),
            addr("http://172.31.255.254/", "172.31.255.254".parse().unwrap()),
            addr("http://192.168.1.1/", "192.168.1.1".parse().unwrap()),
            addr("http://169.254.169.254/latest/meta-data/", "169.254.169.254".parse().unwrap()),
            addr("http://0.0.0.0/", "0.0.0.0".parse().unwrap()),
            addr("http://255.255.255.255/", "255.255.255.255".parse().unwrap()),
            addr("http://100.64.0.1/", "100.64.0.1".parse().unwrap()),
            addr("http://100.127.255.254/", "100.127.255.254".parse().unwrap()),
            addr("http://224.0.0.1/", "224.0.0.1".parse().unwrap()),
            addr6("http://[::1]/", "::1".parse().unwrap()),
            addr6("http://[::]/", "::".parse().unwrap()),
            addr6("http://[fe80::1]/", "fe80::1".parse().unwrap()),
            addr6("http://[febf:ffff::1]/", "febf:ffff::1".parse().unwrap()),
            addr6("http://[fc00::1]/", "fc00::1".parse().unwrap()),
            addr6("http://[fd12:3456::1]/", "fd12:3456::1".parse().unwrap()),
            addr6("http://[ff02::1]/", "ff02::1".parse().unwrap()),
            // The IPv4 ranges inside an IPv6 address: refused as the address that was written.
            addr6("http://[::ffff:127.0.0.1]/", "::ffff:127.0.0.1".parse().unwrap()),
            addr6("http://[::ffff:192.168.1.5]/", "::ffff:192.168.1.5".parse().unwrap()),
            addr6("http://[::ffff:10.0.0.1]/", "::ffff:10.0.0.1".parse().unwrap()),
            addr6("http://[::ffff:7f00:1]/", "::ffff:7f00:1".parse().unwrap()),
            addr6("http://[::127.0.0.1]/", "::127.0.0.1".parse().unwrap()),
        ];
        for (url, want) in &local {
            match check_url(url) {
                Err(found) => assert_eq!(&found, want, "{url}"),
                Ok(passed) => panic!("{url} passed: {passed:?}"),
            }
        }
        assert_eq!(public.len() + local.len(), 58, "16 public and 42 local: the table of the guard");
        // The refusal the model reads for the DevTools port.
        assert_eq!(
            check_url("http://127.0.0.1:9333/json/list").unwrap_err().to_string(),
            "\"127.0.0.1\" is a local or private address; the web tools reach public hosts only"
        );
    }

    /// The rules without the address check leave a local address through: for an address the owner sets.
    #[test]
    fn the_shape_rules_do_not_look_at_the_address() {
        assert!(check_shape("http://127.0.0.1:8088/").is_ok());
        assert!(check_shape("http://searx.local/").is_ok());
        assert_eq!(check_shape("ftp://127.0.0.1/").unwrap_err(), Refusal::Scheme("ftp://127.0.0.1/".into()));
        assert_eq!(check_shape("http://u:p@127.0.0.1/").unwrap_err(), Refusal::UserInfo);
        assert_eq!(check_shape("searx").unwrap_err(), Refusal::Scheme("searx".into()));
    }

    /// The same addresses for any name.
    struct Fixed(Vec<SocketAddr>);
    /// The lookup fails.
    struct Down;
    /// How many times a lookup was asked.
    struct Counter(AtomicUsize);

    #[async_trait]
    impl Resolver for Fixed {
        async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Ok(self.0.clone())
        }
    }

    #[async_trait]
    impl Resolver for Down {
        async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "name not known"))
        }
    }

    #[async_trait]
    impl Resolver for Counter {
        async fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(vec!["93.184.216.34:443".parse().unwrap()])
        }
    }

    fn addr(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), port)
    }

    /// Scenario S30 item 1: a name that resolves to a blocked address, through the fake resolver.
    #[tokio::test]
    async fn a_name_is_refused_when_any_of_its_addresses_is_blocked() {
        let guard = Guard::new(Arc::new(Fixed(vec![addr("127.0.0.1", 443)])));
        assert_eq!(guard.check("https://evil.example/page").await.unwrap_err(), Refusal::Address("127.0.0.1".parse().unwrap()));

        let guard = Guard::new(Arc::new(Fixed(vec![addr("93.184.216.34", 443), addr("10.0.0.1", 443)])));
        assert_eq!(guard.check("https://evil.example/").await.unwrap_err(), Refusal::Address("10.0.0.1".parse().unwrap()), "one blocked address refuses the name");

        let guard = Guard::new(Arc::new(Fixed(vec![addr("192.168.1.1", 443), addr("93.184.216.34", 443)])));
        assert_eq!(guard.check("https://evil.example/").await.unwrap_err(), Refusal::Address("192.168.1.1".parse().unwrap()), "the first one found is named");

        let guard = Guard::new(Arc::new(Fixed(vec![addr("93.184.216.34", 8443), addr("2606:4700:4700::1111", 8443)])));
        assert_eq!(guard.check("https://example.com:8443/").await.unwrap(), vec![addr("93.184.216.34", 8443), addr("2606:4700:4700::1111", 8443)], "public addresses are given to the request");

        // The rules about the URL come first: a local name or a written address is refused without a lookup.
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let guard = Guard::new(counter.clone());
        assert_eq!(guard.check("http://localhost/").await.unwrap_err(), Refusal::Name("localhost".into()));
        assert_eq!(guard.check("http://[::1]/").await.unwrap_err(), Refusal::Address("::1".parse().unwrap()));
        assert_eq!(guard.check("https://1.1.1.1/").await.unwrap(), Vec::<SocketAddr>::new(), "an address that is written is connected to as written");
        assert_eq!(counter.0.load(Ordering::SeqCst), 0, "nothing was looked up");
    }

    #[tokio::test]
    async fn a_name_without_an_address_or_a_failed_lookup_is_an_error() {
        let guard = Guard::new(Arc::new(Fixed(Vec::new())));
        assert_eq!(guard.check("https://example.com/").await.unwrap_err(), Refusal::Resolve("example.com: no address".into()));
        let guard = Guard::new(Arc::new(Down));
        assert_eq!(guard.check("https://example.com/").await.unwrap_err(), Refusal::Resolve("example.com: name not known".into()));
        let guard = Guard::new(Arc::new(Fixed(vec![addr("93.184.216.34", 443)])));
        assert_eq!(guard.check("http://user:pw@example.com/").await.unwrap_err(), Refusal::UserInfo, "before any lookup");
    }

    #[tokio::test]
    async fn unchecked_admits_any_address() {
        assert_eq!(Unchecked.admit("http://127.0.0.1:9333/json/list").await.unwrap(), Vec::<SocketAddr>::new());
        assert_eq!(Unchecked.admit("ftp://x/").await.unwrap(), Vec::<SocketAddr>::new());
    }
}

