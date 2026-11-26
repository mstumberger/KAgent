use super::*;
use chrono::TimeZone;
use crate::sections::TaskApproval;
use crate::tasks::schedule::{self, Schedule, Unit};

fn now() -> DateTime<Local> {
    Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap()
}

/// `now` plus `n` minutes.
fn minutes(n: i64) -> DateTime<Local> {
    now() + Duration::minutes(n)
}

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
    Local.with_ymd_and_hms(year, month, day, hour, minute, 0).unwrap()
}

fn input(body: Value) -> TaskInput {
    TaskInput::parse(body).unwrap()
}

struct Rig {
    store: Arc<TaskStore>,
    events: broadcast::Receiver<AgentEvent>,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let (events, receiver) = broadcast::channel(256);
    Rig { store: TaskStore::load(dir.path(), events, TasksSection::default()), events: receiver, _dir: dir }
}

fn ids(tasks: &[Task]) -> Vec<&str> {
    tasks.iter().map(|t| t.id.as_str()).collect()
}

#[test]
fn old_records_load_through_the_aliases() {
    // Scenario S28 item 3: a file from before S28, as its daemon wrote it.
    let old = |id: &str, kind: &str, schedule: Value, next: Option<DateTime<Local>>, runs: u32, status: Value| {
        json!({"id": id, "title": id, "what": "old job", "kind": kind, "schedule": schedule, "enabled": true, "createdAt": minutes(-1440 * 3).to_rfc3339(), "nextRun": next.map(|n| n.to_rfc3339()), "lastRun": null, "lastStatus": status, "lastResult": "old result", "lastSession": null, "runs": runs, "done": false, "runRequested": false})
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tasks.json"),
        serde_json::to_vec(&json!([
            old("t1", "agent", json!({"type": "At", "at": minutes(60).to_rfc3339()}), Some(minutes(60)), 0, Value::Null),
            old("t2", "remind", json!({"type": "Every", "minutes": 20}), Some(minutes(5)), 7, json!("ok")),
            old("t3", "agent", json!({"type": "Daily", "time": "09:00", "days": [0, 4]}), Some(at(2026, 11, 24, 9, 0)), 3, json!("ok")),
        ]))
        .unwrap(),
    )
    .unwrap();
    let mut defaults = TasksSection::default();
    defaults.timeout_min = 12;
    defaults.deliver = vec!["session".into()];
    defaults.on_approval = TaskApproval::Wait;
    let (events, _receiver) = broadcast::channel(16);
    let store = TaskStore::load(dir.path(), events, defaults);
    let tasks = store.list();
    assert_eq!(tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1", "t2", "t3"]);
    assert_eq!(tasks[0].schedule, Some(Schedule::Once { at: minutes(60) }));
    assert_eq!(tasks[1].schedule, Some(Schedule::interval(Unit::Minute, 20, None, None, None)));
    assert_eq!(tasks[2].schedule, Some(Schedule::interval(Unit::Day, 1, Some("09:00".into()), Some(vec![0, 4]), None)));
    assert_eq!((tasks[1].total_runs(), tasks[1].runs.len()), (7, 0), "the count of before is kept");
    assert_eq!((tasks[1].last_status, tasks[1].last_result.as_deref()), (Some(RunStatus::Ok), Some("old result")));
    // A policy field the record never had comes from settings.tasks.
    for task in &tasks {
        assert_eq!(task.deliver.as_slice(), &["session".to_string()][..]);
        assert_eq!((task.timeout_min, task.on_approval, task.session), (12, TaskApproval::Wait, SessionPolicy::New));
    }
    // And they run as before: every 20 minutes from the stored next run; the next
    // Monday or Friday at nine.
    assert!(store.claim(now(), 2).is_empty(), "nothing is due yet");
    let due = store.claim(minutes(5), 2);
    assert_eq!(ids(&due), ["t2"]);
    assert_eq!(due[0].next_run, Some(minutes(25)));
    let daily = store.claim(at(2026, 11, 24, 9, 0), 2);
    assert_eq!(ids(&daily), ["t3"]);
    assert_eq!(daily[0].next_run, Some(at(2026, 11, 27, 9, 0)));
}

#[test]
fn a_round_trip_an_event_after_every_save_and_a_short_id() {
    let mut r = rig();
    let t = r.store.add(input(json!({"title": "Stand up", "what": "Stretch", "kind": "remind", "schedule": "weekdays 08:30", "deliver": ["desktop", "telegram"], "timeoutMin": 5})), now()).unwrap();
    assert_eq!(t.schedule_text(), "weekdays at 08:30");
    assert_eq!((t.deliver.as_slice(), t.timeout_min), (&["desktop".to_string(), "telegram".to_string()][..], 5));
    assert!(matches!(r.events.try_recv(), Ok(AgentEvent::TasksChanged)), "a save is said");
    let refused = r.store.update(&t.id, json!({"onApproval": "wait", "delayMinutes": 0}), now()).unwrap_err();
    assert_eq!(refused, TaskError::Invalid("delayMinutes must be between 1 and 525600, not 0".into()));
    assert!(r.events.try_recv().is_err(), "a refused change saves nothing");
    r.store.update(&t.id, json!({"onApproval": "wait", "session": "reuse"}), now()).unwrap();
    // Read again from the file.
    let again = TaskStore::load(r._dir.path(), broadcast::channel(16).0, TasksSection::default());
    assert_eq!(again.list(), r.store.list());
    let t = again.get(&t.id).unwrap();
    assert_eq!((t.on_approval, t.session), (TaskApproval::Wait, SessionPolicy::Reuse));
    // Four characters of the id name the one task that has them; fewer, an ambiguous
    // prefix or an unknown id name none.
    let short = &t.id[..9];
    assert_eq!(again.get(short).map(|t| t.id.clone()), Some(t.id.clone()));
    assert!(again.get("task_zzzz").is_none());
    assert_eq!(again.delete("zzzz"), Err(TaskError::NotFound("zzzz".into())));
    let other = again.add(input(json!({"what": "another"})), now()).unwrap();
    assert!(again.get("task_").is_none(), "every id starts with it");
    assert!(again.delete(&t.id).is_ok());
    assert_eq!(ids(&again.list()), [other.id.as_str()]);
}

#[test]
fn a_record_that_does_not_read_is_left_out_and_the_rest_loads() {
    let old = |id: &str, schedule: Value| {
        json!({"id": id, "title": id, "what": "job", "kind": "agent", "schedule": schedule, "enabled": true, "createdAt": now().to_rfc3339(), "runs": 0, "done": false})
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tasks.json"),
        serde_json::to_vec(&json!([
            old("bad", json!({"type": "once", "at": minutes(60).to_rfc3339(), "repeat": true})),
            old("good", json!({"type": "Every", "minutes": 20})),
        ]))
        .unwrap(),
    )
    .unwrap();
    let (events, _receiver) = broadcast::channel(16);
    let store = TaskStore::load(dir.path(), events, TasksSection::default());
    assert_eq!(ids(&store.list()), ["good"]);
    // The same schedule in a request is refused, with the field named.
    let unknown = json!({"type": "once", "at": minutes(60).to_rfc3339(), "repeat": true});
    let error = TaskInput::parse(json!({"what": "job", "schedule": unknown.clone()})).and_then(|i| i.into_task(&TasksSection::default(), now())).unwrap_err();
    assert!(error.to_string().contains("unknown field `repeat`, expected `at`"), "{error}");
    assert_eq!(Schedule::from_value(unknown).unwrap_err(), "a once schedule: unknown field `repeat`, expected `at`");
}

#[test]
fn a_once_more_than_a_minute_late_and_a_run_six_minutes_late_are_missed() {
    // Scenario S28 item 2.
    let mut r = rig();
    let once = |r: &Rig| r.store.add(input(json!({"what": "call", "kind": "remind", "schedule": "in 1m"})), now()).unwrap();
    let a = once(&r);
    assert_eq!(a.next_run, Some(minutes(1)));
    assert_eq!(ids(&r.store.claim(minutes(1) + Duration::seconds(30), 2)), [a.id.as_str()], "half a minute late it still runs");
    assert!(r.store.get(&a.id).unwrap().done, "and a once is done once it runs");
    let b = once(&r);
    assert_eq!(ids(&r.store.claim(minutes(2), 2)), [b.id.as_str()], "a minute late is still on time");
    let c = once(&r);
    while r.events.try_recv().is_ok() {}
    assert!(r.store.claim(minutes(2) + Duration::seconds(1), 2).is_empty(), "later than that it is not run");
    let c = r.store.get(&c.id).unwrap();
    assert!(c.done && c.next_run.is_none());
    assert_eq!(c.runs, [TaskRun { at: minutes(1), status: RunStatus::Missed, duration_sec: None, session_id: None, result: None }]);
    assert_eq!((c.last_status, c.last_result.as_deref()), (Some(RunStatus::Missed), Some("missed: the daemon was down or busy")));
    assert!(matches!(r.events.try_recv(), Ok(AgentEvent::TasksChanged)));
    assert!(matches!(r.events.try_recv(), Ok(AgentEvent::TaskRun { ref run, .. }) if run["status"] == "missed"));

    // A daily task at ten: claimed at 10:05:00 it runs; at 10:06 it is missed and it
    // goes on with tomorrow, without a replay.
    let daily = |r: &Rig| r.store.add(input(json!({"what": "report", "kind": "remind", "schedule": "daily 10:00"})), minutes(-60)).unwrap();
    let on_time = daily(&r);
    assert_eq!(on_time.next_run, Some(now()));
    assert_eq!(ids(&r.store.claim(now() + Duration::minutes(5), 2)), [on_time.id.as_str()]);
    let late = daily(&r);
    assert!(r.store.claim(now() + Duration::minutes(6), 2).is_empty());
    let late = r.store.get(&late.id).unwrap();
    assert_eq!((late.runs[0].status, late.runs[0].at, late.next_run, late.done), (RunStatus::Missed, now(), Some(minutes(1440)), false));
    assert!(r.store.claim(now() + Duration::minutes(7), 2).is_empty(), "the missed run is not replayed");
}

#[test]
fn three_runs_do_not_drift() {
    let mut r = rig();
    let t = r.store.add(input(json!({"what": "ping", "kind": "remind", "schedule": "every 30m"})), now()).unwrap();
    let mut slot = minutes(30);
    for run in 1..=3 {
        // Claimed 7 seconds late; the run takes 4 minutes.
        let due = r.store.claim(slot + Duration::seconds(7), 2);
        assert_eq!(ids(&due), [t.id.as_str()]);
        slot = minutes(30 * (run + 1));
        assert_eq!(due[0].next_run, Some(slot), "run {run}: the next slot is 30 minutes after this one");
        r.store.finish(&t.id, RunStatus::Ok, "reminded: ping", None, 240, slot - Duration::minutes(30) + Duration::seconds(247));
    }
    let t = r.store.get(&t.id).unwrap();
    let started = [30, 60, 90].map(|m| minutes(m) + Duration::seconds(7));
    assert_eq!(t.runs.iter().map(|run| run.at).collect::<Vec<_>>(), started);
    assert_eq!((t.runs[2].duration_sec, t.runs[2].result.as_deref()), (Some(240), Some("reminded: ping")));
    assert_eq!((t.runs.len(), t.total_runs(), t.next_run), (3, 3, Some(minutes(120))));
    let mut last = None;
    while let Ok(event) = r.events.try_recv() {
        last = Some(event);
    }
    assert!(matches!(last, Some(AgentEvent::TaskRun { .. })), "the last event of a run is taskRun");
}

#[test]
fn delay_minutes_is_computed_from_the_clock() {
    // Scenario S28 item 4: "remind me in a minute".
    let r = rig();
    let t = r.store.add(input(json!({"title": "Stretch", "what": "Stand up and stretch", "kind": "remind", "delayMinutes": 1})), now()).unwrap();
    assert_eq!(t.schedule, Some(Schedule::Once { at: minutes(1) }));
    assert_eq!((t.next_run, t.schedule_text().as_str()), (Some(minutes(1)), "once Mon 23 Nov 10:01"));
    assert!(r.store.claim(now(), 2).is_empty(), "not yet");
    assert_eq!(ids(&r.store.claim(minutes(1), 2)), [t.id.as_str()]);
    // The tool names the field in snake_case.
    let five = TaskInput::parse(json!({"what": "x", "delay_minutes": 5})).unwrap().into_task(&TasksSection::default(), now()).unwrap();
    assert_eq!(five.next_run, Some(minutes(5)));
    // A blank schedule next to a delay is no schedule.
    let blank = TaskInput::parse(json!({"what": "x", "schedule": "", "delayMinutes": 1})).unwrap().into_task(&TasksSection::default(), now()).unwrap();
    assert_eq!(blank.next_run, Some(minutes(1)));
    for bad in [json!({"what": "x", "delayMinutes": 0}), json!({"what": "x", "delayMinutes": 525601}), json!({"what": "x", "schedule": "daily 09:00", "delayMinutes": 5}), json!({"what": "x", "delayMinutes": 1.5})] {
        assert!(matches!(TaskInput::parse(bad.clone()).and_then(|i| i.into_task(&TasksSection::default(), now())), Err(TaskError::Invalid(_))), "{bad}");
    }
}

#[test]
fn policy_values_are_checked_and_default_from_settings() {
    let refused = |body: Value, needle: &str| {
        let error = TaskInput::parse(body.clone()).and_then(|i| i.into_task(&TasksSection::default(), now())).unwrap_err();
        assert!(error.to_string().contains(needle), "{body}: {error}");
    };
    refused(json!({"what": "x", "deliver": ["pigeon"]}), "deliver: unknown channel 'pigeon' (desktop, session, telegram)");
    refused(json!({"what": "x", "onApproval": "ignore"}), "unknown variant `ignore`, expected `deny` or `wait`");
    refused(json!({"what": "x", "session": "always"}), "unknown variant `always`, expected `new` or `reuse`");
    refused(json!({"what": "x", "kind": "chore"}), "kind must be agent or remind, not 'chore'");
    refused(json!({"what": "x", "timeoutMin": 0}), "timeoutMin must be between 1 and 1440, not 0");
    refused(json!({"what": "x", "timeoutMin": 1441}), "between 1 and 1440");
    refused(json!({"what": ""}), "say what to do");
    refused(json!({"what": []}), "say what to do");
    refused(json!({"what": "x", "repeat": "weekly"}), "unknown field `repeat`");
    refused(json!({"what": "x", "schedule": "sometimes"}), schedule::EXAMPLES);
    refused(json!({"what": "x", "schedule": {"type": "once", "at": minutes(-1).to_rfc3339()}}), "is in the past");
    refused(json!({"what": "x", "schedule": {"type": "weekly"}}), "unknown schedule type 'weekly'");

    // A field left out takes the value of settings.tasks, read at the request.
    let dir = tempfile::tempdir().unwrap();
    let (events, _receiver) = broadcast::channel(16);
    let mut defaults = TasksSection::default();
    defaults.deliver = vec!["session".into()];
    defaults.on_approval = TaskApproval::Wait;
    defaults.timeout_min = 45;
    let store = TaskStore::load(dir.path(), events, defaults.clone());
    store.set_defaults(Arc::new(move || defaults.clone()));
    let t = store.add(input(json!({"what": ["Check the backups.", "Reply NOTHING when all is well."], "kind": "agent"})), now()).unwrap();
    assert_eq!(t.what, "Check the backups.\nReply NOTHING when all is well.", "lines are joined");
    assert_eq!(t.title, "Check the backups.\nReply NOTHING when all is well.", "no title: the first words of what");
    assert_eq!((t.deliver, t.on_approval, t.session, t.timeout_min), (vec!["session".to_string()], TaskApproval::Wait, SessionPolicy::New, 45));
    assert!((t.enabled, t.done) == (true, false) && t.schedule.is_none());
    // Without the source: the built-in defaults.
    let (events, _receiver) = broadcast::channel(16);
    let plain = TaskStore::load(dir.path(), events, TasksSection::default());
    let t = plain.add(input(json!({"what": "x"})), now()).unwrap();
    assert_eq!((t.deliver, t.on_approval, t.timeout_min), (vec!["desktop".to_string(), "session".to_string()], TaskApproval::Deny, 30));
}

#[test]
fn settings_tasks_are_checked_on_a_patch() {
    use crate::settings::Settings;
    let mut s = Settings::defaults();
    assert_eq!(s.tasks_config(), TasksSection::default());
    s.patch(json!({"tasks": {"onApproval": "wait", "deliver": ["session"], "timeoutMin": 45, "maxConcurrent": 3}})).unwrap();
    assert_eq!(s.tasks_config(), TasksSection { on_approval: TaskApproval::Wait, deliver: vec!["session".into()], timeout_min: 45, max_concurrent: 3 });
    for (patch, message) in [
        (json!({"tasks": {"timeoutMin": 0}}), "invalid settings.tasks: timeoutMin must be between 1 and 1440, not 0"),
        (json!({"tasks": {"timeoutMin": 1441}}), "invalid settings.tasks: timeoutMin must be between 1 and 1440, not 1441"),
        (json!({"tasks": {"maxConcurrent": 0}}), "invalid settings.tasks: tasks.maxConcurrent must be between 1 and 8, not 0"),
        (json!({"tasks": {"maxConcurrent": 9}}), "invalid settings.tasks: tasks.maxConcurrent must be between 1 and 8, not 9"),
        (json!({"tasks": {"deliver": ["pigeon"]}}), "invalid settings.tasks: deliver: unknown channel 'pigeon' (desktop, session, telegram)"),
    ] {
        assert_eq!(s.patch(patch).unwrap_err(), message);
    }
    assert_eq!(s.tasks_config().timeout_min, 45, "a refused patch changes nothing");
}

#[test]
fn run_now_cancel_and_delete() {
    let r = rig();
    let todo = r.store.add(input(json!({"what": "Clean Downloads", "kind": "agent"})), now()).unwrap();
    assert!(todo.schedule.is_none() && todo.next_run.is_none() && todo.schedule_text() == schedule::ON_DEMAND);
    assert!(r.store.claim(now(), 2).is_empty(), "a to-do is not due by itself");
    r.store.request_run(&todo.id).unwrap();
    assert_eq!(ids(&r.store.claim(now(), 2)), [todo.id.as_str()]);
    assert_eq!(r.store.request_run(&todo.id).unwrap_err(), TaskError::Busy("the task is already running".into()));
    // Cancel: the run is recorded `cancelled`, and a to-do is not done after it.
    assert_eq!(r.store.cancel(&todo.id).unwrap(), true);
    r.store.finish(&todo.id, RunStatus::Cancelled, "cancelled", None, 3, now());
    let after = r.store.get(&todo.id).unwrap();
    assert_eq!((after.done, after.last_status, after.runs.len()), (false, Some(RunStatus::Cancelled), 1));
    // A "run now" that was not started yet is dropped.
    r.store.request_run(&todo.id).unwrap();
    assert_eq!(r.store.cancel(&todo.id).unwrap(), true);
    assert!(r.store.claim(now(), 2).is_empty());
    assert_eq!(r.store.cancel(&todo.id).unwrap(), false, "nothing to cancel");
    assert_eq!(r.store.cancel("nope").unwrap_err(), TaskError::NotFound("nope".into()));
    // "Run now" on a paused task with a schedule keeps the schedule and the pause.
    let daily = r.store.add(input(json!({"what": "ping", "kind": "remind", "schedule": "daily 10:00"})), minutes(-60)).unwrap();
    r.store.update(&daily.id, json!({"enabled": false}), now()).unwrap();
    r.store.request_run(&daily.id).unwrap();
    let due = r.store.claim(now(), 2);
    assert_eq!(ids(&due), [daily.id.as_str()]);
    assert!(due[0].next_run.is_some() && !due[0].enabled, "still scheduled, still paused");
    // Deleting a task that runs ends its record.
    assert!(r.store.delete(&daily.id).is_ok());
    assert!(r.store.finish(&daily.id, RunStatus::Ok, "late", None, 1, now()).is_none());
    assert!(r.store.list().iter().all(|t| t.id != daily.id));
}

#[test]
fn resuming_after_its_time_starts_from_then_on() {
    let r = rig();
    let t = r.store.add(input(json!({"what": "ping", "kind": "remind", "schedule": "daily 10:00"})), minutes(-60)).unwrap();
    r.store.update(&t.id, json!({"enabled": false}), now()).unwrap();
    let later = now() + Duration::days(2);
    let resumed = r.store.update(&t.id, json!({"enabled": true}), later).unwrap();
    assert!(resumed.enabled);
    assert_eq!(resumed.next_run, Some(now() + Duration::days(3)), "the next slot after the resume; nothing counts as missed");
    assert!(r.store.claim(later, 2).is_empty());
    // Paused again with its slot still ahead: the slot is kept.
    r.store.update(&t.id, json!({"enabled": false}), later).unwrap();
    let kept = r.store.update(&t.id, json!({"enabled": true}), later + Duration::hours(1)).unwrap();
    assert_eq!(kept.next_run, Some(now() + Duration::days(3)));
    // A new schedule; and the schedule removed: a to-do again.
    let changed = r.store.update(&t.id, json!({"schedule": "every 30m"}), later).unwrap();
    assert_eq!(changed.next_run, Some(later + Duration::minutes(30)));
    let todo = r.store.update(&t.id, json!({"schedule": null}), later).unwrap();
    assert!(todo.schedule.is_none() && todo.next_run.is_none() && !todo.done);
}

#[test]
fn startup_marks_runs_left_running_failed() {
    // Scenario S28: "startup marks stale runs failed".
    let r = rig();
    let t = r.store.add(input(json!({"what": "ping", "kind": "remind", "schedule": "daily 10:00"})), minutes(-60)).unwrap();
    r.store.claim(now(), 2);
    let running = r.store.get(&t.id).unwrap();
    assert_eq!((running.last_status, running.runs[0].status), (Some(RunStatus::Running), RunStatus::Running));
    // What a daemon that stopped in the middle of that run left in its file.
    let (events, mut receiver) = broadcast::channel(16);
    let again = TaskStore::load(r._dir.path(), events, TasksSection::default());
    assert_eq!(again.fail_stale(), 1);
    assert_eq!(again.fail_stale(), 0, "once");
    assert!(matches!(receiver.try_recv(), Ok(AgentEvent::TasksChanged)));
    let after = again.get(&t.id).unwrap();
    assert_eq!((after.last_status, after.last_result.as_deref()), (Some(RunStatus::Failed), Some("daemon restarted")));
    assert_eq!((after.runs[0].status, after.runs[0].result.as_deref()), (RunStatus::Failed, Some("daemon restarted")));
    assert_eq!(after.next_run, Some(minutes(1440)), "and the task still has its next slot");
    // A file from before S28: a count, and `lastStatus` left running.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tasks.json"), serde_json::to_vec(&json!([{"id": "t9", "title": "t9", "what": "job", "kind": "agent", "schedule": {"type": "Every", "minutes": 20}, "enabled": true, "createdAt": now().to_rfc3339(), "lastStatus": "running", "lastResult": "old", "runs": 4, "done": false}])).unwrap()).unwrap();
    let (events, _receiver) = broadcast::channel(16);
    let store = TaskStore::load(dir.path(), events, TasksSection::default());
    assert_eq!(store.fail_stale(), 1);
    let after = store.list().remove(0);
    assert_eq!((after.last_status, after.last_result.as_deref(), after.total_runs()), (Some(RunStatus::Failed), Some("daemon restarted"), 4));
}

#[test]
fn the_history_keeps_the_last_fifty_and_cuts_long_results() {
    let r = rig();
    let t = r.store.add(input(json!({"what": "x", "kind": "agent"})), now()).unwrap();
    for i in 0..60 {
        r.store.request_run(&t.id).unwrap();
        assert_eq!(r.store.claim(now(), 2).len(), 1);
        r.store.finish(&t.id, RunStatus::Ok, &format!("run {i} {}", "x".repeat(1000)), None, 1, now());
    }
    let t = r.store.get(&t.id).unwrap();
    assert_eq!(t.runs.len(), 50);
    let first = t.runs[0].result.as_deref().unwrap();
    assert!(first.starts_with("run 10 ") && first.chars().count() == 500, "{first}");
    assert_eq!(t.last_result.as_deref().unwrap().chars().count(), 1007);
    assert_eq!(t.total_runs(), 50);
    assert_eq!(r.store.runs(&t.id).unwrap().len(), 50);
}
