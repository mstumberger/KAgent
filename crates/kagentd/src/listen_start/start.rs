//! The start at login (voice.md section 14.2): the conditions, the wait for an unlocked session, and the window
//! opened with the marker.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{login, Plan};
use crate::appwin::Mode;
use crate::AppState;

/// A locked session is waited for this long after the daemon's start.
pub const LOCK_WAIT: Duration = Duration::from_secs(10 * 60);
const LOCK_POLL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Start,
    Wait,
    GiveUp,
}

/// What to do now: `since_start` is the time since the daemon started. Unlocked after the ten minutes is too late.
pub fn lock_step(locked: bool, since_start: Duration) -> Step {
    if since_start > LOCK_WAIT {
        Step::GiveUp
    } else if locked {
        Step::Wait
    } else {
        Step::Start
    }
}

/// The plan for the session, from the settings as they are now; `None` when Listen on start is off.
fn plan_of(st: &AppState) -> Option<Plan> {
    let s = st.settings.read().unwrap();
    let voice = s.voice_config();
    voice.listen_on_start.then(|| Plan { greeting: s.startup.greeting.spoken, idle: Duration::from_secs(u64::from(voice.listen_on_start_idle_minutes) * 60) })
}

/// Daemon start. True when the window was opened with a marker (the usual start of the widget is then not needed).
pub async fn at_daemon_start(st: Arc<AppState>) -> bool {
    let begun = Instant::now();
    if plan_of(&st).is_none() {
        return false;
    }
    let start = login::session_start().await;
    if !login::within_login_window(start, chrono::Local::now().naive_local(), login::LOGIN_WINDOW) {
        tracing::info!("listen on start: not started, the daemon did not start within 5 minutes of the login ({start:?})");
        return false;
    }
    loop {
        match lock_step(crate::screen::live::is_locked().await, begun.elapsed()) {
            Step::Start => break,
            Step::Wait => tokio::time::sleep(LOCK_POLL).await,
            Step::GiveUp => {
                tracing::info!("listen on start: not started, the session stayed locked for 10 minutes");
                return false;
            }
        }
    }
    // Read again: the owner may have switched it off while the screen was locked.
    let Some(plan) = plan_of(&st) else { return false };
    if st.windows.running().await {
        tracing::info!("listen on start: not started, KAgent's window is already open");
        return false;
    }
    let mode = if st.settings.read().unwrap().start_widget() { Mode::Compact } else { Mode::Full };
    let marker = st.listen_start.issue(plan, Instant::now());
    tracing::info!("listen on start: opening the window");
    crate::tray::set_window_with(&st, mode, false, Some(&format!("listen={marker}"))).await;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_session_is_waited_for_ten_minutes_after_the_daemons_start() {
        let s = Duration::from_secs;
        assert_eq!(lock_step(false, s(0)), Step::Start);
        assert_eq!(lock_step(false, s(600)), Step::Start, "unlocked at ten minutes");
        assert_eq!(lock_step(true, s(0)), Step::Wait);
        assert_eq!(lock_step(true, s(600)), Step::Wait);
        assert_eq!(lock_step(true, s(601)), Step::GiveUp);
        assert_eq!(lock_step(false, s(601)), Step::GiveUp, "unlocked too late: not at all");
    }
}
