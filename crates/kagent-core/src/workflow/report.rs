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
