//! The `skill_save` and `skill_delete` tools (tools.md section 17; skills.md sections
//! 5 and 10). What is checked before anything is written, and the writing itself, is
//! `plan.rs`, which the skills API uses too.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::{CallAssessment, Risk, SideEffect, Tool, ToolContext, ToolContract, ToolOutput};
use crate::tools::parse;

use super::file;
use super::library::SkillLibrary;

mod plan;
pub use plan::{plan_delete, plan_save, remove_skill, write_skill, DeletePlan, OnChange, SavePlan, MAX_SAVE_BODY_BYTES, RESERVED};

/// Body lines on an approval card.
const PREVIEW_LINES: usize = 30;
/// Files of a skill on an approval card.
const PREVIEW_FILES: usize = 20;

#[derive(Deserialize, Default)]
#[serde(default)]
struct SaveArgs {
    name: String,
    description: String,
    when_to_use: String,
    /// One text, or its lines (tools.md section 17): small models get an array of
    /// lines right where a long Markdown text inside JSON goes wrong.
    #[serde(deserialize_with = "text_or_lines")]
    body: String,
}

fn text_or_lines<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    use serde::de::Error;
    match Value::deserialize(d)? {
        Value::String(text) => Ok(text),
        Value::Null => Ok(String::new()),
        Value::Array(items) => items
            .into_iter()
            .map(|item| item.as_str().map(str::to_string).ok_or_else(|| D::Error::custom("body: every line must be text")))
            .collect::<Result<Vec<_>, _>>()
            .map(|lines| lines.join("\n")),
        other => Err(D::Error::custom(format!("body: expected text or an array of lines, got {other}"))),
    }
}

/// The card (tools.md section 17): what happens, then the name, the description and the
/// first lines of the body.
fn save_preview(plan: &SavePlan, args: &SaveArgs) -> String {
    let mut out = format!("{} skill '{}'\ndescription: {}\n", if plan.overwrite { "Replace the" } else { "New" }, plan.name, one_line(&args.description));
    if !args.when_to_use.trim().is_empty() {
        out.push_str(&format!("when_to_use: {}\n", one_line(&args.when_to_use)));
    }
    out.push('\n');
    let mut lines = args.body.trim_end().lines();
    for line in lines.by_ref().take(PREVIEW_LINES) {
        out.push_str(line);
        out.push('\n');
    }
    let more = lines.count();
    if more > 0 {
        out.push_str(&format!("... {more} more lines\n"));
    }
    out
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `skill_save`: a skill for the merged library, written into the owner's root.
pub struct SkillSave {
    contract: ToolContract,
    library: Arc<RwLock<SkillLibrary>>,
    user_root: PathBuf,
    on_change: OnChange,
}

impl SkillSave {
    pub fn new(library: Arc<RwLock<SkillLibrary>>, user_root: impl Into<PathBuf>, on_change: OnChange) -> Self {
        Self {
            contract: ToolContract::read_only(
                "skill_save",
                "Save a skill: instructions KAgent loads later with load_skill when the description matches a \
                 request. Use it when the owner asks to remember how something was done, or agrees to your offer \
                 to save a fix that is likely to recur. name: lowercase words joined by hyphens. description: \
                 the requests it is for, as trigger words. Never write secrets or machine-specific paths into a \
                 skill. Replaces a skill of the owner with the same name; the skills KAgent ships with are never \
                 touched.",
                json!({"type":"object","required":["name","description","body"],"properties":{
                    "name":{"type":"string","description":"^[a-z0-9][a-z0-9-]{0,63}$, the folder name"},
                    "description":{"type":"string","description":"what it is for and when to use it, at most 1024 characters"},
                    "when_to_use":{"type":"string","description":"optional: the cases the description cannot hold"},
                    "body":{"description":"the instructions as Markdown, one line per item: the steps, naming the tools, and what a good answer looks like; at most 64 KB","anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}
                }}),
            )
            .mutating(SideEffect::Workspace, Risk::Low),
            library,
            user_root: user_root.into(),
            on_change,
        }
    }
}

#[async_trait]
impl Tool for SkillSave {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let name = input.get("name").and_then(Value::as_str).unwrap_or("?");
        let summary = format!("save skill {name}");
        CallAssessment::from_contract(&self.contract, summary.clone(), summary)
    }

    async fn preview(&self, input: &Value, _ctx: &ToolContext) -> Option<String> {
        let args: SaveArgs = serde_json::from_value(input.clone()).ok()?;
        let library = self.library.read().ok()?;
        Some(match plan_save(&library, &self.user_root, &args.name, &args.description, &args.body) {
            Ok(plan) => save_preview(&plan, &args),
            Err(reason) => format!("Not possible: {reason}"),
        })
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: SaveArgs = parse(input)?;
        let plan = {
            let library = self.library.read().map_err(|_| anyhow::anyhow!("skills library lock poisoned"))?;
            match plan_save(&library, &self.user_root, &args.name, &args.description, &args.body) {
                Ok(plan) => plan,
                Err(reason) => return Ok(ToolOutput::error(reason)),
            }
        };
        let text = file::render(&plan.name, &args.description, &args.when_to_use, &args.body);
        let path = write_skill(&plan, &text).await.map_err(|e| anyhow::anyhow!("cannot write {}/SKILL.md: {e}", plan.dir.display()))?;
        (self.on_change)();
        // How it is used, because the model otherwise invents a command for it.
        let mut out = format!(
            "Saved skill '{}' to {} ({}). The agent loads it later with load_skill, or the owner starts a message with /{}.",
            plan.name,
            path.display(),
            if plan.overwrite { "replaced the skill that was there" } else { "new skill" },
            plan.name
        );
        if let Some(source) = plan.shadows {
            out.push_str(&format!(" It hides the {} skill of the same name.", source.label()));
        }
        if plan.disabled {
            out.push_str(" It is switched off in settings (skills.disabled), so it is not in the prompt until the owner enables it in Settings -> Skills.");
        }
        if args.description.trim().is_empty() {
            out.push_str(" It has no description: the agent can only pick it by its name.");
        }
        Ok(ToolOutput::ok(out))
    }
}

#[derive(Deserialize, Default)]
struct NameArgs {
    name: String,
}

/// `skill_delete`: removes a skill of the owner.
pub struct SkillDelete {
    contract: ToolContract,
    library: Arc<RwLock<SkillLibrary>>,
    user_root: PathBuf,
    on_change: OnChange,
}

impl SkillDelete {
    pub fn new(library: Arc<RwLock<SkillLibrary>>, user_root: impl Into<PathBuf>, on_change: OnChange) -> Self {
        Self {
            contract: ToolContract::read_only(
                "skill_delete",
                "Delete a skill the owner or the agent wrote, by name, with its folder. The skills KAgent ships \
                 with cannot be deleted (switch them off instead). The owner approves each delete.",
                json!({"type":"object","required":["name"],"properties":{"name":{"type":"string"}}}),
            )
            .mutating(SideEffect::Workspace, Risk::Low),
            library,
            user_root: user_root.into(),
            on_change,
        }
    }
}

#[async_trait]
impl Tool for SkillDelete {
    fn contract(&self) -> &ToolContract {
        &self.contract
    }

    fn assess(&self, input: &Value) -> CallAssessment {
        let name = input.get("name").and_then(Value::as_str).unwrap_or("?");
        let summary = format!("delete skill {name}");
        CallAssessment::from_contract(&self.contract, summary.clone(), summary)
    }

    async fn preview(&self, input: &Value, _ctx: &ToolContext) -> Option<String> {
        let args: NameArgs = serde_json::from_value(input.clone()).ok()?;
        let library = self.library.read().ok()?;
        Some(match plan_delete(&library, &self.user_root, &args.name, false) {
            Ok(plan) if plan.link => format!("Remove the link {}", plan.dir.display()),
            Ok(plan) => {
                let mut out = format!("Delete skill '{}' and its folder {}:", plan.name, plan.dir.display());
                for (file, len) in plan.files.iter().take(PREVIEW_FILES) {
                    out.push_str(&format!("\n  {file} ({len} bytes)"));
                }
                if plan.files.len() > PREVIEW_FILES {
                    out.push_str(&format!("\n  ... {} more", plan.files.len() - PREVIEW_FILES));
                }
                out
            }
            Err(reason) => format!("Not possible: {reason}"),
        })
    }

    async fn call(&self, input: Value, _ctx: ToolContext) -> anyhow::Result<ToolOutput> {
        let args: NameArgs = parse(input)?;
        let plan = {
            let library = self.library.read().map_err(|_| anyhow::anyhow!("skills library lock poisoned"))?;
            match plan_delete(&library, &self.user_root, &args.name, false) {
                Ok(plan) => plan,
                Err(reason) => return Ok(ToolOutput::error(reason)),
            }
        };
        remove_skill(&plan).await.map_err(|e| anyhow::anyhow!("cannot remove {}: {e}", plan.dir.display()))?;
        (self.on_change)();
        let mut out = format!("Deleted skill '{}' ({}).", plan.name, plan.dir.display());
        if let Some(source) = plan.unshadows {
            out.push_str(&format!(" The {} skill of that name is in use again.", source.label()));
        }
        Ok(ToolOutput::ok(out))
    }
}

#[cfg(test)]
mod tests;
