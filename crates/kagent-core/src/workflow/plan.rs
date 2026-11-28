//! The plan a planner must return (workflows.md section 3.2): find its fenced `json` block, read it, check it. Everything wrong
//! is listed at once, so the planner can be sent back once with all of it.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use super::model::{PlanDoc, Task};

pub const MAX_TASKS: usize = 30;

#[derive(Deserialize)]
struct RawPlan {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tasks: Vec<RawTask>,
}

#[derive(Deserialize)]
struct RawTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    ticket: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default, alias = "doneWhen")]
    done_when: Vec<String>,
    #[serde(default, alias = "dependsOn")]
    depends_on: Vec<String>,
    #[serde(default)]
    files: Vec<String>,
}

/// The text of the last fenced block marked `json` (or, failing that, the last fenced block that is an object).
pub fn json_block(text: &str) -> Option<String> {
    let mut blocks: Vec<(bool, String)> = Vec::new();
    let mut open: Option<(bool, String)> = None;
    for line in text.lines() {
        let t = line.trim();
        match &mut open {
            None => {
                if let Some(rest) = t.strip_prefix("```") {
                    open = Some((rest.trim().eq_ignore_ascii_case("json"), String::new()));
                }
            }
            Some((_, body)) => {
                if t.starts_with("```") {
                    blocks.push(open.take().unwrap());
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    blocks.iter().rev().find(|(json, _)| *json).or_else(|| blocks.iter().rev().find(|(_, b)| b.trim_start().starts_with('{'))).map(|(_, b)| b.clone())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= 20 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Parse and check the plan in `text`. `Err` lists every problem, one per line.
pub fn parse(text: &str) -> Result<PlanDoc, String> {
    let Some(block) = json_block(text) else {
        return Err("there is no fenced ```json block in your last message".into());
    };
    let value: Value = serde_json::from_str(&block).map_err(|e| format!("the json block is not valid JSON: {e}"))?;
    let raw: RawPlan = serde_json::from_value(value).map_err(|e| format!("the json block is not a plan: {e}"))?;
    let mut problems: Vec<String> = Vec::new();
    if raw.tasks.is_empty() || raw.tasks.len() > MAX_TASKS {
        problems.push(format!("tasks: 1 to {MAX_TASKS} tasks, not {}", raw.tasks.len()));
    }
    let mut ids: HashSet<&str> = HashSet::new();
    for (i, t) in raw.tasks.iter().enumerate() {
        let at = format!("tasks[{i}]");
        if !valid_id(&t.id) {
            problems.push(format!("{at}.id: 1 to 20 characters of letters, digits, `.`, `_` and `-`"));
        } else if !ids.insert(t.id.as_str()) {
            problems.push(format!("{at}.id: `{}` is used twice", t.id));
        }
        if t.title.trim().is_empty() || t.title.chars().count() > 200 {
            problems.push(format!("{at}.title: 1 to 200 characters"));
        }
        if t.description.chars().count() > 8000 {
            problems.push(format!("{at}.description: at most 8000 characters"));
        }
        if t.done_when.is_empty() || t.done_when.len() > 8 || t.done_when.iter().any(|l| l.trim().is_empty() || l.chars().count() > 300) {
            problems.push(format!("{at}.done_when: 1 to 8 lines of 1 to 300 characters"));
        }
        if t.files.len() > 30 {
            problems.push(format!("{at}.files: at most 30"));
        }
        if t.ticket.as_deref().is_some_and(|k| k.chars().count() > 40) {
            problems.push(format!("{at}.ticket: at most 40 characters"));
        }
    }
    for (i, t) in raw.tasks.iter().enumerate() {
        for d in &t.depends_on {
            if d == &t.id {
                problems.push(format!("tasks[{i}].depends_on: `{d}` depends on itself"));
            } else if !ids.contains(d.as_str()) {
                problems.push(format!("tasks[{i}].depends_on: no task `{d}`"));
            }
        }
    }
    if problems.is_empty() {
        if let Some(c) = cycle(&raw.tasks) {
            problems.push(format!("depends_on: the tasks {c} depend on each other in a circle"));
        }
    }
    if !problems.is_empty() {
        problems.truncate(8);
        return Err(problems.join("\n"));
    }
    let tasks = raw
        .tasks
        .into_iter()
        .map(|t| {
            let mut task = Task::new(t.id.trim(), t.title.trim());
            task.description = t.description.trim().to_string();
            task.done_when = t.done_when.iter().map(|l| l.trim().to_string()).collect();
            task.depends_on = t.depends_on;
            task.files = t.files;
            task.ticket = t.ticket.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
            task
        })
        .collect();
    Ok(PlanDoc { summary: raw.summary.trim().to_string(), tasks })
}

/// The ids of a cycle, when there is one (Kahn's algorithm: what cannot be taken off is in or behind a cycle).
fn cycle(tasks: &[RawTask]) -> Option<String> {
    let mut left: HashMap<&str, usize> = tasks.iter().map(|t| (t.id.as_str(), t.depends_on.len())).collect();
    let mut queue: Vec<&str> = left.iter().filter(|(_, n)| **n == 0).map(|(id, _)| *id).collect();
    while let Some(done) = queue.pop() {
        left.remove(done);
        for t in tasks.iter().filter(|t| t.depends_on.iter().any(|d| d == done)) {
            if let Some(n) = left.get_mut(t.id.as_str()) {
                *n -= 1;
                if *n == 0 {
                    queue.push(t.id.as_str());
                }
            }
        }
    }
    if left.is_empty() {
        return None;
    }
    let mut ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).filter(|id| left.contains_key(id)).collect();
    ids.dedup();
    Some(ids.join(", "))
}

/// A one-task plan for a run without a planner.
pub fn single(goal: &str) -> PlanDoc {
    let mut task = Task::new("T1", &super::model::title_of(goal));
    task.description = goal.trim().to_string();
    task.done_when = vec!["the goal is met".into()];
    PlanDoc { summary: String::new(), tasks: vec![task] }
}

/// `plan.md`: the plan for a reader.
pub fn markdown(plan: &PlanDoc) -> String {
    let mut out = format!("# Plan\n\n{}\n\n", plan.summary);
    for t in &plan.tasks {
        out.push_str(&format!("## {}: {}\n\n{}\n\n", t.id, t.title, t.description));
        for l in &t.done_when {
            out.push_str(&format!("- done when: {l}\n"));
        }
        if !t.depends_on.is_empty() {
            out.push_str(&format!("- depends on: {}\n", t.depends_on.join(", ")));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(tasks: &str) -> String {
        format!("Here it is.\n```json\n{{\"summary\": \"s\", \"tasks\": [{tasks}]}}\n```\n")
    }

    fn task(id: &str, deps: &str) -> String {
        format!("{{\"id\": \"{id}\", \"title\": \"T {id}\", \"description\": \"d\", \"done_when\": [\"x\"], \"depends_on\": [{deps}], \"files\": [\"a.rs\"]}}")
    }

    #[test]
    fn a_valid_plan_is_read() {
        let p = parse(&plan(&format!("{}, {}", task("M.1", ""), task("M.2", "\"M.1\"")))).unwrap();
        assert_eq!((p.summary.as_str(), p.tasks.len(), p.tasks[1].depends_on.clone()), ("s", 2, vec!["M.1".to_string()]));
        assert_eq!(p.tasks[0].done_when, ["x"]);
    }

    #[test]
    fn the_camel_case_names_and_the_last_block_are_accepted() {
        let text = format!("```json\n{{\"tasks\": []}}\n```\nbetter:\n```json\n{{\"summary\":\"\",\"tasks\":[{{\"id\":\"a\",\"title\":\"t\",\"doneWhen\":[\"y\"],\"dependsOn\":[]}}]}}\n```");
        assert_eq!(parse(&text).unwrap().tasks[0].done_when, ["y"]);
    }

    #[test]
    fn each_kind_of_malformed_plan_says_what_is_wrong() {
        assert!(parse("no block here").unwrap_err().contains("no fenced ```json block"));
        assert!(parse("```json\n{oops\n```").unwrap_err().contains("not valid JSON"));
        assert!(parse("```json\n[1]\n```").unwrap_err().contains("not a plan"));
        assert!(parse(&plan("")).unwrap_err().contains("1 to 30 tasks, not 0"));
        assert!(parse(&plan(&format!("{}, {}", task("a", ""), task("a", "")))).unwrap_err().contains("`a` is used twice"));
        assert!(parse(&plan(&task("a", "\"zz\""))).unwrap_err().contains("no task `zz`"));
        assert!(parse(&plan(&task("a", "\"a\""))).unwrap_err().contains("depends on itself"));
        assert!(parse(&plan(&format!("{}, {}", task("a", "\"b\""), task("b", "\"a\"")))).unwrap_err().contains("depend on each other in a circle"));
        assert!(parse(&plan("{\"id\": \"a\", \"title\": \"t\", \"done_when\": []}")).unwrap_err().contains("done_when: 1 to 8 lines"));
        assert!(parse(&plan("{\"id\": \"has space\", \"title\": \"t\", \"done_when\": [\"x\"]}")).unwrap_err().contains("tasks[0].id"));
    }

    #[test]
    fn thirty_tasks_pass_and_thirty_one_do_not() {
        let many = |n: usize| (0..n).map(|i| task(&format!("t{i}"), "")).collect::<Vec<_>>().join(",");
        assert_eq!(parse(&plan(&many(30))).unwrap().tasks.len(), 30);
        assert!(parse(&plan(&many(31))).unwrap_err().contains("not 31"));
    }

    #[test]
    fn a_plan_without_a_planner_is_one_task() {
        let p = single("Fix the overlay\nand the focus");
        assert_eq!((p.tasks[0].id.as_str(), p.tasks[0].title.as_str(), p.tasks[0].description.as_str()), ("T1", "Fix the overlay", "Fix the overlay\nand the focus"));
    }
}
