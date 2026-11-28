//! A workflow definition (workflows.md section 2): parsed from the YAML subset with `deny_unknown_fields`, then checked. Every
//! message names the field ("steps[1].parallel: ...").

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_STEPS: usize = 8;
pub const MAX_PARALLEL: u32 = 8;
pub const DEFAULT_MAX_ROUNDS: u32 = 3;
pub const MAX_ROUNDS: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Planner,
    Implementer,
    Reviewer,
    Orchestrator,
}

impl Role {
    pub fn word(self) -> &'static str {
        match self {
            Self::Planner => "planner",
            Self::Implementer => "implementer",
            Self::Reviewer => "reviewer",
            Self::Orchestrator => "orchestrator",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Subagent,
    Coder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct WorkerRef {
    pub kind: Kind,
    pub profile: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    Owner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForEach {
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Do {
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Then {
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct OnChanges {
    pub back_to: String,
    pub max_rounds: u32,
    pub then: Then,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub role: Role,
    #[serde(default)]
    pub worker: Option<WorkerRef>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub gate: Option<Gate>,
    #[serde(default)]
    pub for_each: Option<ForEach>,
    #[serde(default)]
    pub parallel: Option<u32>,
    #[serde(default)]
    pub on_changes: Option<OnChanges>,
    #[serde(default)]
    pub r#do: Option<Do>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub isolation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Definition {
    pub name: String,
    pub description: String,
    pub steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Raw {
    name: String,
    description: String,
    steps: Vec<Value>,
}

/// `^[a-z0-9][a-z0-9-]{0,63}$`
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64 && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A model ref: `<providerId>::<modelId>`.
pub fn valid_model_ref(model: &str) -> bool {
    model.split_once("::").is_some_and(|(p, m)| !p.trim().is_empty() && !m.trim().is_empty())
}

impl Definition {
    /// Read a file: the YAML, the types, then `check`. `stem` is the file name without `.yaml`.
    pub fn parse(text: &str, stem: &str, known_profile: &dyn Fn(&str) -> bool) -> Result<Self, String> {
        let value = super::yaml::parse(text)?;
        let raw: Raw = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let mut steps = Vec::new();
        for (i, step) in raw.steps.into_iter().enumerate() {
            steps.push(serde_json::from_value::<Step>(step).map_err(|e| format!("steps[{i}]: {e}"))?);
        }
        let def = Self { name: raw.name, description: raw.description, steps };
        if def.name != stem {
            return Err(format!("name: `{}` must equal the file name `{stem}`", def.name));
        }
        def.check(known_profile)?;
        Ok(def)
    }

    /// The rules of workflows.md section 2.
    pub fn check(&self, known_profile: &dyn Fn(&str) -> bool) -> Result<(), String> {
        if !valid_name(&self.name) {
            return Err("name: lower case letters, digits and hyphens, at most 64, starting with a letter or digit".into());
        }
        if self.description.trim().is_empty() || self.description.chars().count() > 300 {
            return Err("description: 1 to 300 characters".into());
        }
        if self.steps.is_empty() || self.steps.len() > MAX_STEPS {
            return Err(format!("steps: 1 to {MAX_STEPS} steps, not {}", self.steps.len()));
        }
        let mut seen: Vec<&str> = Vec::new();
        for (i, s) in self.steps.iter().enumerate() {
            let at = format!("steps[{i}]");
            if !valid_name(&s.id) {
                return Err(format!("{at}.id: lower case letters, digits and hyphens"));
            }
            if seen.contains(&s.id.as_str()) {
                return Err(format!("{at}.id: `{}` is used by an earlier step", s.id));
            }
            seen.push(&s.id);
            if self.steps[..i].iter().any(|o| o.role == s.role) {
                return Err(format!("{at}.role: a second {} step; a workflow has one step per role", s.role.word()));
            }
            if self.steps[..i].iter().any(|o| rank(o.role) > rank(s.role)) {
                return Err(format!("{at}.role: {} comes after a later role; the order is planner, implementer, reviewer, orchestrator", s.role.word()));
            }
            self.check_step(s, &at, known_profile)?;
        }
        let find = |role| self.steps.iter().find(|s| s.role == role);
        let (build, review) = (find(Role::Implementer), find(Role::Reviewer));
        if let (Some(b), Some(r)) = (build, review) {
            let (bw, rw) = (b.worker.as_ref().unwrap(), r.worker.as_ref().unwrap());
            if bw.profile == rw.profile && b.model == r.model {
                return Err(format!("steps[{}].worker: the reviewer is the same as the implementer (profile {}, same model): a review by the same model is not a review", self.index(&r.id), rw.profile));
            }
            if r.on_changes.is_none() {
                return Err(format!("steps[{}].on_changes: a reviewer after an implementer says where findings go back to", self.index(&r.id)));
            }
        }
        Ok(())
    }

    fn index(&self, id: &str) -> usize {
        self.steps.iter().position(|s| s.id == id).unwrap_or(0)
    }

    fn check_step(&self, s: &Step, at: &str, known_profile: &dyn Fn(&str) -> bool) -> Result<(), String> {
        let worker_role = s.role != Role::Orchestrator;
        match (&s.worker, worker_role) {
            (None, true) => return Err(format!("{at}.worker: a {} step needs {{kind: subagent, profile: <name>}}", s.role.word())),
            (Some(_), false) => return Err(format!("{at}.worker: the orchestrator step has no worker")),
            (Some(w), true) => {
                if w.kind == Kind::Coder {
                    return Err(format!("{at}.worker.kind: coding agents as workers are not built yet (kind: coder); use kind: subagent"));
                }
                if !known_profile(&w.profile) {
                    return Err(format!("{at}.worker.profile: no profile '{}' (Settings -> Subagents, or planner, implementer, reviewer, general, explore)", w.profile));
                }
            }
            _ => {}
        }
        if let Some(m) = &s.model {
            if !valid_model_ref(m) {
                return Err(format!("{at}.model: '{m}' is not a model ref (<providerId>::<modelId>)"));
            }
        }
        if s.gate.is_some() && s.role != Role::Planner {
            return Err(format!("{at}.gate: only the planner step has a gate"));
        }
        if s.for_each.is_some() && !matches!(s.role, Role::Implementer | Role::Reviewer) {
            return Err(format!("{at}.for_each: only the implementer and reviewer steps run for each task"));
        }
        if s.role == Role::Implementer && s.for_each.is_none() {
            return Err(format!("{at}.for_each: the implementer step needs `for_each: task`"));
        }
        if let Some(p) = s.parallel {
            if s.role != Role::Implementer {
                return Err(format!("{at}.parallel: only the implementer step has `parallel`"));
            }
            if !(1..=MAX_PARALLEL).contains(&p) {
                return Err(format!("{at}.parallel: 1 to {MAX_PARALLEL}, not {p}"));
            }
        }
        if let Some(o) = &s.on_changes {
            if s.role != Role::Reviewer {
                return Err(format!("{at}.on_changes: only the reviewer step has `on_changes`"));
            }
            if !(1..=MAX_ROUNDS).contains(&o.max_rounds) {
                return Err(format!("{at}.on_changes.max_rounds: 1 to {MAX_ROUNDS}, not {}", o.max_rounds));
            }
            let target = self.steps.iter().position(|t| t.id == o.back_to);
            let here = self.steps.iter().position(|t| t.id == s.id).unwrap_or(0);
            match target {
                Some(t) if t < here && self.steps[t].role == Role::Implementer => {}
                Some(_) => return Err(format!("{at}.on_changes.back_to: '{}' must be an earlier implementer step", o.back_to)),
                None => return Err(format!("{at}.on_changes.back_to: no step '{}'", o.back_to)),
            }
        }
        if let Some(d) = &s.r#do {
            if s.role != Role::Orchestrator || *d != Do::Summary {
                return Err(format!("{at}.do: only the orchestrator step has `do: summary`"));
            }
        } else if s.role == Role::Orchestrator {
            return Err(format!("{at}.do: the orchestrator step needs `do: summary`"));
        }
        if let Some(o) = &s.output {
            if o != "plan" || s.role != Role::Planner {
                return Err(format!("{at}.output: only `plan` on the planner step"));
            }
        }
        match s.isolation.as_deref() {
            None | Some("none") => {}
            Some("worktree") => return Err(format!("{at}.isolation: worktrees are not built yet; use `none`")),
            Some(other) => return Err(format!("{at}.isolation: `none` (not '{other}')")),
        }
        Ok(())
    }

    pub fn step(&self, role: Role) -> Option<&Step> {
        self.steps.iter().find(|s| s.role == role)
    }

    /// Rounds an implementation may take before the owner decides.
    pub fn max_rounds(&self) -> u32 {
        self.step(Role::Reviewer).and_then(|s| s.on_changes.as_ref()).map_or(DEFAULT_MAX_ROUNDS, |o| o.max_rounds)
    }

    /// How many implementers may run at once.
    pub fn parallel(&self) -> u32 {
        self.step(Role::Implementer).and_then(|s| s.parallel).unwrap_or(1)
    }

    /// Does the owner approve the plan?
    pub fn gated(&self) -> bool {
        self.step(Role::Planner).is_some_and(|s| s.gate == Some(Gate::Owner))
    }
}

fn rank(role: Role) -> u8 {
    match role {
        Role::Planner => 0,
        Role::Implementer => 1,
        Role::Reviewer => 2,
        Role::Orchestrator => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(name: &str) -> bool {
        matches!(name, "planner" | "implementer" | "reviewer" | "general")
    }

    const FULL: &str = "name: full\ndescription: d\nsteps:\n  - id: plan\n    role: planner\n    worker: {kind: subagent, profile: planner}\n    gate: owner\n  - id: build\n    role: implementer\n    for_each: task\n    worker: {kind: subagent, profile: implementer}\n  - id: review\n    role: reviewer\n    for_each: task\n    worker: {kind: subagent, profile: reviewer}\n    on_changes: {back_to: build, max_rounds: 3, then: owner}\n  - id: wrap\n    role: orchestrator\n    do: summary\n";

    fn err(text: &str) -> String {
        Definition::parse(text, "full", &known).unwrap_err()
    }

    #[test]
    fn a_full_definition_is_read() {
        let d = Definition::parse(FULL, "full", &known).unwrap();
        assert_eq!((d.steps.len(), d.max_rounds(), d.parallel(), d.gated()), (4, 3, 1, true));
    }

    #[test]
    fn every_rule_names_the_field() {
        assert!(err(&FULL.replace("name: full", "name: other")).starts_with("name: `other` must equal the file name"));
        assert!(err(&FULL.replace("role: planner", "role: boss")).contains("steps[0]: unknown variant `boss`"));
        assert!(err(&FULL.replace("gate: owner", "gate: owner\n    colour: red")).contains("steps[0]: unknown field `colour`"));
        assert!(err(&FULL.replace("max_rounds: 3", "max_rounds: 9")).starts_with("steps[2].on_changes.max_rounds: 1 to 5, not 9"));
        assert!(err(&FULL.replace("back_to: build", "back_to: nope")).starts_with("steps[2].on_changes.back_to: no step 'nope'"));
        assert!(err(&FULL.replace("back_to: build", "back_to: plan")).starts_with("steps[2].on_changes.back_to: 'plan' must be an earlier implementer step"));
        assert!(err(&FULL.replace("profile: reviewer", "profile: ghost")).starts_with("steps[2].worker.profile: no profile 'ghost'"));
        assert!(err(&FULL.replace("for_each: task\n    worker: {kind: subagent, profile: implementer}", "worker: {kind: subagent, profile: implementer}")).starts_with("steps[1].for_each"));
        assert!(err(&FULL.replace("    gate: owner\n", "    gate: owner\n    parallel: 2\n")).starts_with("steps[0].parallel: only the implementer"));
        assert!(err(&FULL.replace("worker: {kind: subagent, profile: implementer}", "worker: {kind: subagent, profile: implementer}\n    parallel: 12")).starts_with("steps[1].parallel: 1 to 8, not 12"));
        assert!(err(&FULL.replace("worker: {kind: subagent, profile: planner}", "worker: {kind: subagent, profile: planner}\n    model: opus")).starts_with("steps[0].model: 'opus' is not a model ref"));
        assert!(err(&FULL.replace("{kind: subagent, profile: reviewer}", "{kind: subagent, profile: implementer}")).contains("a review by the same model is not a review"));
    }

    #[test]
    fn the_same_profile_with_another_model_is_a_review() {
        let t = FULL.replace("{kind: subagent, profile: reviewer}", "{kind: subagent, profile: implementer}\n    model: \"a::b\"");
        assert!(Definition::parse(&t, "full", &known).is_ok());
    }

    #[test]
    fn a_coding_agent_and_a_worktree_are_refused_clearly() {
        assert!(err(&FULL.replace("kind: subagent, profile: implementer", "kind: coder, profile: implementer")).contains("coding agents as workers are not built yet (kind: coder)"));
        assert!(err(&FULL.replace("    for_each: task\n    worker: {kind: subagent, profile: implementer}", "    for_each: task\n    isolation: worktree\n    worker: {kind: subagent, profile: implementer}")).contains("worktrees are not built yet"));
    }

    #[test]
    fn roles_are_unique_and_ordered() {
        let two = FULL.replace("  - id: wrap\n    role: orchestrator\n    do: summary\n", "  - id: build2\n    role: implementer\n    for_each: task\n    worker: {kind: subagent, profile: implementer}\n");
        assert!(err(&two).contains("a second implementer step"));
        let wrong = "name: full\ndescription: d\nsteps:\n  - id: a\n    role: reviewer\n    worker: {kind: subagent, profile: reviewer}\n  - id: b\n    role: planner\n    worker: {kind: subagent, profile: planner}\n";
        assert!(err(wrong).contains("the order is planner"));
    }

    #[test]
    fn a_reviewer_after_an_implementer_needs_on_changes() {
        let t = FULL.replace("    on_changes: {back_to: build, max_rounds: 3, then: owner}\n", "");
        assert!(err(&t).starts_with("steps[2].on_changes"));
    }
}
