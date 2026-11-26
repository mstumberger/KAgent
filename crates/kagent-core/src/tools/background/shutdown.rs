//! Daemon exit for the background tasks (agents.md section 4): every task gets TERM, and KILL
//! when its process group is still there 2 s later.

use std::time::{Duration, Instant};

use crate::tools::exec::kill_group;

use super::Background;

impl Background {
    /// Daemon exit (agents.md section 4): every task gets TERM, and KILL if its process
    /// group is still there 2 s later. `stop_all` leaves that second signal to a thread
    /// that dies with the daemon, so here it is sent before the daemon goes. Returns at
    /// once when nothing runs.
    pub async fn shutdown(&self) {
        let groups: Vec<u32> = self.0.tasks.lock().unwrap().values().filter_map(|t| t.state.lock().unwrap().pid).collect();
        self.stop_all();
        let end = Instant::now() + Duration::from_secs(2);
        let mut left = groups;
        while !left.is_empty() && Instant::now() < end {
            left.retain(|pid| group_alive(*pid));
            if !left.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        for pid in left {
            kill_group(pid, "KILL");
        }
    }
}

/// Whether process group `pgid` has a member that still runs (a zombie is gone, it only
/// waits to be reaped). Read from `/proc`: `kill -0` on a group answers "failed" on
/// systems where the same `kill` delivers to it (seen with procps `kill`).
fn group_alive(pgid: u32) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else { return false };
    entries.flatten().any(|entry| {
        let Some(stat) = std::fs::read_to_string(entry.path().join("stat")).ok() else { return false };
        // After the command name in brackets: state, parent pid, process group, ...
        let mut fields = stat.rsplit_once(')').map(|(_, rest)| rest).unwrap_or_default().split_whitespace();
        let state = fields.next().unwrap_or_default();
        fields.nth(1) == Some(pgid.to_string().as_str()) && state != "Z"
    })
}
