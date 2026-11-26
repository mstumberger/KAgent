//! Scheduled tasks (automations.md): reminders and unattended agent prompts with a
//! schedule in words or cron, stored in `<data>/tasks.json` and run by a loop every
//! 15 s. `schedule` is the pure part (parsing, `next_run`), `store` the record and its
//! file, `runner` the loop and one run, `tools` the `tasks` tool, `http` the routes and
//! `templates` the three templates of the Tasks tab.

pub mod cron;
pub mod http;
pub mod runner;
pub mod schedule;
pub mod store;
pub mod templates;
pub mod tools;

pub use runner::start;
pub use store::TaskStore;
pub use tools::TasksTool;
