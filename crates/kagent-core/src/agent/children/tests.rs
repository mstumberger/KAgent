//! The pure parts of `children.rs`: the block of a child's prompt, the first message of
//! its job, what a tool says about a child, what a restricted session may do, and how
//! many turns in a row a parent ran on its own. The orchestration itself is tested with
//! the scripted model in `tests/subagents.rs`.

use serde_json::json;

use super::*;
use crate::protocol::Role;

fn view(status: ChildStatus) -> ChildView {
    ChildView::new("c1".into(), "disk usage".into(), "disk-audit".into(), status)
}

#[test]
fn the_block_names_the_job_the_limits_and_the_profile_prompt() {
    // agents.md section 2: the sentence, then the profile `prompt`.
    let mut profile = Profile::general();
    profile.prompt = "  Extra system prompt text.\n".into();
    let block = subagent_block("Check what uses the disk.", &profile);
    assert!(block.starts_with("\nYou are a subagent. Check what uses the disk. End with a complete final answer: it is returned to the agent that started you.\n"), "{block}");
    assert!(block.ends_with("\nExtra system prompt text.\n"), "the profile prompt is the last part: {block}");
    assert!(!block.contains("tool groups"), "every group: nothing to say");

    // A profile with groups is told them; `explore` is told it is read-only.
    profile.tool_groups = vec!["files".into(), "system".into()];
    let block = subagent_block("Look around", &profile);
    assert!(block.contains("Your tool groups are only: files, system. Tools outside them are refused.\n"), "{block}");
    let block = subagent_block("Look around", &Profile::explore());
    assert!(block.contains("You are read-only: every call that would change something is refused."), "{block}");
    assert_eq!(subagent_block("one-line job", &Profile::general()), "\nYou are a subagent. one-line job. End with a complete final answer: it is returned to the agent that started you.\n");
}

#[test]
fn the_job_is_preceded_by_the_parents_digest_when_there_is_one() {
    assert_eq!(job_text("User: why is the disk full?\nAssistant: let us look", "List the largest folders."), "What the conversation you were started from is about (a digest):\nUser: why is the disk full?\nAssistant: let us look\n\nYour job:\nList the largest folders.");
    assert_eq!(job_text("", "  List them.  "), "Your job:\nList them.", "without a digest the job stands alone");
}

#[test]
fn render_says_how_a_child_is_and_what_it_answered() {
    // agents.md section 3: `[Agent "<description>" <done|failed|cancelled> in 42 s]`.
    let child = view(ChildStatus::Done);
    let done = End { status: ChildStatus::Done, seconds: 42, output: "  The answer.  ".into(), error: None };
    assert_eq!(render(&child, Some(&done), 100), "[Agent \"disk usage\" done in 42 s]\nagent c1, profile disk-audit\nThe answer.\n");
    let failed = End { status: ChildStatus::Failed, seconds: 3, output: String::new(), error: Some("no chat model selected".into()) };
    assert_eq!(render(&child, Some(&failed), 100), "[Agent \"disk usage\" failed in 3 s]\nagent c1, profile disk-audit\nerror: no chat model selected\n");
    assert!(render(&child, Some(&End { output: String::new(), ..done.clone() }), 100).contains("(it gave no answer)"), "a child that ended without an answer says so");
    // The output is cut at the limit, in characters, and says where to get more.
    let long = End { output: "é".repeat(10), ..done.clone() };
    assert_eq!(render(&child, Some(&long), 4), "[Agent \"disk usage\" done in 42 s]\nagent c1, profile disk-audit\néééé\n(cut: call agent_wait with its id for more)\n");
    // A child that did not end yet.
    assert_eq!(render(&view(ChildStatus::Queued), None, 100), "[Agent \"disk usage\" queued]\nagent c1, profile disk-audit\n");
    assert_eq!(render(&view(ChildStatus::Running), None, 100), "[Agent \"disk usage\" running for 0 s]\nagent c1, profile disk-audit\n");

    // The status is as api.md section 7 names it.
    assert_eq!(serde_json::to_value(ChildStatus::Failed).unwrap(), json!("failed"));
    assert_eq!(serde_json::to_value(&view(ChildStatus::Running)).unwrap(), json!({ "id": "c1", "title": "disk usage", "profile": "disk-audit", "status": "running" }));
}

#[test]
fn a_subagent_has_its_groups_read_only_and_no_tools_over_subagents() {
    // A profile with groups keeps its tools and loses the tools over subagents.
    let groups = Restriction { groups: Some(vec!["system".into(), "agent".into()]), read_only: false, child: true };
    for tool in ["system_info", "todo_write", "plan_exit", "task_output"] {
        assert!(groups.allows(tool), "{tool} is in its groups");
    }
    for tool in ["write_file", "shell", "web_fetch"] {
        assert!(!groups.allows(tool), "{tool} is outside its groups");
    }
    for tool in ["agent_run", "agent_wait", "agent_stop", "agent_list"] {
        assert!(!groups.allows(tool), "{tool}: a subagent cannot start or manage subagents, whatever its groups");
    }
    // `general`: every group, still no tools over subagents.
    let general = Restriction { child: true, ..Default::default() };
    assert!(general.allows("write_file") && !general.allows("agent_run"));
    // A session that is not a subagent has no restriction.
    assert!(Restriction::default().allows("agent_run") && Restriction::default().allows("shell"));
    assert_eq!(Profile::builtin("explore"), Some(Profile::explore()), "explore is read-only");
    assert_ne!(Profile::builtin("general"), None);
    assert_eq!(Profile::builtin("disk-audit"), None);
}

#[test]
fn the_turns_a_parent_ran_on_its_own_are_counted_from_the_end() {
    let m = |role: Role, text: &str, via: Via| match role {
        Role::Assistant => Message::assistant(text.to_string(), vec![]).with_via(via),
        _ => Message::user(text.to_string()).with_via(via),
    };
    // Two answers to notifications in a row, then the owner's question.
    let messages = [m(Role::User, "q", Via::Text), m(Role::Assistant, "a", Via::Text), m(Role::User, "n1", Via::Agent), m(Role::Assistant, "a1", Via::Agent), m(Role::User, "n2", Via::Agent), m(Role::Assistant, "a2", Via::Agent)];
    assert_eq!(auto_turns(&messages), 2);
    // The last turn answered a Telegram message: nothing in a row.
    let with_owner = [messages.as_slice(), &[m(Role::User, "q", Via::Telegram)]].concat();
    assert_eq!(auto_turns(&with_owner), 0);
    assert_eq!(auto_turns(&[]), 0);
    // A task's result is delivered by the runner: its session is not woken.
    let mut task = SessionMeta { id: "t".into(), title: String::new(), created_at: Utc::now(), updated_at: Utc::now(), message_count: 0, origin: SessionOrigin::Task, parent: None, summary: None, summary_upto: None, todo: Vec::new(), planning: false, approved_plan: None, model: None, reasoning_level: None, project: None, pinned: false, images_notice_shown: false };
    assert!(!interactive(&task));
    task.origin = SessionOrigin::Chat;
    assert!(interactive(&task));
    task.origin = SessionOrigin::Voice;
    assert!(interactive(&task));
}

#[test]
fn a_child_running_since_a_minute_ago_ran_a_minute() {
    let mut child = view(ChildStatus::Running);
    assert_eq!(child.running_for(), 0, "not started");
    child.started_at = Some(Utc::now() - chrono::Duration::seconds(65));
    assert!((60..=66).contains(&child.running_for()), "{}", child.running_for());
    child.ended_at = child.started_at.map(|start| start + chrono::Duration::seconds(42));
    assert_eq!(child.running_for(), 42, "after it ended the time stands still");
}
