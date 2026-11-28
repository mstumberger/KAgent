//! settings.web (web.md section 8): whether the reader may run, and the sites it is
//! always used for. A protected key: it decides where pages are read from.

use serde::{Deserialize, Serialize};

use crate::sections::SectionError;
use crate::settings::Settings;

/// At most this many sites, of at most this many characters.
pub const MAX_SITES: usize = 50;
const MAX_HOST: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WebSection {
    /// The reader may run (the fallback of `web_fetch`, `render: true`, the last engine of the search chain).
    pub reader: bool,
    /// Hosts `web_fetch` always reads through the reader (a subdomain of one counts).
    pub reader_sites: Vec<String>,
}

impl Default for WebSection {
    fn default() -> Self {
        Self { reader: true, reader_sites: Vec::new() }
    }
}

impl WebSection {
    pub fn validate(&self) -> Result<(), SectionError> {
        let bad = |message: String| SectionError::Invalid { key: "web".into(), message };
        if self.reader_sites.len() > MAX_SITES {
            return Err(bad(format!("readerSites holds at most {MAX_SITES} sites")));
        }
        for site in &self.reader_sites {
            if !is_host(site) {
                return Err(bad(format!("readerSites: \"{site}\" is not a host name such as news.example.com (no scheme, path or port)")));
            }
        }
        Ok(())
    }

    /// Whether `host` is on the list: a listed name or a subdomain of one.
    pub fn is_reader_site(&self, host: &str) -> bool {
        self.reader_sites.iter().any(|site| host_matches(host, site))
    }
}

/// `host` is `site` or a subdomain of it (names compared without case or a final dot).
pub fn host_matches(host: &str, site: &str) -> bool {
    let (host, site) = (host.trim_end_matches('.').to_ascii_lowercase(), site.trim_end_matches('.').to_ascii_lowercase());
    !site.is_empty() && (host == site || host.ends_with(&format!(".{site}")))
}

/// A name of letters, digits, dots and hyphens, lowercase, with at least one dot: nothing that is a scheme, a port or a path.
pub fn is_host(site: &str) -> bool {
    !site.is_empty()
        && site.len() <= MAX_HOST
        && site.contains('.')
        && !site.starts_with(['.', '-'])
        && !site.ends_with(['.', '-'])
        && site.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

impl Settings {
    /// settings.web.
    pub fn web_config(&self) -> WebSection {
        self.section::<WebSection>("web").unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_section_has_defaults_and_refuses_what_is_not_a_host() {
        let mut s = Settings::defaults();
        assert_eq!(s.web_config(), WebSection { reader: true, reader_sites: vec![] });
        assert_eq!(serde_json::to_value(WebSection::default()).unwrap(), json!({"reader": true, "readerSites": []}));
        s.patch(json!({"web": {"readerSites": ["news.example.com", "example.org"]}})).unwrap();
        assert!(s.web_config().is_reader_site("news.example.com") && s.web_config().is_reader_site("www.example.org"));
        assert!(!s.web_config().is_reader_site("notexample.org") && !s.web_config().is_reader_site("example.com"));
        for bad in ["https://example.com", "example.com/path", "Example.com", "localhost", "a b.com", "example.com:80", ".example.com", ""] {
            assert!(s.patch(json!({"web": {"readerSites": [bad]}})).is_err(), "{bad:?}");
        }
        assert!(s.patch(json!({"web": {"reader": "yes"}})).is_err());
        assert!(s.patch(json!({"web": {"readr": false}})).unwrap_err().contains("did you mean \"reader\"?"));
        s.patch(json!({"web": {"reader": false}})).unwrap();
        assert!(!s.web_config().reader && s.web_config().reader_sites.len() == 2, "a missing field keeps what the file has");
    }
}
