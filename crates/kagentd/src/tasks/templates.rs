//! The three templates of the Tasks tab (automations.md section 6). Each adds a task
//! that is paused; the owner switches on the ones to run.

use chrono::{DateTime, Local};
use serde_json::{json, Value};

use super::schedule;

pub struct Template {
    pub id: &'static str,
    pub title: &'static str,
    pub what: &'static str,
    /// In words, as the owner could type it.
    pub schedule: &'static str,
}

pub const TEMPLATES: [Template; 3] = [
    Template {
        id: "morning-health-check",
        title: "Morning health check",
        what: "Run doctor. If the score is 90 or more reply NOTHING, otherwise summarize what lowers it.",
        schedule: "weekdays 08:00",
    },
    Template {
        id: "disk-report",
        title: "Disk report",
        what: "Report disk usage per filesystem and the three largest folders in the home directory.",
        schedule: "every monday 09:00",
    },
    Template { id: "heartbeat", title: "Heartbeat", what: "Check open alerts and failed units. Reply NOTHING when there is nothing new.", schedule: "every 30m" },
];

pub fn get(id: &str) -> Option<&'static Template> {
    TEMPLATES.iter().find(|t| t.id == id)
}

/// The templates as the API lists them.
pub fn views(now: DateTime<Local>) -> Vec<Value> {
    TEMPLATES
        .iter()
        .map(|t| {
            let text = schedule::parse(t.schedule, now).ok().flatten().map(|s| s.describe()).unwrap_or_default();
            json!({ "id": t.id, "title": t.title, "what": t.what, "kind": "agent", "schedule": t.schedule, "scheduleText": text })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use crate::sections::TasksSection;
    use crate::tasks::store::TaskStore;

    #[test]
    fn every_template_parses_and_adds_a_paused_task() {
        let now = Local.with_ymd_and_hms(2026, 11, 23, 10, 0, 0).unwrap();
        let listed = views(now);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0]["scheduleText"], "weekdays at 08:00");
        assert_eq!(listed[1]["scheduleText"], "mon at 09:00");
        assert_eq!(listed[2]["scheduleText"], "every 30 minutes");
        let dir = tempfile::tempdir().unwrap();
        let (events, _) = tokio::sync::broadcast::channel(16);
        let store = TaskStore::load(dir.path(), events, TasksSection::default());
        for template in TEMPLATES {
            let task = store.add_template(template.id, now).unwrap();
            assert!(!task.enabled && !task.done && task.kind == "agent", "{} is added paused", template.id);
            assert!(task.next_run.is_some(), "{} has its next run", template.id);
            assert_eq!(task.what, template.what);
        }
        assert!(store.claim(now, 2).is_empty(), "a paused task is not due");
        assert_eq!(store.list().len(), 3);
        assert!(matches!(store.add_template("nope", now), Err(super::super::store::TaskError::Invalid(_))));
    }
}
