//! The watch with a fake capture and a scripted vision model (screen-watch.md section 7): no
//! real screenshot, no real model call. Time is paused: a 20 minute watch runs in milliseconds.

use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::vision::Api;
use super::*;

pub fn png(top: bool) -> Vec<u8> {
    let mut img = image::RgbImage::from_pixel(320, 180, image::Rgb([10, 10, 10]));
    let rows = if top { 0..90 } else { 90..180 };
    for y in rows {
        for x in 0..320 {
            img.put_pixel(x, y, image::Rgb([240, 240, 240]));
        }
    }
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

pub fn reply(met: bool, note: &str, confidence: f32) -> String {
    format!("{{\"met\": {met}, \"note\": \"{note}\", \"confidence\": {confidence}}}")
}

/// The daemon's side, scripted. `replies` is read in order; when it runs dry the model says "not met".
pub struct Fake {
    pub cfg: Mutex<ScreenWatchSection>,
    pub replies: Mutex<VecDeque<Result<String, String>>>,
    pub cloud: Mutex<Option<VecDeque<Result<String, String>>>>,
    pub cloud_frames: AtomicUsize,
    pub local_calls: AtomicU32,
    pub told: Mutex<Vec<String>>,
    pub restarts: Mutex<Vec<String>>,
    /// The capture goes through `capture::grab` (with `KAGENT_FAKE_SCREENSHOT`), like the daemon's.
    pub real_grab: AtomicBool,
    pub locked: AtomicBool,
    pub busy: AtomicBool,
    pub cards: AtomicUsize,
    pub gone: AtomicBool,
    pub same_frame: AtomicBool,
    pub captures: AtomicU32,
    pub look_time: Mutex<Duration>,
    pub no_model: AtomicBool,
    pub own_window: AtomicBool,
    pub capture_error: Mutex<Option<String>>,
}

impl Fake {
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            cfg: Mutex::new(ScreenWatchSection::default()),
            replies: Mutex::new(VecDeque::new()),
            cloud: Mutex::new(None),
            cloud_frames: AtomicUsize::new(0),
            local_calls: AtomicU32::new(0),
            told: Mutex::new(Vec::new()),
            restarts: Mutex::new(Vec::new()),
            real_grab: AtomicBool::new(false),
            locked: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            cards: AtomicUsize::new(0),
            gone: AtomicBool::new(false),
            same_frame: AtomicBool::new(false),
            captures: AtomicU32::new(0),
            look_time: Mutex::new(Duration::ZERO),
            no_model: AtomicBool::new(false),
            own_window: AtomicBool::new(false),
            capture_error: Mutex::new(None),
        })
    }
    pub fn say(&self, replies: &[&str]) {
        self.replies.lock().unwrap().extend(replies.iter().map(|r| Ok(r.to_string())));
    }
    pub fn told(&self) -> Vec<String> {
        self.told.lock().unwrap().clone()
    }
}

#[async_trait]
impl Env for Fake {
    fn config(&self) -> ScreenWatchSection {
        self.cfg.lock().unwrap().clone()
    }
    async fn local_model(&self) -> Result<LocalModel, String> {
        if self.no_model.load(Ordering::SeqCst) {
            return Err("Screen watch needs a local vision model: Settings -> Local models recommends gemma3:4b for this machine.".into());
        }
        Ok(LocalModel { id: "gemma3:4b".into(), base: "http://127.0.0.1:11434".into(), api: Api::Ollama })
    }
    async fn active_window(&self) -> Result<WindowInfo, String> {
        Ok(WindowInfo { id: "0x0460000a".into(), title: "konsole: cargo build".into(), own: self.own_window.load(Ordering::SeqCst) })
    }
    async fn capture(&self, target: Target, window_id: Option<&str>) -> Result<Vec<u8>, CaptureError> {
        if self.real_grab.load(Ordering::SeqCst) {
            return capture::grab(target, window_id).await;
        }
        if self.gone.load(Ordering::SeqCst) {
            return Err(CaptureError::WindowGone);
        }
        if let Some(e) = self.capture_error.lock().unwrap().clone() {
            return Err(CaptureError::Other(e));
        }
        let n = self.captures.fetch_add(1, Ordering::SeqCst);
        Ok(png(self.same_frame.load(Ordering::SeqCst) || n % 2 == 0))
    }
    async fn locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }
    fn cards_open(&self) -> usize {
        self.cards.load(Ordering::SeqCst)
    }
    fn turn_busy(&self, _model: &LocalModel) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
    async fn look_local(&self, _model: &LocalModel, jpeg: &[u8], _goal: &str) -> Result<String, String> {
        assert_eq!(&jpeg[..2], &[0xff, 0xd8], "the model gets a JPEG frame");
        self.local_calls.fetch_add(1, Ordering::SeqCst);
        let d = *self.look_time.lock().unwrap();
        if !d.is_zero() {
            tokio::time::sleep(d).await;
        }
        self.replies.lock().unwrap().pop_front().unwrap_or_else(|| Ok(reply(false, "still compiling", 0.9)))
    }
    async fn look_cloud(&self, jpeg: &[u8], _goal: &str) -> Option<Result<String, String>> {
        let mut cloud = self.cloud.lock().unwrap();
        let q = cloud.as_mut()?;
        assert!(!jpeg.is_empty());
        self.cloud_frames.fetch_add(1, Ordering::SeqCst);
        Some(q.pop_front().unwrap_or_else(|| Ok(reply(true, "cloud agrees", 0.9))))
    }
    async fn tell(&self, _watch: &Watch, text: &str) {
        self.told.lock().unwrap().push(text.to_string());
    }
    async fn restarted(&self, _watch: &Watch, text: &str) {
        self.restarts.lock().unwrap().push(text.to_string());
    }
}

pub struct Rig {
    pub sw: std::sync::Arc<ScreenWatch>,
    pub fake: std::sync::Arc<Fake>,
    pub events: broadcast::Receiver<AgentEvent>,
}

pub fn rig() -> Rig {
    let fake = Fake::new();
    let (tx, events) = broadcast::channel(1024);
    Rig { sw: ScreenWatch::new(fake.clone(), tx, None), fake, events }
}

pub fn args(goal: &str) -> StartArgs {
    StartArgs { session_id: "s1".into(), goal: goal.into(), ..Default::default() }
}

/// Let the watch run until it is over (the clock is paused, so this takes no time).
pub async fn settle(sw: &ScreenWatch) -> Watch {
    for _ in 0..100_000 {
        if sw.current().is_none() {
            return sw.last_ended().expect("it ended");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("the watch did not end: {:?}", sw.current());
}

pub fn states(rx: &mut broadcast::Receiver<AgentEvent>) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::ScreenWatchChanged { watch } = ev {
            out.push((watch["state"].as_str().unwrap().to_string(), watch["looks"].as_u64().unwrap()));
        }
    }
    out
}

mod guards;
mod hits;
mod models;
mod telling;
mod tool;
