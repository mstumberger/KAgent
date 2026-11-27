//! Spoken confirmation of the live model's actions (voice.md section 12). A call the
//! permission engine would ask about does not run on the model's first call and raises
//! no card: the model gets `needs_confirmation` and a sentence to say ("I want to reboot
//! the computer now. Say confirm to go ahead, or no."). Only the owner's own speech
//! heard after that (the input transcription, never the model's output) answers it. The
//! model then calls the same tool with the same arguments again, and that call runs with
//! its card answered by the voice, so nothing waits for a click.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use kagent_core::tool::CallAssessment;

use super::approve::{classify_for, Answer};

/// How long a spoken answer stays good for the repeated call.
pub const VALID_FOR: Duration = Duration::from_secs(90);

#[derive(Debug, Clone)]
struct Pending {
    key: String,
    summary: String,
    /// Root or destructive: only an explicit "confirm" / "approve" / "potrdi" counts.
    strong: bool,
    asked_at: Instant,
    /// The owner's speech since the question was asked.
    heard: String,
    answer: Option<Answer>,
}

/// What the live session does with a call that would need approval.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Not asked yet (or asked about another call): ask now with this tool response.
    Ask(Value),
    /// The owner confirmed this call: run it and answer its card by voice.
    Run,
    /// The owner said no to this call.
    Refused(Value),
}

/// The one question a live session has open at a time.
#[derive(Default)]
pub struct VoiceConfirm {
    pending: Mutex<Option<Pending>>,
}

/// Within this time the same changing call (tool and arguments) is not run twice: a
/// live model that loops on one action is stopped after the first.
pub const REPEAT_WINDOW: Duration = Duration::from_secs(20);

/// The changing calls a live session ran a moment ago.
#[derive(Default)]
pub struct Repeats {
    ran: Mutex<Vec<(String, Instant)>>,
}

impl Repeats {
    /// Record that the call runs now; false when the same call ran within
    /// `REPEAT_WINDOW` (it must not run again).
    pub fn first(&self, tool: &str, args: &Value, now: Instant) -> bool {
        let key = key(tool, args);
        let mut ran = self.ran.lock().unwrap();
        ran.retain(|(_, at)| now.duration_since(*at) <= REPEAT_WINDOW);
        if ran.iter().any(|(k, _)| *k == key) {
            return false;
        }
        ran.push((key, now));
        true
    }
}

/// A call's identity: the tool and its exact arguments.
pub fn key(tool: &str, args: &Value) -> String {
    format!("{tool} {args}")
}

/// The sentence the model says (pure, tested).
pub fn sentence(summary: &str, strong: bool, destructive: bool) -> String {
    let what = summary.trim().trim_end_matches('.');
    let warning = if destructive { " This cannot be undone; unsaved work may be lost." } else { "" };
    let how = if strong { "Say confirm to go ahead, or no." } else { "Say yes to go ahead, or no." };
    format!("I want to do this: {what}.{warning} {how}")
}

impl VoiceConfirm {
    /// The step for a call the engine would ask about, at `now`.
    pub fn step(&self, tool: &str, args: &Value, a: &CallAssessment, now: Instant) -> Step {
        let key = key(tool, args);
        let mut pending = self.pending.lock().unwrap();
        if let Some(p) = pending.as_ref().filter(|p| p.key == key && now.duration_since(p.asked_at) <= VALID_FOR) {
            match p.answer {
                Some(Answer::Yes) => {
                    *pending = None;
                    return Step::Run;
                }
                Some(Answer::No) => {
                    *pending = None;
                    return Step::Refused(json!({"status": "denied", "output": "not executed: the owner said no. Do not try again unless they ask."}));
                }
                None => {
                    let say = sentence(&a.summary, p.strong, a.destructive);
                    return Step::Ask(json!({"status": "needs_confirmation", "say": say, "output": "not executed yet: the owner has not confirmed. Say the `say` text and wait for their answer."}));
                }
            }
        }
        let strong = a.privileged || a.destructive;
        let say = sentence(&a.summary, strong, a.destructive);
        *pending = Some(Pending { key, summary: a.summary.clone(), strong, asked_at: now, heard: String::new(), answer: None });
        Step::Ask(json!({
            "status": "needs_confirmation",
            "say": say,
            "output": "not executed: actions that change the system need the owner's spoken confirmation. Say the `say` text word for word and wait. If they confirm, call this tool again with exactly the same arguments; if they say no, drop it."
        }))
    }

    /// The owner's latest words. Answers the open question once; returns the answer then.
    pub fn hear(&self, chunk: &str) -> Option<Answer> {
        let mut pending = self.pending.lock().unwrap();
        let p = pending.as_mut().filter(|p| p.answer.is_none())?;
        p.heard.push_str(chunk);
        let answer = classify_for(&p.heard, p.strong, &p.summary)?;
        p.answer = Some(answer);
        Some(answer)
    }

    /// The summary of the open question, while it waits for an answer.
    #[cfg(test)]
    pub fn waiting(&self) -> Option<String> {
        self.pending.lock().unwrap().as_ref().filter(|p| p.answer.is_none()).map(|p| p.summary.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kagent_core::tool::{CallAssessment, ToolContract};

    fn assessment(summary: &str, destructive: bool) -> CallAssessment {
        let contract = ToolContract::read_only("system_power", "", json!({"type": "object"}));
        let mut a = CallAssessment::from_contract(&contract, summary.into(), summary.into());
        a.read_only = false;
        if destructive {
            a = a.destructive();
        }
        a
    }

    fn status(step: &Step) -> &str {
        match step {
            Step::Ask(v) | Step::Refused(v) => v["status"].as_str().unwrap(),
            Step::Run => "run",
        }
    }

    /// The session of 2026-11-27: "How can we fix it?", and the live model rebooted the PC.
    #[test]
    fn a_reboot_is_asked_out_loud_and_runs_only_after_a_spoken_confirm() {
        let c = VoiceConfirm::default();
        let reboot = json!({"action": "reboot", "minutes": 0});
        let a = assessment("reboot now", true);
        let t0 = Instant::now();
        let first = c.step("system_power", &reboot, &a, t0);
        assert_eq!(status(&first), "needs_confirmation", "the first call never runs");
        let Step::Ask(v) = &first else { unreachable!() };
        assert_eq!(v["say"], "I want to do this: reboot now. This cannot be undone; unsaved work may be lost. Say confirm to go ahead, or no.");
        assert_eq!(c.waiting().as_deref(), Some("reboot now"));
        // The model calls again before the owner answered: still not run.
        assert_eq!(status(&c.step("system_power", &reboot, &a, t0)), "needs_confirmation");
        // Words that are not a strong confirmation do not count.
        assert_eq!(c.hear("How can we fix it?"), None);
        assert_eq!(c.hear(" yes"), None, "a bare yes is not enough for a reboot");
        assert_eq!(c.hear(" confirm"), Some(Answer::Yes));
        assert_eq!(c.waiting(), None);
        // Other arguments are another call: asked anew.
        assert_eq!(status(&c.step("system_power", &json!({"action": "poweroff"}), &a, t0)), "needs_confirmation");
    }

    #[test]
    fn the_confirmed_call_runs_once_and_a_no_refuses_it() {
        let c = VoiceConfirm::default();
        let args = json!({"action": "reboot"});
        let a = assessment("reboot now", true);
        let t0 = Instant::now();
        c.step("system_power", &args, &a, t0);
        c.hear("yes, reboot");
        assert_eq!(c.step("system_power", &args, &a, t0), Step::Run);
        assert_eq!(status(&c.step("system_power", &args, &a, t0)), "needs_confirmation", "one confirmation runs one call");
        c.hear("no, don't");
        assert_eq!(status(&c.step("system_power", &args, &a, t0)), "denied");
    }

    /// The session of 2026-11-27, 22:03: the live model sent the same `terminal send`
    /// 25 times in 15 seconds, into the owner's only terminal.
    #[test]
    fn the_same_changing_call_runs_once_in_the_window() {
        let r = Repeats::default();
        let args = json!({"action": "send", "target": "konsole", "text": "systemctl --failed"});
        let t0 = Instant::now();
        assert!(r.first("terminal", &args, t0));
        for i in 1..25 {
            assert!(!r.first("terminal", &args, t0 + Duration::from_millis(600 * i)), "repeat {i} is refused");
        }
        assert!(r.first("terminal", &json!({"action": "send", "target": "konsole", "text": "uptime"}), t0), "another text is another call");
        assert!(r.first("terminal", &args, t0 + REPEAT_WINDOW + Duration::from_secs(1)), "later it may run again");
    }

    #[test]
    fn a_plain_action_takes_a_yes_and_an_old_answer_expires() {
        let c = VoiceConfirm::default();
        let args = json!({"text": "hi"});
        let a = assessment("send a message", false);
        let t0 = Instant::now();
        let Step::Ask(v) = c.step("message", &args, &a, t0) else { panic!() };
        assert_eq!(v["say"], "I want to do this: send a message. Say yes to go ahead, or no.");
        c.hear("yes");
        assert_eq!(status(&c.step("message", &args, &a, t0 + VALID_FOR + Duration::from_secs(1))), "needs_confirmation", "too late: asked again");
    }
}
