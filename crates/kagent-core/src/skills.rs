//! SKILL.md loader (Claude/ZCode-compatible format): frontmatter name +
//! description index into the system prompt; `load_skill` returns the body.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::{Tool, ToolContext, ToolContract, ToolOutput};

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Parse one SKILL.md; frontmatter is `---` fenced YAML with `name`/`description`.
pub fn parse_skill(raw: &str) -> Option<(String, String, String)> {
    let rest = raw.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    // Flat `key: value` YAML is enough for skill frontmatter.
    let mut name: Option<String> = None;
    let mut description = String::new();
    for line in fm.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once(':')?;
        let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
        if k.trim() == "name" {
            name = Some(v);
        } else if k.trim() == "description" {
            description = v;
        }
    }
    let name = name.filter(|n| !n.is_empty())?;
    Some((name, description, body))
}

/// Load all skills from a directory of `<name>/SKILL.md`.
pub fn load_skills(dir: &Path) -> Vec<Skill> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for entry in rd.flatten() {
        let path = entry.path().join("SKILL.md");
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        match parse_skill(&raw) {
            Some((name, description, body)) => out.push(Skill { name, description, body }),
            None => tracing::warn!("skill {} skipped: bad or missing frontmatter", path.display()),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The prompt index: "- name: description" lines.
pub fn skills_index(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nDiagnostic playbooks (call load_skill with the name before using one):\n");
    for s in skills {
        out.push_str(&format!("- {}: {}\n", s.name, s.description));
    }
    out
}

pub struct LoadSkill {
    contract: ToolContract,
    skills_dir: PathBuf,
}

impl LoadSkill {
    pub fn new(skills_dir: impl Into<PathBuf>) -> Self {
        Self {
            contract: ToolContract::read_only(
                "load_skill",
                "Load a diagnostic playbook by name. Get names from the system prompt's playbook index.",
                json!({"type":"object","required":["name"],"properties":{
                    "name":{"type":"string"}
                }}),
            ),
            skills_dir: skills_dir.into(),
        }
    }
}

#[derive(Deserialize, Default)]
struct NameArgs {
    name: Option<String>,
}

#[async_trait]
impl Tool for LoadSkill {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }
    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: NameArgs = parse_value(input)?;
        let Some(name) = args.name.filter(|n| !n.is_empty()) else {
            return Ok(ToolOutput::error("name is required"));
        };
        let skills = load_skills(&self.skills_dir);
        match skills.iter().find(|s| s.name == name) {
            Some(s) => Ok(ToolOutput::ok(s.body.clone())),
            None => Ok(ToolOutput::error(format!(
                "unknown skill '{name}'. Available: {}",
                skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

fn parse_value<T: Default + serde::de::DeserializeOwned>(value: Value) -> anyhow::Result<T> {
    match value {
        Value::Null => Ok(T::default()),
        v => Ok(serde_json::from_value(v)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let raw = "---\nname: slow-machine\ndescription: why is it slow\n---\nStep 1: ...\n";
        let (name, desc, body) = parse_skill(raw).expect("parses");
        assert_eq!(name, "slow-machine");
        assert_eq!(desc, "why is it slow");
        assert!(body.starts_with("Step 1"));
    }

    #[test]
    fn missing_frontmatter_is_skipped() {
        assert!(parse_skill("no frontmatter here").is_none());
        assert!(parse_skill("---\ndescription: no name\n---\nbody").is_none());
    }

    #[test]
    fn loads_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        std::fs::write(dir.path().join("alpha/SKILL.md"), "---\nname: alpha\ndescription: a\n---\nA body").unwrap();
        std::fs::create_dir(dir.path().join("beta")).unwrap();
        std::fs::write(dir.path().join("beta/SKILL.md"), "broken").unwrap();
        let skills = load_skills(dir.path());
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "alpha");
        assert!(skills_index(&skills).contains("alpha: a"));
    }
}
