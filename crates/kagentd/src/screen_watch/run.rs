//! The loop of a watch (screen-watch.md sections 2, 3 and 5): look, judge, report, wait.
//! It ends by itself (a hit of a `once` watch, the time, five unreadable answers in a row, a
//! closed window) or is ended from outside (`ScreenWatch::end`), which also cancels it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::capture::CaptureError;
use super::look::{self, Looker, Prepared, Verdict, LOW_CONFIDENCE};
use super::{LocalModel, Mode, ScreenWatch, State, Target, Watch};

/// Unreadable answers in a row that end the watch.
const MAX_UNREADABLE: u32 = 5;
/// A `repeat` watch reports at most this often.
const REPORT_EVERY: Duration = Duration::from_secs(30);
/// The event with `looks` and `lastNote` goes out at most this often (the state changes always go).
const EVENT_EVERY: Duration = Duration::from_secs(5);
/// The wait before the second look of a hit that is not sure.
const SECOND_LOOK: Duration = Duration::from_secs(2);
/// How many looks the median of the slow check is made of.
const SLOW_WINDOW: usize = 5;

/// What asking for a frame came to.
enum Frame {
    Png(Vec<u8>),
    /// Nothing taken, nothing counted (locked, a card is open, a failed capture): wait and go on.
    Wait,
    /// The watch is over (it ended, or was ended).
    Over,
}

/// What a look came to.
enum Judged {
    Skipped,
    Verdict(Verdict, Vec<u8>),
    /// The note, which counts towards the five.
    Unreadable(String),
}

struct Run {
    sw: Arc<ScreenWatch>,
    id: String,
    goal: String,
    mode: Mode,
    target: Target,
    window_id: Option<String>,
    minutes: u32,
    model: LocalModel,
    cancel: CancellationToken,
    deadline: Instant,
    looker: Looker,
    /// Unreadable answers and failed captures in a row.
    failures: u32,
    times: VecDeque<Duration>,
    every: u64,
    last_report: Option<Instant>,
    last_event: Instant,
}

pub async fn run(sw: Arc<ScreenWatch>, watch: Watch, model: LocalModel, cancel: CancellationToken, deadline: Instant) {
    let mut r = Run {
        sw,
        id: watch.id.clone(),
        goal: watch.goal.clone(),
        mode: watch.mode,
        target: watch.target,
        window_id: watch.window_id.clone(),
        minutes: watch.minutes,
        model,
        cancel,
        deadline,
        looker: Looker::default(),
        failures: 0,
        times: VecDeque::new(),
        every: u64::from(watch.every_sec),
        last_report: None,
        last_event: Instant::now(),
    };
    r.go().await;
}

impl Run {
    /// Wait `secs` (never past the end of the time). False when the watch was ended meanwhile.
    async fn nap(&self, d: Duration) -> bool {
        let until = (Instant::now() + d).min(self.deadline);
        tokio::select! {
            _ = self.cancel.cancelled() => false,
            _ = tokio::time::sleep_until(until) => true,
        }
    }

    /// Change the watch; false when it is not running any more. The event goes out at most every 5 s, or at once when `force`.
    fn touch(&mut self, force: bool, f: impl FnOnce(&mut Watch)) -> bool {
        let Some(w) = self.sw.update(&self.id, f) else { return false };
        if force || self.last_event.elapsed() >= EVENT_EVERY {
            self.last_event = Instant::now();
            self.sw.emit(&w);
        }
        true
    }

    /// One look is made: counted, with the note when there is one.
    fn counted(&mut self, note: Option<String>) -> bool {
        let at = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        self.touch(false, |w| {
            w.looks += 1;
            w.last_look_at = Some(at);
            if let Some(n) = note {
                w.last_note = n;
            }
        })
    }

    async fn fail(&mut self, reason: &str) {
        if let Some(w) = self.sw.end(&self.id, State::Failed, None, Some(reason)) {
            self.sw.env.tell(&w, &format!("Screen watch ended: {reason}. (goal: {})", w.goal)).await;
        }
    }

    async fn expire(&mut self) {
        let Some(w) = self.sw.end(&self.id, State::Expired, None, None) else { return };
        let note = if w.last_note.is_empty() { "nothing to report" } else { w.last_note.as_str() };
        let unit = if self.minutes == 1 { "minute" } else { "minutes" };
        self.sw.env.tell(&w, &format!("Screen watch ended after {} {unit}: {note}", self.minutes)).await;
    }

    /// A frame, under the rules of `vision.md` section 3: never a locked screen, never KAgent's own cards.
    async fn frame(&mut self) -> Frame {
        let env = self.sw.env.clone();
        if env.locked().await {
            return if self.touch(true, |w| w.paused = true) { Frame::Wait } else { Frame::Over };
        }
        if self.target == Target::Monitor && env.cards_open() > 0 {
            return Frame::Wait;
        }
        if !self.touch(true, |w| w.paused = false) {
            return Frame::Over;
        }
        match env.capture(self.target, self.window_id.as_deref()).await {
            Ok(png) => Frame::Png(png),
            Err(CaptureError::WindowGone) => {
                self.fail("the window was closed").await;
                Frame::Over
            }
            Err(CaptureError::Other(e)) => self.broken(format!("the screen could not be captured: {e}")).await,
        }
    }

    /// A failure of the frame or of the model that counts towards the five in a row.
    async fn broken(&mut self, note: String) -> Frame {
        self.failures += 1;
        if self.failures >= MAX_UNREADABLE {
            self.fail(&format!("{} in a row ({note})", if note == look::UNREADABLE { "5 unreadable answers" } else { "5 failed looks" })).await;
            return Frame::Over;
        }
        if self.touch(false, |w| w.last_note = note) {
            Frame::Wait
        } else {
            Frame::Over
        }
    }

    /// Decide on a frame and, unless it is skipped, ask the local model.
    async fn judge(&mut self, png: &[u8], force: bool) -> Judged {
        let jpeg = match self.looker.prepare(png, force) {
            Ok(Prepared::Skip) => return Judged::Skipped,
            Ok(Prepared::Frame(j)) => j,
            Err(e) => return Judged::Unreadable(format!("the frame could not be read: {e}")),
        };
        let started = Instant::now();
        let reply = self.sw.env.look_local(&self.model, &jpeg, &self.goal).await;
        self.times.push_back(started.elapsed());
        match reply {
            Ok(raw) => match look::parse_reply(&raw) {
                Some(v) => Judged::Verdict(v, jpeg),
                None => Judged::Unreadable(look::UNREADABLE.into()),
            },
            Err(e) => Judged::Unreadable(e),
        }
    }

    /// The looks are slow when their median is over half of the interval: the interval doubles (section 5).
    fn slow(&mut self) {
        if self.times.len() < SLOW_WINDOW {
            return;
        }
        let mut sorted: Vec<Duration> = self.times.drain(..).collect();
        sorted.sort();
        if sorted[SLOW_WINDOW / 2].as_secs_f64() * 2.0 > self.every as f64 && self.every < 60 {
            self.every = (self.every * 2).min(60);
            let (note, every) = (format!("the looks are slow: now every {} s", self.every), self.every as u32);
            self.touch(true, |w| {
                w.last_note = note;
                w.every_sec = every;
            });
        }
    }

    async fn go(&mut self) {
        loop {
            if self.cancel.is_cancelled() {
                return;
            }
            if Instant::now() >= self.deadline {
                self.expire().await;
                return;
            }
            // A turn that thinks on the same model goes first (section 5).
            if self.sw.env.turn_busy(&self.model) {
                if !self.nap(Duration::from_secs(1)).await {
                    return;
                }
                continue;
            }
            let png = match self.frame().await {
                Frame::Png(p) => p,
                Frame::Wait => {
                    if !self.nap(Duration::from_secs(self.every)).await {
                        return;
                    }
                    continue;
                }
                Frame::Over => return,
            };
            if !self.look(png).await {
                return;
            }
            self.slow();
            if !self.nap(Duration::from_secs(self.every)).await {
                return;
            }
        }
    }

    /// One look and what follows it. False when the watch is over.
    async fn look(&mut self, png: Vec<u8>) -> bool {
        match self.judge(&png, false).await {
            Judged::Skipped => self.counted(None),
            Judged::Unreadable(note) => {
                if !self.counted(Some(note.clone())) {
                    return false;
                }
                !matches!(self.broken(note).await, Frame::Over)
            }
            Judged::Verdict(v, jpeg) => {
                self.failures = 0;
                if !self.counted(Some(v.note.clone())) {
                    return false;
                }
                if !v.met {
                    return true;
                }
                self.hit(v, jpeg).await
            }
        }
    }

    /// The model says met. A hit that is not sure needs a second look, and the optional cloud check comes last.
    async fn hit(&mut self, mut v: Verdict, mut jpeg: Vec<u8>) -> bool {
        if v.confidence < LOW_CONFIDENCE {
            if !self.nap(SECOND_LOOK).await {
                return false;
            }
            let png = match self.frame().await {
                Frame::Png(p) => p,
                Frame::Wait => return true,
                Frame::Over => return false,
            };
            match self.judge(&png, true).await {
                Judged::Verdict(second, j) => {
                    if !self.counted(Some(second.note.clone())) {
                        return false;
                    }
                    if !second.met {
                        return true;
                    }
                    (v, jpeg) = (second, j);
                }
                Judged::Unreadable(note) => return self.counted(Some(note)),
                Judged::Skipped => return true,
            }
        }
        if self.sw.env.config().cloud_confirm {
            // The one frame that leaves the machine, only for a hit, and only once: a failed answer is not asked again.
            match self.sw.env.look_cloud(&jpeg, &self.goal).await {
                Some(Ok(raw)) => match look::parse_reply(&raw) {
                    Some(c) if !c.met => return self.touch(true, |w| w.last_note = format!("not confirmed by the cloud model: {}", c.note)),
                    Some(c) if !c.note.is_empty() => v.note = c.note,
                    _ => {}
                },
                _ => {}
            }
        }
        self.report(v.note).await
    }

    /// Tell the owner of a hit. False when the watch is over.
    async fn report(&mut self, note: String) -> bool {
        let text = format!("Screen watch: {}. (goal: {})", note.trim_end_matches('.'), self.goal);
        let text = if note.is_empty() { format!("Screen watch: the goal looks reached. (goal: {})", self.goal) } else { text };
        match self.mode {
            Mode::Once => {
                if let Some(w) = self.sw.end(&self.id, State::Met, Some(&note), None) {
                    self.sw.env.tell(&w, &text).await;
                }
                false
            }
            Mode::Repeat => {
                if !self.touch(true, |w| w.last_note = note) {
                    return false;
                }
                if self.last_report.is_none_or(|t| t.elapsed() >= REPORT_EVERY) {
                    self.last_report = Some(Instant::now());
                    if let Some(w) = self.sw.current() {
                        self.sw.env.tell(&w, &text).await;
                    }
                }
                true
            }
        }
    }
}
