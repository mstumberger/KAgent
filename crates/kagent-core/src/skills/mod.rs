//! Skills library (skills.md sections 1-4): `SKILL.md` files in the format Claude
//! Code, Codex and ZCode use, merged from several roots into one library. Its index
//! goes into the system prompt; `load_skill` returns a skill's body.
//!
//! - `frontmatter`: `parse` and `unfold_scalar`.
//! - `file`: writing a `SKILL.md`, new or changed.
//! - `library`: the roots, `SkillEntry`, `SkillLibrary` (scan, refresh, index).
//! - `tool`: the `load_skill` tool and the usage counter.
//! - `author`: the `skill_save` and `skill_delete` tools, and the checks of the API.

pub mod author;
pub mod file;
pub mod frontmatter;
pub mod library;
pub mod tool;

pub use author::{plan_delete, plan_save, remove_skill, write_skill, DeletePlan, SavePlan};
pub use library::{SkillEntry, SkillLibrary, SkillRoot, SkillSource};
pub use tool::{LoadSkill, SkillCounter, SkillUsage};
