//! The texts the engine sends to its workers (workflows.md section 3). Data and rules for the worker, never an approval.

use super::model::{Finding, Severity, Task};
use crate::agent::children::cut;

const PLAN_FORMAT: &str = "```json\n{\"summary\": \"one paragraph\", \"tasks\": [\n  {\"id\": \"T1\", \"title\": \"short title\", \"description\": \"what to do\", \"done_when\": [\"a checkable line\"], \"depends_on\": [], \"files\": [\"path/you/expect/to/change\"]}\n]}\n```";
const VERDICT_FORMAT: &str = "```json\n{\"verdict\": \"pass\", \"findings\": []}\n```\nor, when something must change:\n```json\n{\"verdict\": \"changes\", \"findings\": [{\"severity\": \"must\", \"file\": \"path\", \"line\": 12, \"what\": \"what is wrong\", \"why\": \"why it matters\"}]}\n```\n`changes` needs at least one `must` finding; `pass` has none. Use `should` for what is only worth saying.";

pub fn planner(goal: &str, project: &str, feedback: Option<&str>) -> String {
    let mut out = format!("The job:\n{}\n\nThe project folder: {project}\n\nSplit the job into 1 to 30 tasks that one worker can each do and check, in the order they can be done. Each task has an id (letters, digits, `.`, `_`, `-`; unique), a title, a description, 1 to 8 `done_when` lines that can be checked, the ids it `depends_on`, and the `files` you expect it to change. Your last message must hold the plan as ONE fenced json block in this form:\n{PLAN_FORMAT}\n", goal.trim());
    if let Some(f) = feedback {
        out.push_str(&format!("\nYou planned this before and the owner rejected the plan: {}\nReturn a new plan that takes this into account.\n", f.trim()));
    }
    out
}

pub fn plan_retry(problems: &str) -> String {
    format!("Your plan could not be used:\n{problems}\n\nReturn the plan as the JSON block described: one fenced json block with `summary` and `tasks`, in the form:\n{PLAN_FORMAT}")
}

pub fn rejected(reason: &str) -> String {
    format!("The owner rejected your plan: {}\nReturn a new plan as the JSON block described (one fenced json block, the same form as before).", reason.trim())
}

/// The summaries of the tasks `task` depends on.
pub fn dependency_lines(done: &[(&Task, String)]) -> String {
    done.iter().map(|(t, report)| format!("- {}: {}\n  {}", t.id, t.title, cut(report.trim(), 1500).replace('\n', "\n  "))).collect::<Vec<_>>().join("\n")
}

pub fn findings_text(findings: &[&Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            let at = if f.file.is_empty() { String::new() } else if f.line > 0 { format!(" ({}:{})", f.file, f.line) } else { format!(" ({})", f.file) };
            let why = if f.why.trim().is_empty() { String::new() } else { format!(" Why: {}", f.why.trim()) };
            format!("- {}{at}: {}{why}", if f.severity == Severity::Must { "MUST" } else { "should" }, f.what.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn implementer(goal: &str, task: &Task, deps: &str, findings: &[&Finding], restarted: bool) -> String {
    let mut out = format!("The run's goal:\n{}\n\nYour task {}: {}\n{}\n\nDone when:\n{}\n", goal.trim(), task.id, task.title, task.description.trim(), task.done_when.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"));
    if !task.files.is_empty() {
        out.push_str(&format!("\nFiles you are expected to touch: {}\n", task.files.join(", ")));
    }
    if !deps.is_empty() {
        out.push_str(&format!("\nTasks this one depends on are done:\n{deps}\n"));
    }
    if !findings.is_empty() {
        out.push_str(&format!("\nA reviewer found these problems in an earlier attempt; fix every MUST one:\n{}\n", findings_text(findings)));
    }
    if restarted {
        out.push_str("\nAn earlier attempt at this task was interrupted: some files may already be changed. Look at what is there before you change anything.\n");
    }
    out.push_str("\nWhen you are done, end with a report: what changed (files), how you checked it (commands and results), what is left.");
    out
}

pub fn findings_back(round: u32, findings: &[&Finding]) -> String {
    format!("A reviewer checked your work (round {}) and found these problems; fix every MUST one, then end with a report again (what changed, how you checked it, what is left):\n{}", round.saturating_sub(1), findings_text(findings))
}

pub fn reviewer(goal: &str, task: &Task, report: &str, diff: &str) -> String {
    let mut out = format!("The run's goal:\n{}\n\nReview task {}: {}\n{}\n\nDone when:\n{}\n", goal.trim(), task.id, task.title, task.description.trim(), task.done_when.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"));
    if !report.trim().is_empty() {
        out.push_str(&format!("\nThe implementer's report:\n{}\n", cut(report.trim(), 6000)));
    }
    out.push_str(&format!("\nThe changes (git diff against the last commit):\n{}\n", if diff.trim().is_empty() { "(nothing to show: read the files)" } else { diff.trim() }));
    out.push_str(&format!("\nYou cannot change anything. Check every done-when line. Your last message must hold the verdict as ONE fenced json block:\n{VERDICT_FORMAT}"));
    out
}

pub fn verdict_retry(problem: &str) -> String {
    format!("Your verdict could not be used: {problem}\nReturn the verdict as the JSON block described: one fenced json block.\n{VERDICT_FORMAT}")
}

/// A question as the Brain's session is told it.
pub fn question_for_brain(run: &str, title: &str, role: &str, task: &str, qid: &str, text: &str, options: &[String]) -> String {
    let opts = if options.is_empty() { String::new() } else { format!(" Options: {}.", options.join(" | ")) };
    format!("[question from {role} {task} in run {run} \"{title}\"] {text}{opts} (question {qid}: answer with workflow_answer, or pass it on with ask_owner)")
}
