//! The run's report (`do: summary`, workflows.md section 3.3): written by the engine from the tasks, no model.

use super::model::{Run, TaskState};
use crate::agent::children::cut;

pub fn summary(run: &Run) -> String {
    let (done, total) = run.counts();
    let mut out = format!("Run {} of {}: {done} of {total} tasks done.\n", run.id, run.workflow);
    if !run.plan_summary.is_empty() {
        out.push_str(&format!("\nPlan: {}\n", run.plan_summary));
    }
    out.push_str("\nTasks:\n");
    for t in &run.tasks {
        let rounds = if t.rounds.len() > 1 { format!(", {} rounds", t.rounds.len()) } else { String::new() };
        let accepted = if t.accepted { ", accepted by the owner" } else { "" };
        out.push_str(&format!("- {} {}: {}{rounds}{accepted}\n", t.id, t.title, t.state.word()));
        if !t.error.is_empty() {
            out.push_str(&format!("    {}\n", cut(&t.error, 300)));
        }
        if t.state == TaskState::Done || t.state == TaskState::NeedsOwner {
            let report = t.last_report().trim();
            if !report.is_empty() {
                out.push_str(&format!("    report: {}\n", cut(report, 400).replace('\n', " ")));
            }
        }
        for f in t.open_findings().into_iter().filter(|_| t.state == TaskState::Done && t.rounds.last().is_some_and(|r| r.verdict == Some(super::model::Verdict::Changes))) {
            out.push_str(&format!("    must (not fixed, no implementer): {}\n", f.what));
        }
    }
    let should: Vec<String> = run.tasks.iter().flat_map(|t| t.should_findings().into_iter().map(move |f| format!("- {} {}{}: {}", t.id, f.file, if f.line > 0 { format!(":{}", f.line) } else { String::new() }, f.what))).collect();
    if !should.is_empty() {
        out.push_str(&format!("\nFindings to look at later (should):\n{}\n", should.join("\n")));
    }
    let mut files: Vec<&str> = run.tasks.iter().flat_map(|t| t.changed_files.iter().map(String::as_str)).collect();
    files.sort_unstable();
    files.dedup();
    if !files.is_empty() {
        out.push_str(&format!("\nFiles changed: {}\n", files.join(", ")));
    }
    out
}

/// What `workflow_status` says about one run, for the Brain.
pub fn status(run: &Run) -> String {
    let (done, total) = run.counts();
    let mut out = format!("Run {} \"{}\" ({}): {}", run.id, run.title, run.workflow, run.state.word());
    if !run.reason.is_empty() {
        out.push_str(&format!(" ({})", run.reason));
    }
    out.push_str(&format!(", {done} of {total} tasks done.\n"));
    for t in &run.tasks {
        let round = if t.round > 0 && matches!(t.state, TaskState::Implementing | TaskState::InReview | TaskState::Changes) { format!(", round {}", t.round) } else { String::new() };
        out.push_str(&format!("- {} {}: {}{round}\n", t.id, t.title, t.state.word()));
    }
    for q in run.questions.iter().filter(|q| q.answer.is_none()) {
        out.push_str(&format!("Open question {} from task {}: {}\n", q.id, q.task, cut(&q.text, 300)));
    }
    if run.state == super::RunState::AwaitingApproval {
        out.push_str("The plan waits for the owner's approval.\n");
    }
    out
}

/// What `workflow_run` asks the owner: the workflow, the project and the workers.
pub fn card(def: &super::Definition, project: &str, goal: &str) -> String {
    let workers: Vec<String> = def.steps.iter().filter_map(|s| s.worker.as_ref().map(|w| format!("{} ({}{})", s.role.word(), w.profile, s.model.as_deref().map(|m| format!(", {m}")).unwrap_or_default()))).collect();
    format!("run the workflow {} in {} with {}: {}", def.name, project.rsplit('/').next().unwrap_or(project), workers.join(", "), cut(goal.trim(), 100))
}
