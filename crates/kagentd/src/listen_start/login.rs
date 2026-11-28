//! Whether the daemon started just after the owner logged in (voice.md section 14.2). The start of the
//! graphical session comes from logind; the decision is a pure function of its text and the clock.

use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};

/// "After login" means the daemon started within this long of the session's start.
pub const LOGIN_WINDOW: Duration = Duration::from_secs(5 * 60);

/// The session start from `loginctl show-session <id> -p Timestamp --value` (`Sat 2026-11-28 08:15:02 CET`,
/// the time zone of the machine, which the clock in use shares). `None` for `n/a`, empty text or anything else.
pub fn parse_session_start(text: &str) -> Option<NaiveDateTime> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let at = words.iter().position(|w| NaiveDate::parse_from_str(w, "%Y-%m-%d").is_ok())?;
    let date = NaiveDate::parse_from_str(words[at], "%Y-%m-%d").ok()?;
    let time = NaiveTime::parse_from_str(words.get(at + 1)?, "%H:%M:%S%.f").ok()?;
    Some(date.and_time(time))
}

/// The daemon started `now`, the session at `start`: after login, and not a restart during the day. Without a start
/// time, or with one in the future, it is not.
pub fn within_login_window(start: Option<NaiveDateTime>, now: NaiveDateTime, window: Duration) -> bool {
    let Some(start) = start else { return false };
    let Ok(age) = (now - start).to_std() else { return false };
    age <= window
}

/// The id of a session from `loginctl show-user -p Display --value`: a word of letters and digits, else none.
pub fn session_id(text: &str) -> Option<String> {
    let id = text.trim();
    (!id.is_empty() && id.len() <= 16 && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| id.to_string())
}

async fn loginctl(args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new("loginctl").args(args).output().await.ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The start of the owner's graphical session: the one in `XDG_SESSION_ID`, else the user's `Display` session.
pub async fn session_start() -> Option<NaiveDateTime> {
    let id = match std::env::var("XDG_SESSION_ID").ok().and_then(|v| session_id(&v)) {
        Some(id) => id,
        None => {
            let uid = std::fs::metadata("/proc/self").ok().map(|m| std::os::unix::fs::MetadataExt::uid(&m))?.to_string();
            session_id(&loginctl(&["show-user", &uid, "-p", "Display", "--value"]).await?)?
        }
    };
    parse_session_start(&loginctl(&["show-session", &id, "-p", "Timestamp", "--value"]).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    #[test]
    fn the_session_start_is_read_from_what_loginctl_prints() {
        for (text, want) in [
            ("Sat 2026-11-28 08:15:02 CET\n", Some(at("2026-11-28 08:15:02"))),
            ("Sat 2026-11-28 08:15:02 CET", Some(at("2026-11-28 08:15:02"))),
            ("Mon 2026-08-03 23:59:59 CEST", Some(at("2026-08-03 23:59:59"))),
            ("Sat 2026-11-28 08:15:02.123456 UTC", Some(at("2026-11-28 08:15:02") + chrono::Duration::microseconds(123_456))),
            ("2026-11-28 08:15:02", Some(at("2026-11-28 08:15:02"))),
            ("n/a", None),
            ("", None),
            ("\n", None),
            ("Sat 28 Nov 2026", None),
            ("Sat 2026-11-28", None),
            ("Sat 2026-11-28 25:61:00 CET", None),
        ] {
            assert_eq!(parse_session_start(text), want, "{text:?}");
        }
    }

    #[test]
    fn after_login_means_within_five_minutes_of_the_session_start() {
        let start = Some(at("2026-11-28 08:15:02"));
        for (now, want) in [
            ("2026-11-28 08:15:02", true),
            ("2026-11-28 08:15:30", true),
            ("2026-11-28 08:20:01", true),
            ("2026-11-28 08:20:02", true),
            ("2026-11-28 08:20:03", false),
            ("2026-11-28 08:21:00", false),
            ("2026-11-28 11:00:00", false),
            ("2026-11-29 08:15:30", false),
            ("2026-11-28 08:15:01", false),
        ] {
            assert_eq!(within_login_window(start, at(now), LOGIN_WINDOW), want, "{now}");
        }
        // Across midnight.
        assert!(within_login_window(Some(at("2026-11-28 23:58:00")), at("2026-11-29 00:01:00"), LOGIN_WINDOW));
        assert!(!within_login_window(Some(at("2026-11-28 23:50:00")), at("2026-11-29 00:01:00"), LOGIN_WINDOW));
        // When it cannot be told, it does not start.
        assert!(!within_login_window(None, at("2026-11-28 08:15:30"), LOGIN_WINDOW));
    }

    #[test]
    fn a_session_id_is_a_short_word() {
        assert_eq!(session_id("2\n"), Some("2".into()));
        assert_eq!(session_id("c1"), Some("c1".into()));
        for bad in ["", "\n", "a b", "../x", "1;rm", "this-is-not-an-id", "12345678901234567"] {
            assert_eq!(session_id(bad), None, "{bad:?}");
        }
    }
}
