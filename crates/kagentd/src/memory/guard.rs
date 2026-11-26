//! The secret guard (memory.md section 4): `memory_save`, and the API with it, refuse
//! content that looks like a secret.

use std::sync::OnceLock;

use regex::Regex;

/// The error of a refused save.
pub const REFUSED: &str = "do not store secrets in memory";

static MARKERS: OnceLock<Regex> = OnceLock::new();
static RUN: OnceLock<Regex> = OnceLock::new();
static URL: OnceLock<Regex> = OnceLock::new();

/// What marks a secret: an API key, an AWS access key id, a GitHub or Slack token, a PEM
/// block, or a named secret (`password: ...`, `token = ...`) followed by 8 or more
/// characters without a space. A key starts at a word boundary, so "task-list" is no key;
/// `sk-` also stands for the `sk_live_` and `sk_test_` keys.
fn markers() -> &'static Regex {
    MARKERS.get_or_init(|| {
        Regex::new(r"(?i)\b(?:sk[-_][a-z0-9_-]{8,}|akia[a-z0-9]{8,}|ghp_[a-z0-9]{8,}|xox[abprs]-[a-z0-9-]{8,}|(?:password|token|secret|api[_-]?key)s?\s*[:=]\s*\S{8,})|-----BEGIN")
            .expect("a valid pattern")
    })
}

fn label(found: &str) -> &'static str {
    let found = found.to_ascii_lowercase();
    if found.starts_with("sk") {
        "an API key"
    } else if found.starts_with("akia") {
        "an AWS access key id"
    } else if found.starts_with("ghp_") {
        "a GitHub token"
    } else if found.starts_with("xox") {
        "a Slack token"
    } else if found.starts_with("-----begin") {
        "a private key block"
    } else {
        "a password or token with its value"
    }
}

/// What in `text` looks like a secret, or `None`. Besides the markers: a run of 32 or
/// more hex or base64 characters, outside a web address (a long path of one is no key).
pub fn find_secret(text: &str) -> Option<&'static str> {
    if let Some(found) = markers().find(text) {
        return Some(label(found.as_str()));
    }
    let run = RUN.get_or_init(|| Regex::new(r"[A-Za-z0-9+/=]{32,}").expect("a valid pattern"));
    let url = URL.get_or_init(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*://").expect("a valid pattern"));
    text.split_whitespace()
        .any(|token| {
            let bare = token.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
            !url.is_match(bare) && run.is_match(token)
        })
        .then_some("a long run of key-like characters")
}

/// For the table of the tests: whether `find_secret` finds anything.
#[cfg(test)]
pub fn is_secret(text: &str) -> bool {
    find_secret(text).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten lines with a secret in them.
    const REFUSED_LINES: [&str; 10] = [
        "OpenAI key sk-abc123def456ghi789jkl012mno",
        "Anthropic key: sk-ant-api03-XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
        "AWS access key AKIAIOSFODNN7EXAMPLE",
        "GitHub token ghp_16CharactersXXXXXXXXXXXXXXXXXXXXXXXXXXXX",
        concat!("Slack bot token xo", "xb-123456789012-1234567890123-AbCdEfGhIjKlMnOp"),
        "-----BEGIN RSA PRIVATE KEY-----",
        "wifi password: hunter2hunter2",
        "router token: 0123456789abcdef0123",
        "the disk id is a3f9c2e1b4d6f807162534475869a1b3c5d7e9f0a2b4c6d8e0f1a3b5c",
        "encoded: QmFzZTY0U3RyaW5nV2l0aExvbmdSdW5PZlNlY3JldA==",
    ];

    /// Six ordinary lines.
    const ORDINARY_LINES: [&str; 6] = [
        "the printer is in the hallway",
        "The router admin user is kagent-ro, read only.",
        "Backups run at 02:30 every night; the NAS is at 192.168.88.10.",
        "The task-list and the disk-usage notes are in ~/Documents.",
        "Ask before changing anything; the token bucket of the API is small.",
        "Docs: https://documentation.mikrotik.com/ReferenceDocumentation/ApiGuide",
    ];

    /// The table of memory.md section 9: 10 secret-like lines refused, 6 ordinary accepted.
    #[test]
    fn the_guard_table() {
        for line in REFUSED_LINES {
            assert!(is_secret(line), "refused: {line}");
        }
        for line in ORDINARY_LINES {
            assert!(!is_secret(line), "accepted: {line}");
        }
        assert!(!is_secret(""), "nothing is no secret");
    }

    /// Scenario S25 item 2, first half: an `sk-...` key is refused, and the error is the one of the spec.
    #[test]
    fn a_key_is_named_in_the_error() {
        assert_eq!(find_secret("the api key is sk-proj-0123456789abcdef"), Some("an API key"));
        assert_eq!(find_secret("Password = 0123456789abcdef0123"), Some("a password or token with its value"));
        assert_eq!(find_secret("-----BEGIN OPENSSH PRIVATE KEY-----"), Some("a private key block"));
        assert_eq!(find_secret("the printer is in the hallway"), None);
        assert_eq!(REFUSED, "do not store secrets in memory");
    }

    #[test]
    fn the_edges_of_the_rules() {
        // A key starts at a word boundary and has key characters after its mark. A hyphen or
        // an underscore follows "sk" (Stripe writes sk_live_ and sk_test_).
        for plain in ["ask-me anything", "the desk-lamp is off", "my sk- key", "risk-assessment done", "a xoxo at the end"] {
            assert!(!is_secret(plain), "{plain}");
        }
        for key in ["\"sk-0123456789abcdef\"", "(sk_live_0123456789)", "AKIAABCDEFGHIJKLMNOP", concat!("xo", "xp-1234567890-abcdefghij")] {
            assert!(is_secret(key), "{key}");
        }
        // A named secret needs 8 or more characters without a space after the mark.
        assert!(!is_secret("password: 1234"));
        assert!(!is_secret("the token: is in the keyring"));
        assert!(!is_secret("the tokenizer: BERT is slow"));
        assert!(is_secret("API_KEY=0123456789abcdef"));
        // A long run outside a web address counts; one inside does not. Links are safe to keep.
        assert!(is_secret("sha256: 63f9c2e1b4d6f807162534475869a1b3c5d7e9f0a2b4c6d8e0f1a3b5c7d9e"));
        assert!(!is_secret("(https://example.com/aVeryLongPathSegmentWithoutAnyPunctuationAtAll)"));
        assert!(!is_secret("see https://example.com/ReferenceDocumentation/GettingStarted/ApiGuide/v2"));
        // Short runs and ordinary identifiers do not: hyphens and underscores are not key characters here.
        assert!(!is_secret("session 550e8400-e29b-41d4-a716-446655440000 and commit 1a2b3c4"));
        assert!(!is_secret("kagent_memory_store_write_private_to_disk"));
    }
}
