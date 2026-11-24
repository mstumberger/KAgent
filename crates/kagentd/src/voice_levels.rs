//! Voice levels hub: the single source of "who is speaking and how loud" for every
//! visualizer (KDE plasmoid, overlay orb). The live voice proxy (S10) feeds it the PCM
//! it already relays; activity (thinking, tool, approval) comes from AgentEvents.
//!
//! Consumers poll `GET /api/voice/levels?since=<seq>` (the plasmoid has no WebSocket
//! module available), so frames are kept in a small ring with sequence numbers.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use kagent_core::protocol::AgentEvent;

/// Envelope points per frame. A frame is one audio chunk (typically 20-100 ms) split
/// into this many segments; each value is that segment's loudness in 0..1.
pub const BANDS: usize = 16;
const RING: usize = 512;
const MAX_FRAMES_PER_POLL: usize = 96;
/// A voice counts as "speaking" while frames for it are this recent.
const SPEAKING_WINDOW: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelFrame {
    pub seq: u64,
    pub t_ms: u64,
    pub who: Who,
    pub rms: f32,
    pub bands: [f32; BANDS],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Activity {
    #[default]
    Idle,
    Thinking,
    Tool,
    Approval,
}

struct Inner {
    seq: u64,
    frames: VecDeque<LevelFrame>,
    activity: Activity,
    detail: String,
    last_user: Option<Instant>,
    last_assistant: Option<Instant>,
    running_turns: usize,
}

pub struct LevelHub {
    started: Instant,
    inner: Mutex<Inner>,
}

/// Loudness of PCM16 samples in 0..1, perceptually compressed (sqrt) so quiet speech
/// is still visible.
fn loudness(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    let rms = (sum / samples.len() as f64).sqrt() / 32768.0;
    ((rms * 4.0).sqrt() as f32).min(1.0)
}

pub fn envelope(samples: &[i16]) -> [f32; BANDS] {
    let mut out = [0.0; BANDS];
    if samples.is_empty() {
        return out;
    }
    let seg = samples.len().div_ceil(BANDS);
    for (i, chunk) in samples.chunks(seg).enumerate().take(BANDS) {
        out[i] = loudness(chunk);
    }
    out
}

impl Default for LevelHub {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelHub {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            inner: Mutex::new(Inner {
                seq: 0,
                frames: VecDeque::with_capacity(RING),
                activity: Activity::Idle,
                detail: String::new(),
                last_user: None,
                last_assistant: None,
                running_turns: 0,
            }),
        }
    }

    /// Feed one chunk of mono PCM16 audio for a speaker.
    pub fn feed_pcm16(&self, who: Who, samples: &[i16]) {
        self.push(who, loudness(samples), envelope(samples));
    }

    fn push(&self, who: Who, rms: f32, bands: [f32; BANDS]) {
        let now = Instant::now();
        let t_ms = now.duration_since(self.started).as_millis() as u64;
        let mut g = self.inner.lock().unwrap();
        g.seq += 1;
        let seq = g.seq;
        if g.frames.len() == RING {
            g.frames.pop_front();
        }
        g.frames.push_back(LevelFrame { seq, t_ms, who, rms, bands });
        match who {
            Who::User => g.last_user = Some(now),
            Who::Assistant => g.last_assistant = Some(now),
        }
    }

    /// Track agent activity from the event stream.
    pub fn on_event(&self, ev: &AgentEvent) {
        let mut g = self.inner.lock().unwrap();
        match ev {
            AgentEvent::TurnStarted { .. } => {
                g.running_turns += 1;
                g.activity = Activity::Thinking;
                g.detail.clear();
            }
            AgentEvent::ToolCallStarted { name, .. } => {
                g.activity = Activity::Tool;
                g.detail = name.clone();
            }
            AgentEvent::ToolCallFinished { .. } | AgentEvent::ApprovalResolved { .. } => {
                if g.running_turns > 0 {
                    g.activity = Activity::Thinking;
                    g.detail.clear();
                }
            }
            AgentEvent::ApprovalRequested(req) => {
                g.activity = Activity::Approval;
                g.detail = req.summary.clone();
            }
            AgentEvent::TurnFinished { .. } => {
                g.running_turns = g.running_turns.saturating_sub(1);
                if g.running_turns == 0 {
                    g.activity = Activity::Idle;
                    g.detail.clear();
                }
            }
            _ => {}
        }
    }

    /// State + frames newer than `since`. `state` is `speaking` / `listening` while a
    /// voice is active, otherwise the agent activity.
    pub fn snapshot(&self, since: u64) -> Value {
        let g = self.inner.lock().unwrap();
        let recent = |t: Option<Instant>| t.is_some_and(|t| t.elapsed() < SPEAKING_WINDOW);
        let state = if recent(g.last_assistant) {
            "speaking".to_string()
        } else if recent(g.last_user) {
            "listening".to_string()
        } else {
            serde_json::to_value(g.activity).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
        };
        // A client that fell behind (or restarted) gets only the newest frames.
        let frames: Vec<&LevelFrame> = g.frames.iter().filter(|f| f.seq > since).collect();
        let skip = frames.len().saturating_sub(MAX_FRAMES_PER_POLL);
        json!({
            "seq": g.seq,
            "state": state,
            "detail": g.detail,
            "bands": BANDS,
            "frames": frames.into_iter().skip(skip).collect::<Vec<_>>(),
        })
    }

    /// Synthetic speech-like levels for testing visualizers without voice: ~3 s of the
    /// assistant talking, a pause, then ~2 s of the user. 50 ms frames.
    pub async fn run_demo(&self) {
        let plan = [(Who::Assistant, 60usize), (Who::User, 0), (Who::User, 40)];
        let mut phase = 0.0f32;
        for (who, frames) in plan {
            if frames == 0 {
                tokio::time::sleep(Duration::from_millis(600)).await;
                continue;
            }
            for f in 0..frames {
                let mut bands = [0.0; BANDS];
                for (i, b) in bands.iter_mut().enumerate() {
                    let t = (f * BANDS + i) as f32 / BANDS as f32; // in frames
                    phase += 0.37;
                    // Syllables at ~4.5 Hz (one every ~4.4 frames) with jitter.
                    let syllable = (t * std::f32::consts::TAU / 4.4).sin().max(0.0).powf(0.7);
                    let jitter = 0.75 + 0.25 * (phase * 2.3).sin() * (phase * 0.71).cos();
                    let word_gap = if (t / 13.0).fract() > 0.82 { 0.15 } else { 1.0 };
                    *b = (syllable * jitter * word_gap).clamp(0.0, 1.0);
                }
                let rms = bands.iter().sum::<f32>() / BANDS as f32;
                self.push(who, rms, bands);
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::protocol::{TurnStatus, Usage, Via};

    #[test]
    fn envelope_of_silence_and_tone() {
        assert_eq!(envelope(&[0i16; 800]), [0.0; BANDS]);
        let loud: Vec<i16> = (0..800).map(|i| if i % 2 == 0 { 16000 } else { -16000 }).collect();
        let e = envelope(&loud);
        assert!(e.iter().all(|&v| v > 0.9 && v <= 1.0), "{e:?}");
        // Short buffers do not panic and fill the leading segments.
        assert!(envelope(&[1000, -1000, 1000])[0] > 0.0);
    }

    #[test]
    fn snapshot_reports_speaking_then_activity() {
        let hub = LevelHub::new();
        hub.on_event(&AgentEvent::TurnStarted { session_id: "s".into(), turn_id: "t".into(), input: "x".into(), via: Via::Text });
        assert_eq!(hub.snapshot(0)["state"], "thinking");
        hub.feed_pcm16(Who::Assistant, &[8000i16; 320]);
        let snap = hub.snapshot(0);
        assert_eq!(snap["state"], "speaking");
        assert_eq!(snap["frames"].as_array().unwrap().len(), 1);
        // Polling with the returned seq yields no repeats.
        let seq = snap["seq"].as_u64().unwrap();
        assert!(hub.snapshot(seq)["frames"].as_array().unwrap().is_empty());
        hub.on_event(&AgentEvent::TurnFinished {
            session_id: "s".into(),
            turn_id: "t".into(),
            status: TurnStatus::Completed,
            error: None,
            usage: Usage::default(),
        });
        std::thread::sleep(SPEAKING_WINDOW + Duration::from_millis(20));
        assert_eq!(hub.snapshot(seq)["state"], "idle");
    }

    #[test]
    fn ring_and_poll_cap() {
        let hub = LevelHub::new();
        for _ in 0..(RING + 50) {
            hub.feed_pcm16(Who::User, &[100i16; 16]);
        }
        let snap = hub.snapshot(0);
        assert_eq!(snap["frames"].as_array().unwrap().len(), MAX_FRAMES_PER_POLL);
        assert_eq!(snap["seq"].as_u64().unwrap(), (RING + 50) as u64);
    }
}

// ---------------------------------------------------------------- HTTP handlers

pub mod http {
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::extract::{Query, State};
    use axum::response::{IntoResponse, Response};
    use axum::Json;

    use crate::AppState;

    /// GET /api/voice/levels?since=<seq>
    pub async fn levels(State(st): State<Arc<AppState>>, Query(q): Query<HashMap<String, String>>) -> Response {
        let since = q.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
        Json(st.levels.snapshot(since)).into_response()
    }

    /// POST /api/voice/demo: synthetic levels for testing visualizers.
    pub async fn demo(State(st): State<Arc<AppState>>) -> Response {
        let hub = st.levels.clone();
        tokio::spawn(async move { hub.run_demo().await });
        axum::http::StatusCode::ACCEPTED.into_response()
    }
}
