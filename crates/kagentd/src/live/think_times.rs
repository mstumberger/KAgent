//! How long the last brain turns took (voice-control.md section 3): the daemon keeps the
//! last 5 in memory, and `GET /api/state` shows their median, so Settings -> Voice can
//! warn when every request waits too long for the Brain.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many durations are kept.
const KEPT: usize = 5;

static TIMES: Mutex<VecDeque<u64>> = Mutex::new(VecDeque::new());

/// A brain turn of a voice session took `ms`.
pub fn record(ms: u64) {
    push(&mut TIMES.lock().unwrap(), ms);
}

/// The median of the kept durations (the upper middle one of an even count), or `None`.
pub fn median_ms() -> Option<u64> {
    median(&TIMES.lock().unwrap())
}

fn push(times: &mut VecDeque<u64>, ms: u64) {
    times.push_back(ms);
    while times.len() > KEPT {
        times.pop_front();
    }
}

fn median(times: &VecDeque<u64>) -> Option<u64> {
    let mut sorted: Vec<u64> = times.iter().copied().collect();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_of_the_last_five() {
        let mut t = VecDeque::new();
        assert_eq!(median(&t), None);
        for ms in [9000, 1000, 2000] {
            push(&mut t, ms);
        }
        assert_eq!(median(&t), Some(2000));
        for ms in [3000, 4500, 5000, 5200] {
            push(&mut t, ms);
        }
        assert_eq!(t.len(), 5);
        assert_eq!(t.front(), Some(&2000), "the oldest are dropped: 9000 and 1000 are gone");
        assert_eq!(median(&t), Some(4500));
    }
}
