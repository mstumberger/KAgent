//! The test seams that stand in for the owner's screen (`KAGENT_FAKE_SCREENSHOT`,
//! `KAGENT_FAKE_LOCKED`, `KAGENT_FAKE_WINDOW`). They are honoured only in a debug build: in a
//! release build the variable is ignored, with one warning, so an environment that leaks one
//! can never make the daemon show a fake picture or skip the lock check.

use std::sync::Mutex;

/// Names already warned about (one line each).
static WARNED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The value of the seam `name` in a debug build; in a release build always `None`.
pub fn seam(name: &str) -> Option<String> {
    let value = std::env::var(name).ok();
    if value.is_some() && !cfg!(debug_assertions) {
        warn_once(name);
        return None;
    }
    value
}

fn warn_once(name: &str) {
    let mut warned = WARNED.lock().unwrap();
    if !warned.iter().any(|w| w == name) {
        warned.push(name.to_string());
        tracing::warn!("{name} is a test seam and is ignored in a release build");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seam_is_read_in_a_debug_build_only() {
        std::env::set_var("KAGENT_FAKE_SEAM_TEST", "x");
        assert_eq!(seam("KAGENT_FAKE_SEAM_TEST"), if cfg!(debug_assertions) { Some("x".to_string()) } else { None });
        std::env::remove_var("KAGENT_FAKE_SEAM_TEST");
        assert_eq!(seam("KAGENT_FAKE_SEAM_TEST"), None);
    }
}
