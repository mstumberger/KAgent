//! Background tasks (agents.md section 7): the output cap keeps the tail, the limit of 20,
//! stop-all kills a `sleep`, a task tells its session how it ended unless it was killed,
//! and the tools that read and stop a task the `shell` tool started.
//!
//! Each test sleeps for its own number of seconds (`sleep 47`, `48`, `49`): the tests of
//! this file run at the same time, and `exec.rs` counts the `sleep 60` of its own test.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::*;
use crate::tool::{PrivilegeMethod, Tool, ToolContext};
use crate::tools::shell::Shell;
use crate::tools::tasktools::{TaskOutput, TaskStop};
use tokio_util::sync::CancellationToken;

const SLEEP: &str = "sleep 47";
const SLEEP_ONCE: &str = "sleep 48";
const SLEEP_TOOL: &str = "sleep 49";

fn ctx(session: &str) -> ToolContext {
    ToolContext {
        cancel: CancellationToken::new(),
        timeout: Duration::from_secs(30),
        privilege: PrivilegeMethod::None,
        max_output_bytes: 24_000,
        session_id: session.into(),
        call_id: "call".into(),
        services: None,
    }
}

/// Processes running exactly `command` (its whole command line).
fn running_count(command: &str) -> usize {
    std::process::Command::new("pgrep")
        .args(["-c", "-f", &format!("^{command}$")])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0))
        .unwrap_or(0)
}

async fn until(secs: u64, what: &str, mut pred: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !pred() {
        assert!(Instant::now() < end, "deadline: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn ended(bg: &Background, id: &str) -> (TaskInfo, String) {
    until(10, "the task ends", || bg.info(id).is_some_and(|i| i.status != TaskStatus::Running)).await;
    bg.output(id, 4_000).await.expect("the task is kept")
}

#[test]
fn what_is_said_about_a_task() {
    let info = |status, code| TaskInfo { id: "bg_1a2b3c4d".into(), status, exit_code: code, seconds: 5, output_file: "/data/task-out/bg_1a2b3c4d.log".into() };
    assert_eq!(info(TaskStatus::Running, None).head(), "[Task bg_1a2b3c4d running for 5 s]");
    assert_eq!(info(TaskStatus::Done, Some(0)).head(), "[Task bg_1a2b3c4d done, exit 0, after 5 s]");
    assert_eq!(info(TaskStatus::Failed, Some(3)).head(), "[Task bg_1a2b3c4d failed, exit 3, after 5 s]");
    assert_eq!(info(TaskStatus::Failed, None).head(), "[Task bg_1a2b3c4d failed, exit signal, after 5 s]");
    assert_eq!(info(TaskStatus::Killed, None).head(), "[Task bg_1a2b3c4d killed, after 5 s]");
    // The notification of agents.md section 4.
    assert_eq!(info(TaskStatus::Done, Some(0)).notice("out\n"), "[Task bg_1a2b3c4d finished, exit 0]\nout\n");
    assert_eq!(info(TaskStatus::Done, Some(0)).notice("  \n"), "[Task bg_1a2b3c4d finished, exit 0]\n", "no output: the line alone");
    assert_eq!(serde_json::to_value(TaskStatus::Running).unwrap(), Value::String("running".into()));
    assert_eq!(serde_json::to_value(info(TaskStatus::Done, Some(0))).unwrap()["outputFile"], "/data/task-out/bg_1a2b3c4d.log");
}

#[tokio::test]
async fn the_output_file_keeps_its_last_mib() {
    let dir = tempfile::tempdir().unwrap();
    let bg = Background::new(dir.path());
    // 30 000 lines of 81 bytes: 2.4 MiB, past the cap and the slack after it.
    let line = "0123456789".repeat(8);
    let info = bg.start("s", &format!("for i in $(seq 1 30000); do echo \"$i {line}\"; done; echo LAST-LINE"), None).await.unwrap();
    let (done, tail) = ended(&bg, &info.id).await;
    assert_eq!((done.status, done.exit_code), (TaskStatus::Done, Some(0)));
    assert!(tail.ends_with("LAST-LINE\n"), "the tail ends {:?}", &tail[tail.len().saturating_sub(40)..]);
    assert_eq!(done.output_file, dir.path().join(format!("{}.log", info.id)).display().to_string());
    let raw = std::fs::read_to_string(&done.output_file).unwrap();
    assert!(raw.len() <= MAX_FILE_BYTES as usize, "the file is {} bytes", raw.len());
    assert!(raw.len() > MAX_FILE_BYTES as usize - 4096, "and not cut shorter than it needs to be: {} bytes", raw.len());
    let first: u32 = raw.lines().next().unwrap().split(' ').next().unwrap().parse().unwrap();
    assert!(first > 10_000, "the first lines are gone and the last ones kept: the file starts at line {first}");
    assert!(raw.starts_with(&format!("{first} {line}\n")), "a whole first line: {}", raw.lines().next().unwrap());
    assert_eq!(bg.running(), 0);
    // The id is `bg_` and 8 hex digits; the tail is in characters.
    assert!(info.id.starts_with("bg_") && info.id.len() == 11 && info.id[3..].chars().all(|c| c.is_ascii_hexdigit()), "{}", info.id);
    assert_eq!(bg.output(&info.id, 10).await.unwrap().1, "LAST-LINE\n", "the last 10 characters, the newline the command ended with included");
}

#[tokio::test]
async fn at_most_twenty_tasks_run_and_stop_all_kills_them() {
    let dir = tempfile::tempdir().unwrap();
    let bg = Background::new(dir.path());
    let mut ids = Vec::new();
    for _ in 0..MAX_RUNNING {
        ids.push(bg.start("s", SLEEP, None).await.expect("a task starts").id);
    }
    assert_eq!(bg.running(), MAX_RUNNING);
    let refused = bg.start("s", SLEEP, None).await.err().expect("the 21st task is refused");
    assert!(refused.contains("too many background tasks: 20 are running"), "{refused}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), MAX_RUNNING, "nothing was started for the refused one");

    until(5, "the twenty commands run", || running_count(SLEEP) >= MAX_RUNNING).await;
    assert_eq!(bg.stop_all(), MAX_RUNNING, "every running task is stopped");
    // TERM, then KILL after 2 s (exec.rs): gone well before that.
    until(4, "the killed commands are gone", || running_count(SLEEP) == 0).await;
    assert_eq!(bg.running(), 0, "a stopped task frees its slot at once");
    until(4, "the killed tasks are recorded", || ids.iter().all(|id| bg.info(id).is_some_and(|i| i.status == TaskStatus::Killed))).await;
    assert_eq!(bg.stop_all(), 0, "nothing runs any more");
    assert!(!bg.stop(&ids[0]), "and one of them is not running");
    // The slot is free again: a new task starts.
    let (info, _) = ended(&bg, &bg.start("s", "true", None).await.unwrap().id).await;
    assert_eq!(info.status, TaskStatus::Done);
}

#[tokio::test]
async fn a_task_tells_its_session_how_it_ended_unless_it_was_killed() {
    let dir = tempfile::tempdir().unwrap();
    let bg = Background::new(dir.path());
    let told: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let sink = told.clone();
    bg.configure(dir.path(), Arc::new(move |session, text| sink.lock().unwrap().push((session, text))));

    let failed = bg.start("sess-1", "echo first; echo second >&2; exit 3", None).await.unwrap();
    let (info, tail) = ended(&bg, &failed.id).await;
    assert_eq!((info.status, info.exit_code), (TaskStatus::Failed, Some(3)));
    assert!(info.head().starts_with(&format!("[Task {} failed, exit 3, after", failed.id)), "{}", info.head());
    let mut lines: Vec<&str> = tail.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["first", "second"], "stdout and stderr, each as it came");
    let first = told.lock().unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!((first[0].0.as_str(), first[0].1.lines().next()), ("sess-1", Some(format!("[Task {} finished, exit 3]", failed.id).as_str())));
    let mut lines: Vec<&str> = first[0].1.lines().skip(1).collect();
    lines.sort_unstable();
    assert_eq!(lines, ["first", "second"], "the message carries the output after its line");

    // A cwd is honoured; exit 0 tells too.
    let ok = bg.start("sess-2", "pwd", Some(dir.path().display().to_string())).await.unwrap();
    let (info, tail) = ended(&bg, &ok.id).await;
    assert_eq!((info.status, info.exit_code), (TaskStatus::Done, Some(0)));
    assert_eq!(tail.trim(), dir.path().display().to_string());
    drop(first);
    assert_eq!(told.lock().unwrap().len(), 2);
    assert!(told.lock().unwrap()[1].1.starts_with(&format!("[Task {} finished, exit 0]\n", ok.id)));

    // Stopped on purpose: no word to the session, and it is `killed`.
    let killed = bg.start("sess-3", SLEEP_ONCE, None).await.unwrap();
    until(5, "the command runs", || running_count(SLEEP_ONCE) == 1).await;
    assert!(bg.stop(&killed.id));
    until(4, "the killed command is gone", || running_count(SLEEP_ONCE) == 0).await;
    let (info, _) = ended(&bg, &killed.id).await;
    assert_eq!(info.status, TaskStatus::Killed, "not `failed` although it died of a signal");
    assert!(info.head().starts_with(&format!("[Task {} killed, after", killed.id)), "{}", info.head());
    assert_eq!(told.lock().unwrap().len(), 2, "a killed task tells nobody");

    // A command that fails tells its exit code.
    let error = bg.start("s", "exit 7", None).await.unwrap();
    assert_eq!(ended(&bg, &error.id).await.0.exit_code, Some(7));
}

#[tokio::test]
async fn the_shell_tool_starts_a_task_and_the_task_tools_read_and_stop_it() {
    let dir = tempfile::tempdir().unwrap();
    let bg = Background::new(dir.path());
    let shell = Shell::with_tasks(bg.clone());
    let out = shell.call(json!({"command": "echo line-1; echo line-2; echo line-3", "background": true}), ctx("sess-tools")).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let started: Value = serde_json::from_str(&out.content).expect("the answer is {taskId, outputFile}");
    let id = started["taskId"].as_str().expect("taskId");
    assert_eq!(started["outputFile"], json!(dir.path().join(format!("{id}.log"))));
    assert!(id.starts_with("bg_"));

    // `task_output` waits for the end (wait_sec) and shows the output.
    let output = TaskOutput::new(bg.clone());
    let out = output.call(json!({"task_id": id, "wait_sec": 5, "tail": 1000}), ctx("sess-tools")).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.starts_with(&format!("[Task {id} done, exit 0, after")), "{}", out.content);
    assert!(out.content.ends_with("line-1\nline-2\nline-3"), "the output without the newline the command ended with: {}", out.content);
    // The tail is the last `tail` characters: eight lines of 74, cut to the last 100.
    let out = shell.call(json!({"command": "for i in 1 2 3 4 5 6 7 8; do echo \"0123456789-$i-012345678901234567890123456789012345678901234567890123456789\"; done", "background": true}), ctx("sess-tools")).await.unwrap();
    let id = serde_json::from_str::<Value>(&out.content).unwrap()["taskId"].as_str().unwrap().to_string();
    let out = output.call(json!({"task_id": id, "wait_sec": 5, "tail": 100}), ctx("sess-tools")).await.unwrap();
    let (head, shown) = out.content.split_once('\n').unwrap();
    assert!(head.starts_with(&format!("[Task {id} done, exit 0")), "{head}");
    assert!(shown.ends_with("\n0123456789-8-012345678901234567890123456789012345678901234567890123456789") && shown.chars().count() <= 100, "{}", shown);
    assert!(shown.lines().count() == 2 && !shown.starts_with("0123456789-"), "the first lines are gone and the last cut: {:?}", shown);
    assert_eq!(output.call(json!({"task_id": id, "tail": 100000}), ctx("sess-tools")).await.unwrap().content.lines().count(), 1 + 8, "a tail over the range is read as the nearest end (20000): all eight lines");
    // A task of another day.
    let out = output.call(json!({"task_id": "bg_00000000"}), ctx("sess-tools")).await.unwrap();
    assert!(out.is_error && out.content.contains("no background task 'bg_00000000'"), "{}", out.content);

    // `task_stop` kills a running task.
    let out = shell.call(json!({"command": SLEEP_TOOL, "background": true}), ctx("sess-tools")).await.unwrap();
    let id = serde_json::from_str::<Value>(&out.content).unwrap()["taskId"].as_str().unwrap().to_string();
    until(5, "the command runs", || running_count(SLEEP_TOOL) == 1).await;
    let stop = TaskStop::new(bg.clone());
    let out = stop.call(json!({"task_id": id}), ctx("sess-tools")).await.unwrap();
    assert!(out.content.contains("killed"), "{}", out.content);
    until(4, "the killed command is gone", || running_count(SLEEP_TOOL) == 0).await;
    assert_eq!(ended(&bg, &id).await.0.status, TaskStatus::Killed);
    assert!(stop.call(json!({"task_id": id}), ctx("sess-tools")).await.unwrap().is_error, "not running any more");

    // Without the flag the shell tool answers as before: the command's own output.
    let out = shell.call(json!({"command": "echo plain"}), ctx("sess-tools")).await.unwrap();
    assert_eq!(out.content, "plain\n");
    // The flag does not change what the call may do: `ls` is read-only either way.
    assert!(shell.assess(&json!({"command": "ls", "background": true})).read_only);
    assert!(!shell.assess(&json!({"command": "rm x", "background": true})).read_only);
}
