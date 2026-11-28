//! What the web tools need of the reader (web.md section 8): a page rendered by a
//! browser, as HTML. The real reader is `reader::Reader`; a test puts a fake in its place.

use async_trait::async_trait;

use super::guard::Refusal;

/// A page after the browser rendered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Where the browser ended up (after redirects).
    pub url: String,
    pub html: String,
}

/// Why the reader gave no page. Shown to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderError {
    /// Turned off in Settings -> Web.
    Off,
    /// No Chromium-family browser on this machine.
    NoBrowser,
    /// The browser could not be started or driven.
    Failed(String),
    /// The window of the owner is open on the reader's profile.
    WindowOpen,
    /// The page asks for a human check and did not clear in time; the address it ended on.
    Challenge(String),
    /// The egress refused the page, or a redirect of it.
    Refused { refusal: Refusal, redirected: bool },
    /// The main document answered with this status.
    Status(u16),
    TooLarge,
    Timeout,
}

impl std::fmt::Display for ReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Off => write!(f, "the reader is off (Settings -> Web)"),
            Self::NoBrowser => write!(f, "no Chromium-family browser was found (brave-browser, google-chrome, chromium)"),
            Self::Failed(what) => write!(f, "the reader failed: {what}"),
            Self::WindowOpen => write!(f, "the reader's window is open: close it first"),
            Self::Challenge(_) => write!(f, "this site asks for a human check"),
            Self::Refused { refusal, redirected: true } => write!(f, "redirect to a blocked address stopped: {refusal}"),
            Self::Refused { refusal, redirected: false } => write!(f, "refused: {refusal}"),
            Self::Status(code) => write!(f, "the page answered {code}{}", super::reason(*code)),
            Self::TooLarge => write!(f, "the page is larger than {} MB", super::MAX_BODY / (1024 * 1024)),
            Self::Timeout => write!(f, "the page did not finish loading in time"),
        }
    }
}

#[async_trait]
pub trait PageReader: Send + Sync {
    /// Render a page. One page at a time; the egress holds inside the browser.
    async fn render(&self, url: &str) -> Result<Rendered, ReaderError>;
}
