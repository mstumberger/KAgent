//! Changing KAgent's speaking voice in a live session (voice-control.md section 2): the
//! voice is part of the setup, so a new voice needs a new connection. The session rolls
//! it at a `turnComplete`, as it does for a `goAway`.

/// The voice to roll to: the configured one, when it is set and not the connection's.
pub(super) fn changed(configured: &str, connected: &str) -> Option<String> {
    let configured = configured.trim();
    (!configured.is_empty() && configured != connected).then(|| configured.to_string())
}

/// A connection made by a voice roll that ended before its setup completed: the server
/// refused the resumption handle with another voice. The session then connects fresh
/// (no handle) and sends the history frames again.
pub(super) fn refused(voice_rolled: bool, set_up: bool) -> bool {
    voice_rolled && !set_up
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_voice_rolls_once_and_an_empty_or_same_one_does_not() {
        assert_eq!(changed("Puck", "Kore").as_deref(), Some("Puck"));
        assert_eq!(changed("Kore", "Kore"), None);
        assert_eq!(changed("  ", "Kore"), None);
        assert_eq!(changed(" Puck ", "Puck"), None, "the stored spelling is trimmed");
    }

    #[test]
    fn only_a_rolled_connection_that_never_set_up_falls_back_to_a_fresh_one() {
        assert!(refused(true, false));
        assert!(!refused(true, true), "set up: an ordinary drop keeps the handle");
        assert!(!refused(false, false), "no voice change: an ordinary drop keeps the handle");
    }
}
