//! Workflows (workflows.md): the definitions, the states of a run, the plan and verdict parsers, and (further down) the engine
//! that runs a run with KAgent's own subagents.

mod control;
pub mod definition;
mod drive;
pub mod engine;
mod git;
pub mod mailbox;
pub mod model;
mod prompts;
pub mod report;
mod store;
mod task;
mod worker;
pub mod plan;
pub mod roles;
pub mod shipped;
pub mod verdict;
pub mod yaml;

pub use definition::{Definition, Role};
pub use model::{Run, RunState, Task, TaskState};
pub use engine::{Decision, DefinitionLookup, Differ, Engine, EngineConfig, EngineError, ProfileLookup, StartRequest, TaskEdit};
pub use git::relative as relative_files;
pub use mailbox::Answerer;
