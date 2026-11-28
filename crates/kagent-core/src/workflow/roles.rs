//! The profiles of the three worker roles (workflows.md section 3.1), built into the engine.

use crate::agent::children::Profile;

pub const PLANNER: &str = "planner";
pub const IMPLEMENTER: &str = "implementer";
pub const REVIEWER: &str = "reviewer";

const PLANNER_PROMPT: &str = "You are the planner of a workflow run. Read what you need of the project (you cannot change anything), then split the job into tasks that one worker can each do and check. Your last message must hold the plan as one fenced json block, in the form the job describes, and nothing in it may be invented: name only files you saw.";
const IMPLEMENTER_PROMPT: &str = "You are the implementer of one task of a workflow run. Do that task and only that task, in the folder of the project. Check your work (run the project's own tests or build when it has them). End with a report: what changed (files), how you checked it (commands and results), what is left. You do not decide that the task is done: a reviewer does.";
const REVIEWER_PROMPT: &str = "You are the reviewer of one task of a workflow run. You cannot change anything. Read the diff and the files, and check the task's done-when lines. Your last message must hold the verdict as one fenced json block, in the form the job describes. Report as `must` only what makes the task not done; style and ideas are `should`.";

/// The profile of a role, `None` for any other name.
pub fn profile(name: &str) -> Option<Profile> {
    let (description, read_only, iterations, prompt) = match name {
        PLANNER => ("Splits a job into tasks; read-only.", true, 40, PLANNER_PROMPT),
        IMPLEMENTER => ("Does one task of a run and reports.", false, 60, IMPLEMENTER_PROMPT),
        REVIEWER => ("Reviews one task of a run; read-only.", true, 40, REVIEWER_PROMPT),
        _ => return None,
    };
    Some(Profile { name: name.into(), description: description.into(), model: String::new(), tool_groups: Vec::new(), max_iterations: iterations, prompt: prompt.into(), read_only })
}
