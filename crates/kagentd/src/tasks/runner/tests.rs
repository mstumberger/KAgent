use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use kagent_core::permission::{ApprovalKind, ApprovalRequest};
use kagent_core::tool::Risk;
use serde_json::json;
use tokio::sync::{broadcast, Notify};

use super::*;
use chrono::TimeZone;

use crate::sections::TasksSection;
use crate::tasks::store::TaskInput;

/// Longer than any test waits.
const LONG: Duration = Duration::from_secs(600);

fn now() -> DateTime<Local> {
    Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
}

/// The agent as the runner needs it, scripted by the test.
struct Fake {
    events: broadcast::Sender<AgentEvent>,
    /// While `holding`, `wait_idle` returns only once the turn was cancelled.
    holding: AtomicBool,
    waiting: AtomicUsize,
    /// A card was raised and nobody answered it yet: like the real loop, the turn does
    /// not end before that.
    unanswered: AtomicBool,
    release: Notify,
    /// How the turn ends.
    answer: Mutex<TurnEnd>,
    /// A card to raise when the turn starts.
    card: Mutex<Option<ApprovalRequest>>,
    log: Mutex<Vec<String>>,
    /// The sessions made: how many, and the one of each task with `reuse`.
    sessions: Mutex<(usize, HashMap<String, String>)>,
}

impl Fake {
    fn say(&self, status: TurnStatus, text: &str) {
        *self.answer.lock().unwrap() = TurnEnd { status, error: None, text: text.into() };
    }

    fn raise(&self, tool: &str, summary: &str) {
        *self.card.lock().unwrap() = Some(ApprovalRequest {
            request_id: "req-1".into(),
            session_id: String::new(),
            call_id: "call-1".into(),
            tool: tool.into(),
            kind: ApprovalKind::Tool,
            input: json!({}),
            summary: summary.into(),
            reason: "it changes the system".into(),
            risk: Risk::Low,
            privileged: false,
            destructive: false,
            can_remember: true,
            rule_key: tool.into(),
            preview: None,
        });
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// The owner answers the card in the UI.
    fn owner_answers(&self) {
        self.unanswered.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
    }
}

#[async_trait]
impl TaskRunner for Fake {
    async fn session(&self, policy: SessionPolicy, title: &str, existing: Option<&str>) -> anyhow::Result<String> {
        if let (SessionPolicy::Reuse, Some(id)) = (policy, existing) {
            return Ok(id.to_string());
        }
        let mut sessions = self.sessions.lock().unwrap();
        if policy == SessionPolicy::Reuse {
            if let Some(id) = sessions.1.get(title) {
                return Ok(id.clone());
            }
        }
        sessions.0 += 1;
        let id = format!("sess{}", sessions.0);
        if policy == SessionPolicy::Reuse {
            sessions.1.insert(title.to_string(), id.clone());
        }
        Ok(id)
    }

    async fn send(&self, session: &str, prompt: &str) -> anyhow::Result<()> {
        self.log.lock().unwrap().push(format!("send {session} {prompt}"));
        if let Some(card) = self.card.lock().unwrap().take() {
            let card = ApprovalRequest { session_id: session.to_string(), ..card };
            let _ = self.events.send(AgentEvent::ApprovalRequested(card));
            self.unanswered.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    async fn wait_idle(&self, _session: &str) -> TurnEnd {
        // Counted from creation to drop: the runner drops this future whenever an event
        // comes in and asks again.
        let _waiting = Counted(&self.waiting);
        self.waiting.fetch_add(1, Ordering::SeqCst);
        while self.holding.load(Ordering::SeqCst) || self.unanswered.load(Ordering::SeqCst) {
            if tokio::time::timeout(Duration::from_millis(20), self.release.notified()).await.is_ok() {
                break;
            }
        }
        self.answer.lock().unwrap().clone()
    }

    async fn cancel(&self, session: &str) -> bool {
        self.log.lock().unwrap().push(format!("cancel {session}"));
        self.holding.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
        true
    }

    async fn deny(&self, request_id: &str, reason: &str) -> bool {
        self.log.lock().unwrap().push(format!("deny {request_id} {reason}"));
        self.unanswered.store(false, Ordering::SeqCst);
        true
    }

    fn events(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
}

/// Decrements a counter when dropped.
struct Counted<'a>(&'a AtomicUsize);

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What was delivered to the owner.
#[derive(Default)]
struct Recorder(Mutex<Vec<Notification>>);

#[async_trait]
impl Deliverer for Recorder {
    async fn deliver(&self, notification: Notification) {
        self.0.lock().unwrap().push(notification);
    }
}

impl Recorder {
    fn items(&self) -> Vec<Notification> {
        self.0.lock().unwrap().clone()
    }
}

struct Rig {
    runner: Arc<Runner>,
    fake: Arc<Fake>,
    sent: Arc<Recorder>,
    store: Arc<TaskStore>,
    _dir: tempfile::TempDir,
    _receiver: broadcast::Receiver<AgentEvent>,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let (events, receiver) = broadcast::channel(256);
    let store = TaskStore::load(dir.path(), events.clone(), TasksSection::default());
    let fake = Arc::new(Fake {
        events,
        holding: AtomicBool::new(false),
        waiting: AtomicUsize::new(0),
        unanswered: AtomicBool::new(false),
        release: Notify::new(),
        answer: Mutex::new(TurnEnd::empty()),
        card: Mutex::new(None),
        log: Mutex::new(Vec::new()),
        sessions: Mutex::new((0, HashMap::new())),
    });
    let sent = Arc::new(Recorder::default());
    let runner = Arc::new(Runner { store: store.clone(), runner: fake.clone(), deliver: sent.clone(), now: Arc::new(now) });
    Rig { runner, fake, sent, store, _dir: dir, _receiver: receiver }
}

/// A to-do with the body of a request.
fn add(rig: &Rig, body: serde_json::Value) -> Task {
    rig.store.add(TaskInput::parse(body).unwrap(), now()).unwrap()
}

/// Ask for the task and claim it: what the loop hands to a run.
fn claimed(rig: &Rig, task: &Task) -> Task {
    rig.store.request_run(&task.id).unwrap();
    let due = rig.store.claim(now(), 2);
    assert_eq!(due.len(), 1, "one task is claimed");
    due.into_iter().next().unwrap()
}

#[tokio::test]
async fn nothing_sends_no_notification_and_a_real_answer_does() {
    let r = rig();
    let t = add(&r, json!({"title": "Backups", "what": "check the backups", "kind": "agent"}));
    r.fake.say(TurnStatus::Completed, "NOTHING");
    r.runner.run(&claimed(&r, &t), LONG).await;
    let after = r.store.get(&t.id).unwrap();
    assert_eq!((after.last_status, after.last_result.as_deref()), (Some(RunStatus::Ok), Some("")), "an empty result");
    assert_eq!(after.runs[0].result.as_deref(), Some(""));
    assert!(r.sent.items().is_empty(), "no notification");

    let t = add(&r, json!({"title": "Backups", "what": "check the backups", "kind": "agent"}));
    r.fake.say(TurnStatus::Completed, "Two units failed: a.service and b.service.");
    r.runner.run(&claimed(&r, &t), LONG).await;
    let sent = r.sent.items();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].title.as_str(), sent[0].body.as_str(), sent[0].channels.clone()), ("Task done: Backups", "Two units failed: a.service and b.service.", vec![Channel::Desktop]), "`session` stores, it is not a notification channel");
    assert_eq!(r.store.get(&t.id).unwrap().last_result.as_deref(), Some("Two units failed: a.service and b.service."));
    // The prompt of an unattended run.
    let log = r.fake.log();
    assert!(log[1].starts_with("send sess2 Scheduled task 'Backups' runs unattended. Reply with exactly NOTHING if nothing needs the owner's attention.\ncheck the backups"), "{}", log[1]);
}

#[tokio::test]
async fn a_turn_that_said_nothing_is_a_failed_run() {
    let r = rig();
    let t = add(&r, json!({"what": "check", "kind": "agent"}));
    r.fake.say(TurnStatus::Completed, "");
    r.runner.run(&claimed(&r, &t), LONG).await;
    let after = r.store.get(&t.id).unwrap();
    assert_eq!((after.last_status, after.last_result.as_deref()), (Some(RunStatus::Failed), Some("the agent gave no answer (see the task's session)")));
    assert_eq!(r.sent.items()[0].body, "no result; open the task's session");
    let t = add(&r, json!({"what": "check", "kind": "agent"}));
    r.fake.say(TurnStatus::Failed, "");
    r.runner.run(&claimed(&r, &t), LONG).await;
    let after = r.store.get(&t.id).unwrap();
    assert_eq!(after.last_status, Some(RunStatus::Failed));
    // A to-do is done only after a run that went well.
    assert!(!after.done);
}

#[tokio::test]
async fn deny_refuses_a_card_at_once_and_wait_leaves_it() {
    let r = rig();
    // Scenario S28 item 5: the call is refused, the result lists what was skipped, the
    // history shows ok.
    let t = add(&r, json!({"title": "Backups", "what": "delete the old backups", "kind": "agent", "onApproval": "deny"}));
    r.fake.say(TurnStatus::Completed, "I could not delete the backups.");
    r.fake.raise("shell", "rm -rf ~/backups/old");
    r.runner.run(&claimed(&r, &t), LONG).await;
    assert!(r.fake.log().iter().any(|line| line == "deny req-1 unattended task: this needs approval on the desktop"), "{:?}", r.fake.log());
    let after = r.store.get(&t.id).unwrap();
    assert_eq!(after.runs[0].status, RunStatus::Ok);
    assert_eq!(after.last_result.as_deref(), Some("I could not delete the backups.\n\nSkipped, they need approval on the desktop:\n- shell: rm -rf ~/backups/old"));
    assert_eq!(r.sent.items()[0].title, "Task done: Backups");

    // `wait`: the card stays in the UI until it is answered, or the approval timeout
    // passes. The session of the card is the one of the run.
    let t = add(&r, json!({"what": "delete the old backups", "kind": "agent", "onApproval": "wait"}));
    r.fake.say(TurnStatus::Completed, "NOTHING");
    r.fake.raise("shell", "rm -rf ~/backups/old");
    let before = r.fake.log().len();
    let (runner, task) = (r.runner.clone(), claimed(&r, &t));
    let run = tokio::spawn(async move { runner.run(&task, LONG).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.fake.waiting.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the run waits with the card open");
    assert!(!r.fake.log()[before..].iter().any(|line| line.starts_with("deny ")), "{:?}", r.fake.log());
    r.fake.owner_answers();
    tokio::time::timeout(Duration::from_secs(5), run).await.unwrap().unwrap();
    assert!(!r.fake.log()[before..].iter().any(|line| line.starts_with("deny ")), "{:?}", r.fake.log());
    assert_eq!(r.store.get(&t.id).unwrap().last_result.as_deref(), Some(""));
}

#[tokio::test]
async fn a_run_out_of_time_is_cancelled() {
    let r = rig();
    let t = add(&r, json!({"what": "check", "kind": "agent", "timeoutMin": 2}));
    r.fake.holding.store(true, Ordering::SeqCst);
    r.fake.say(TurnStatus::Cancelled, "half a report");
    r.runner.run(&claimed(&r, &t), Duration::from_millis(40)).await;
    let after = r.store.get(&t.id).unwrap();
    assert_eq!(after.last_status, Some(RunStatus::Timeout));
    assert_eq!(after.last_result.as_deref(), Some("timed out after 2 minutes: half a report"), "the partial text is kept");
    assert!(r.fake.log().iter().any(|line| line == "cancel sess1"), "{:?}", r.fake.log());
    assert_eq!(r.sent.items()[0].title, "Task timed out: check");
}

#[tokio::test]
async fn cancel_stops_the_run_and_it_is_recorded_cancelled() {
    let r = rig();
    let t = add(&r, json!({"what": "check", "kind": "agent"}));
    r.fake.holding.store(true, Ordering::SeqCst);
    r.fake.say(TurnStatus::Cancelled, "half a report");
    let (runner, task) = (r.runner.clone(), claimed(&r, &t));
    let run = tokio::spawn(async move { runner.run(&task, LONG).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while r.fake.waiting.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the run waits for its turn");
    assert_eq!(r.store.cancel(&t.id).unwrap(), true);
    tokio::time::timeout(Duration::from_secs(5), run).await.unwrap().unwrap();
    let after = r.store.get(&t.id).unwrap();
    assert_eq!((after.last_status, after.last_result.as_deref()), (Some(RunStatus::Cancelled), Some("half a report")));
    assert!(r.sent.items().is_empty(), "a cancelled run is not announced");
    assert_eq!(r.store.cancel(&t.id).unwrap(), false, "a finished run has nothing to cancel");
    // A task cancelled between its claim and its run does not start.
    let t = add(&r, json!({"what": "check", "kind": "agent"}));
    let claimed = claimed(&r, &t);
    r.store.cancel(&t.id).unwrap();
    r.runner.run(&claimed, LONG).await;
    let after = r.store.get(&t.id).unwrap();
    assert_eq!((after.last_status, after.last_result.as_deref()), (Some(RunStatus::Cancelled), Some("cancelled")));
    assert_eq!(r.fake.log().len(), 2, "nothing was sent for it: {:?}", r.fake.log());
}

#[tokio::test]
async fn at_most_max_concurrent_agent_runs_at_once() {
    let r = rig();
    let a = add(&r, json!({"what": "a", "kind": "agent"}));
    let b = add(&r, json!({"what": "b", "kind": "agent"}));
    let c = add(&r, json!({"what": "c", "kind": "remind"}));
    for task in [&a, &b, &c] {
        r.store.request_run(&task.id).unwrap();
    }
    r.fake.holding.store(true, Ordering::SeqCst);
    // The limit is one: the first pass starts one agent task. A reminder is no turn, so
    // it does not wait for a slot.
    let first = r.store.claim(now(), 1);
    assert_eq!(first.iter().map(|t| t.kind.as_str()).collect::<Vec<_>>(), ["agent", "remind"]);
    // The second agent task waits: a later pass does not start it while the first runs.
    assert!(r.store.claim(now(), 1).is_empty());
    assert_eq!(r.store.get(&b.id).unwrap().last_status, None, "not claimed");
    // The first run ends: the next pass starts the second.
    r.fake.holding.store(false, Ordering::SeqCst);
    r.fake.say(TurnStatus::Completed, "NOTHING");
    r.runner.run(&first[0], LONG).await;
    let second = r.store.claim(now(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, b.id);
    r.runner.run(&second[0], LONG).await;
    // With the limit at two, two agent tasks start in one pass.
    let d = add(&r, json!({"what": "d", "kind": "agent"}));
    for task in [&a, &d] {
        r.store.request_run(&task.id).unwrap();
    }
    assert_eq!(r.store.claim(now(), 2).len(), 2);
}

#[tokio::test]
async fn reuse_keeps_one_session_and_new_makes_one_per_run() {
    let r = rig();
    r.fake.say(TurnStatus::Completed, "NOTHING");
    let t = add(&r, json!({"what": "check", "kind": "agent", "session": "reuse"}));
    for _ in 0..2 {
        r.runner.run(&claimed(&r, &t), LONG).await;
    }
    let after = r.store.get(&t.id).unwrap();
    assert_eq!((r.fake.sessions.lock().unwrap().0, after.session_id.as_deref(), after.runs.iter().map(|run| run.session_id.clone()).collect::<Vec<_>>()), (1, Some("sess1"), vec![Some("sess1".to_string()), Some("sess1".to_string())]));
    let t = add(&r, json!({"what": "check", "kind": "agent"}));
    for _ in 0..2 {
        r.runner.run(&claimed(&r, &t), LONG).await;
    }
    assert_eq!(r.fake.sessions.lock().unwrap().0, 3, "a new session for each run");
    assert_eq!(r.store.get(&t.id).unwrap().session_id, None);
}

#[tokio::test]
async fn a_reminder_goes_to_its_channels() {
    let r = rig();
    let t = add(&r, json!({"title": "Electrician", "what": "Call the electrician", "kind": "remind", "deliver": ["desktop", "session"]}));
    r.runner.run(&claimed(&r, &t), LONG).await;
    let sent = r.sent.items();
    assert_eq!((sent[0].title.as_str(), sent[0].body.as_str(), sent[0].channels.clone()), ("Reminder: Electrician", "Call the electrician", vec![Channel::Desktop]));
    let after = r.store.get(&t.id).unwrap();
    assert_eq!((after.last_status, after.last_result.as_deref()), (Some(RunStatus::Ok), Some("reminded: Call the electrician")));
    assert_eq!(after.runs[0].session_id, None, "a reminder has no session");
    // Nothing to show on: the run is recorded, nothing is delivered.
    let quiet = add(&r, json!({"what": "silent", "kind": "remind", "deliver": ["session"]}));
    r.runner.run(&claimed(&r, &quiet), LONG).await;
    assert_eq!(r.sent.items().len(), 1);
    let telegram = add(&r, json!({"what": "far", "kind": "remind", "deliver": ["telegram"]}));
    r.runner.run(&claimed(&r, &telegram), LONG).await;
    assert_eq!(r.sent.items()[1].channels, vec![Channel::Telegram]);
}
