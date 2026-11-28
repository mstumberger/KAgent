//! Workflows (workflows.md): the definitions, the states of a run, the plan and verdict parsers, and (further down) the engine
//! that runs a run with KAgent's own subagents.

pub mod definition;
pub mod model;
pub mod plan;
pub mod roles;
pub mod shipped;
pub mod verdict;
pub mod yaml;

pub use definition::{Definition, Role};
pub use model::{Run, RunState, Task, TaskState};
