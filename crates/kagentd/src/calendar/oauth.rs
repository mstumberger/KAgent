//! OAuth with the owner's own client (calendar.md section 1): authorization code with
//! PKCE and a loopback redirect. Pure helpers (the URL, the challenge, the `state` book)
//! and the three calls to Google (exchange, refresh, revoke). Nothing here logs or returns
//! a secret: an error names what Google said, with every secret taken out.

use std::collections::HashMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::Hosts;

/// The two scopes, and nothing else (calendar.md section 1).
pub const SCOPES: [&str; 2] = ["https://www.googleapis.com/auth/calendar.events", "https://www.googleapis.com/auth/calendar.calendarlist.readonly"];

/// How long a `state` is good for.
pub const STATE_TTL: Duration = Duration::minutes(10);

/// 32 random bytes, URL-safe: a `state`, or the seed of a verifier.
fn random_token(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

/// The PKCE pair (RFC 7636, method S256): a 43..128 character verifier and its challenge.
pub fn pkce() -> (String, String) {
    let verifier = random_token(48);
    let challenge = challenge_of(&verifier);
    (verifier, challenge)
}

pub fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The page Google asks the owner to approve on.
pub fn auth_url(hosts: &Hosts, client_id: &str, redirect: &str, state: &str, challenge: &str) -> String {
    let query = [
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("response_type", "code"),
        ("scope", &SCOPES.join(" ")),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        // A refresh token is only handed out with offline access and a consent screen.
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let encoded: Vec<String> = query.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    format!("{}?{}", hosts.auth, encoded.join("&"))
}

pub(crate) fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The `state` values that wait for their callback: single use, ten minutes.
#[derive(Default)]
pub struct Pending {
    waiting: HashMap<String, (String, DateTime<Utc>)>,
}

impl Pending {
    /// A new `state` and the PKCE challenge that goes with it; the verifier is kept here.
    pub fn begin(&mut self, now: DateTime<Utc>) -> (String, String) {
        self.waiting.retain(|_, (_, at)| now - *at <= STATE_TTL);
        let state = random_token(32);
        let (verifier, challenge) = pkce();
        self.waiting.insert(state.clone(), (verifier, now));
        (state, challenge)
    }

    /// The verifier of a `state`, which then is used up. `None`: unknown, used or too old.
    pub fn take(&mut self, state: &str, now: DateTime<Utc>) -> Option<String> {
        let (verifier, at) = self.waiting.remove(state)?;
        (now - at <= STATE_TTL).then_some(verifier)
    }

    /// Whether a `state` is still waiting (the connection says `connecting`).
    pub fn any(&self, now: DateTime<Utc>) -> bool {
        self.waiting.values().any(|(_, at)| now - *at <= STATE_TTL)
    }
}

/// What Google gave back for a code.
#[derive(Debug, Clone)]
pub struct Granted {
    pub access: String,
    pub expires_in: i64,
    pub refresh: Option<String>,
}

/// Why a token call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenError {
    /// The refresh token is no longer good (revoked, expired in "Testing", password changed).
    InvalidGrant,
    /// Anything else, in words, without a secret.
    Other(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidGrant => write!(f, "Google no longer accepts this connection (invalid_grant): reconnect in Settings -> Calendar"),
            Self::Other(what) => write!(f, "{what}"),
        }
    }
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Take every secret out of a text that may repeat one.
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    secrets.iter().filter(|s| s.len() >= 6).fold(text.to_string(), |t, s| t.replace(s, "<secret>"))
}

async fn token_call(http: &reqwest::Client, hosts: &Hosts, form: &[(&str, &str)], secrets: &[&str]) -> Result<Granted, TokenError> {
    let response = http.post(&hosts.token).form(form).send().await.map_err(|e| TokenError::Other(scrub(&format!("Google's sign-in did not answer: {}", e.without_url()), secrets)))?;
    let status = response.status();
    let body: TokenBody = response.json().await.map_err(|_| TokenError::Other(format!("Google's sign-in answered {status} with something that is not a token answer")))?;
    if let Some(error) = body.error {
        if error == "invalid_grant" {
            return Err(TokenError::InvalidGrant);
        }
        let detail = body.error_description.unwrap_or_default();
        return Err(TokenError::Other(scrub(&format!("Google refused the sign-in: {error} {detail}").trim().to_string(), secrets)));
    }
    match (status.is_success(), body.access_token) {
        (true, Some(access)) => Ok(Granted { access, expires_in: body.expires_in.unwrap_or(3600), refresh: body.refresh_token }),
        _ => Err(TokenError::Other(format!("Google's sign-in answered {status} without an access token"))),
    }
}

/// The code of the callback for tokens.
pub async fn exchange(http: &reqwest::Client, hosts: &Hosts, client_id: &str, client_secret: &str, code: &str, verifier: &str, redirect: &str) -> Result<Granted, TokenError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", verifier),
        ("redirect_uri", redirect),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ];
    token_call(http, hosts, &form, &[client_secret, code, verifier]).await
}

/// A new access token for a refresh token.
pub async fn refresh(http: &reqwest::Client, hosts: &Hosts, client_id: &str, client_secret: &str, refresh_token: &str) -> Result<Granted, TokenError> {
    let form = [("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id), ("client_secret", client_secret)];
    token_call(http, hosts, &form, &[client_secret, refresh_token]).await
}

/// Tell Google to drop a token. A failure is the caller's to log: the local copy goes anyway.
pub async fn revoke(http: &reqwest::Client, hosts: &Hosts, token: &str) -> Result<(), String> {
    let response = http.post(&hosts.revoke).form(&[("token", token)]).send().await.map_err(|e| scrub(&e.without_url().to_string(), &[token]))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("Google answered {} to the revoke", response.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Hosts {
        Hosts::real()
    }

    #[test]
    fn the_url_has_the_scopes_the_challenge_and_the_loopback_redirect() {
        let (state, challenge) = Pending::default().begin(Utc::now());
        let url = auth_url(&hosts(), "id-1.apps.googleusercontent.com", "http://127.0.0.1:7878/api/calendar/oauth/callback", &state, &challenge);
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"), "{url}");
        for part in [
            "client_id=id-1.apps.googleusercontent.com",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A7878%2Fapi%2Fcalendar%2Foauth%2Fcallback",
            "response_type=code",
            "scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.events%20https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.calendarlist.readonly",
            "code_challenge_method=S256",
            "access_type=offline",
            &format!("state={state}"),
            &format!("code_challenge={challenge}"),
        ] {
            assert!(url.contains(part), "{part} in {url}");
        }
        assert!(!url.contains("client_secret") && !url.contains("calendar.readonly") && !url.contains("auth/drive"), "only the two scopes, no secret: {url}");
    }

    #[test]
    fn the_challenge_is_the_s256_of_the_verifier() {
        // The example of RFC 7636 appendix B.
        assert_eq!(challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let (v, c) = pkce();
        assert!((43..=128).contains(&v.len()), "{}", v.len());
        assert_eq!(challenge_of(&v), c);
    }

    #[test]
    fn a_state_is_single_use_and_ten_minutes() {
        let t0 = Utc::now();
        let mut pending = Pending::default();
        let (state, _) = pending.begin(t0);
        assert_eq!(state.len(), 43, "32 random bytes");
        assert!(pending.any(t0));
        assert!(pending.take("unknown", t0).is_none(), "an unknown state");
        assert!(pending.take(&state, t0 + Duration::minutes(5)).is_some(), "the right one, in time");
        assert!(pending.take(&state, t0 + Duration::minutes(5)).is_none(), "used once");
        let (old, _) = pending.begin(t0);
        assert!(pending.take(&old, t0 + Duration::minutes(10) + Duration::seconds(1)).is_none(), "too old");
        assert!(!pending.any(t0 + Duration::hours(1)));
        let (a, _) = pending.begin(t0);
        let (b, _) = pending.begin(t0);
        assert_ne!(a, b);
    }

    #[test]
    fn a_secret_is_taken_out_of_a_text() {
        assert_eq!(scrub("bad secret GOCSPX-abcdef123 here", &["GOCSPX-abcdef123"]), "bad secret <secret> here");
        assert_eq!(scrub("short ab stays", &["ab"]), "short ab stays", "a very short value is not a secret worth cutting");
    }
}
