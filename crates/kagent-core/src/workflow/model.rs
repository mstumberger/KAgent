//! What a run is (workflows.md section 3): the states, the tasks, the findings, the questions and the messages. All of it is
//! stored in `run.json` (camelCase) and is what the API answers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::definition::Definition;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Planning,
    AwaitingApproval,
    Working,
    Paused,
    Done,
    Stopped,
    Failed,
}

impl RunState {
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Stopped | Self::Failed)
    }

    /// Counts against `workflows.maxRuns`.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Planning | Self::AwaitingApproval | Self::Working)
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::AwaitingApproval => "awaiting approval",
            Self::Working => "working",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Waiting,
    Blocked,
    Implementing,
    InReview,
    /// Findings went back to the implementer (`round` says which round it is now).
    Changes,
    NeedsOwner,
    Done,
    Dropped,
    Failed,
    Stopped,
}

impl TaskState {
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Dropped | Self::Failed | Self::Stopped | Self::Blocked)
    }

    /// A worker of this task may be running.
    pub fn is_working(self) -> bool {
        matches!(self, Self::Implementing | Self::InReview | Self::Changes)
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Implementing => "implementing",
            Self::InReview => "in review",
            Self::Changes => "changes requested",
            Self::NeedsOwner => "needs the owner",
            Self::Done => "done",
            Self::Dropped => "dropped",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Must,
    Should,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: Severity,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: u32,
    pub what: String,
    #[serde(default)]
    pub why: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Changes,
}

/// One round of a task: what the implementer reported and what the reviewer said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Round {
    pub round: u32,
    pub report: String,
    #[serde(default)]
    pub verdict: Option<Verdict>,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressStep {
    pub title: String,
    pub status: StepStatus,
}

/// A worker of a task: its session (a subagent's), for the board to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    pub role: String,
    pub session: String,
    pub round: u32,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub done_when: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    /// A key of an outside tracker, as the planner gave it.
    #[serde(default)]
    pub ticket: Option<String>,
    /// The key of KAgent's own ticket for this task (tickets.md section 9).
    #[serde(default)]
    pub ka_ticket: Option<String>,
    pub state: TaskState,
    /// The round of the implementer now (0 before the first).
    #[serde(default)]
    pub round: u32,
    /// Rounds the owner added with `more_round`.
    #[serde(default)]
    pub extra_rounds: u32,
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub rounds: Vec<Round>,
    #[serde(default)]
    pub workers: Vec<WorkerRecord>,
    #[serde(default)]
    pub progress: Vec<ProgressStep>,
    /// Files the implementer changed (from its calls), for the reviewer and the board.
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
}

impl Task {
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            description: String::new(),
            done_when: Vec::new(),
            depends_on: Vec::new(),
            files: Vec::new(),
            ticket: None,
            ka_ticket: None,
            state: TaskState::Waiting,
            round: 0,
            extra_rounds: 0,
            accepted: false,
            error: String::new(),
            rounds: Vec::new(),
            workers: Vec::new(),
            progress: Vec::new(),
            changed_files: Vec::new(),
            started_at: None,
            ended_at: None,
        }
    }

    /// The `should` findings of every round: they are listed in the run's report.
    pub fn should_findings(&self) -> Vec<&Finding> {
        self.rounds.iter().flat_map(|r| r.findings.iter()).filter(|f| f.severity == Severity::Should).collect()
    }

    /// The report of the latest round.
    pub fn last_report(&self) -> &str {
        self.rounds.last().map_or("", |r| r.report.as_str())
    }

    /// The `must` findings of the latest round.
    pub fn open_findings(&self) -> Vec<&Finding> {
        self.rounds.last().map(|r| r.findings.iter().filter(|f| f.severity == Severity::Must).collect()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Queued,
    Delivered,
    Expired,
}

/// A message of the orchestrator to a worker (workflows.md section 4.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub task: String,
    pub text: String,
    pub state: Delivery,
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub delivered_at: Option<DateTime<Utc>>,
    /// The file of a long text, relative to the run's folder.
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    /// Asked; the Brain may answer.
    Open,
    /// The owner has to answer (a card on the board).
    Escalated,
    Answered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub task: String,
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    pub blocking: bool,
    pub state: QuestionState,
    pub asked_at: DateTime<Utc>,
    #[serde(default)]
    pub answer: Option<String>,
    /// `brain` or `owner`.
    #[serde(default)]
    pub answered_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDoc {
    pub summary: String,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub workflow: String,
    /// The definition as it was when the run started.
    pub definition: Definition,
    pub project: String,
    pub goal: String,
    pub title: String,
    pub state: RunState,
    #[serde(default)]
    pub reason: String,
    /// The conversation that started the run (the Brain that answers questions), if any.
    #[serde(default)]
    pub origin_session: Option<String>,
    /// The session the workers are children of.
    pub run_session: String,
    /// The key of KAgent's ticket for the run (tickets.md section 9).
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub plan_summary: String,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub messages: Vec<Message>,
    /// The planner's session, while the plan can still be sent back.
    #[serde(default)]
    pub planner_session: Option<String>,
    /// What the owner said when he rejected the plan, for the planner.
    #[serde(default)]
    pub feedback: Option<String>,
    /// Plans the owner rejected.
    #[serde(default)]
    pub rejections: u32,
    #[serde(default)]
    pub report: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Run {
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Tasks done and the total, for the event and the list.
    pub fn counts(&self) -> (usize, usize) {
        (self.tasks.iter().filter(|t| t.state == TaskState::Done).count(), self.tasks.len())
    }
}

/// The first line of the goal, at most 80 characters.
pub fn title_of(goal: &str) -> String {
    let line = goal.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("run");
    crate::agent::children::cut(line, 80).to_string()
}
