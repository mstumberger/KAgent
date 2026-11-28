# Workers and workflows: runs, the loop and the board

Status: v0.0.3, stages W1 (the engine), W2 (tools, API, communication) and W3 (the board). Not built here: coding agents as
workers (`kind: coder`, W4), park and continue after a usage limit and a worktree per task (W5), the Settings page for
workflows and schedules (W6). A definition with `kind: coder` is refused on load with "coding agents as workers are not
built yet (kind: coder); use kind: subagent".

A **workflow** is a saved recipe: which roles run in which order, with which worker and model, and where the owner must say
yes. A **run** is one execution of it on one job in one project. A **worker** is one agent doing one step: here always a
subagent of KAgent (`agents.md`), with any provider's model. The words are in `GLOSSARY.md`.

## 1. Names

- The sidebar entry is **Runs** (view id `runs`), and it is **the board** of the glossary. "Work" is already the name of the work
  panel of `work-windows.md` (the agents, plan and progress of one conversation, and the widget's sheet "Work"), so the new view
  does not use it. The board reuses the components of the work panel (`components/work/*`) for a run's workers.
- A run's **task** is one unit of work the planner returned. Each task is also a **ticket** (`tickets.md` section 9, section 7 here): the
  word "task" alone still means a scheduled task (`automations.md`).
- Run ids are `run_<8 hex>`; task ids are the planner's (`M.1`, `T2`).

## 2. A definition

Files: `<config>/workflows/<name>.yaml`; the shipped ones are built into the daemon (`workflows/` in the source tree) and are
read-only; a file in the config folder cannot use a shipped name. The agent cannot write these files (no tool does): they decide
what runs unattended. Only a subset of YAML is read (`crates/kagent-core/src/workflow/yaml.rs`: block maps and lists, flow
`{a: b}` and `[a, b]`, plain, single and double quoted scalars, comments; no anchors, no tags, no `|` or `>` blocks); anything else
is refused with its line number.

```yaml
name: plan-implement-review
description: A strong model plans, a worker implements each task, another reviews.
steps:
  - id: plan
    role: planner
    worker: {kind: subagent, profile: planner}
    model: "anthropic::claude-opus-4-1"     # optional: a model ref; else the profile's, else the chat model
    gate: owner                             # the owner approves the plan before any implementer starts
  - id: build
    role: implementer
    for_each: task
    worker: {kind: subagent, profile: implementer}
    parallel: 1
  - id: review
    role: reviewer
    for_each: task
    worker: {kind: subagent, profile: reviewer}
    on_changes: {back_to: build, max_rounds: 3, then: owner}
  - id: wrap
    role: orchestrator
    do: summary
```

Types are `deny_unknown_fields`; an error names the field and the step ("steps[1].parallel: 1 to 8, not 12").

- `name` (`^[a-z0-9][a-z0-9-]{0,63}$`, equal to the file name), `description` (1..300), `steps` (1..8).
- A step: `id` (same pattern as a name, unique), `role` (`planner`, `implementer`, `reviewer`, `orchestrator`), `worker`
  (`{kind: subagent, profile: <name>}`; required for the three worker roles, absent for `orchestrator`), `model` (a ref
  `<providerId>::<modelId>`), `gate` (`owner`; planner only), `for_each` (`task`; implementer and reviewer only), `parallel`
  (1..8, implementer only, default 1), `on_changes` (reviewer only: `back_to` an earlier implementer step, `max_rounds` 1..5,
  `then: owner`), `do` (`summary`; orchestrator only). `output` and `isolation` are accepted for the plan's sake as `plan` and
  `none` only (`isolation: worktree` is refused: "worktrees are not built yet").
- The shapes that make sense: at most one step per role; the order is planner, implementer, reviewer, orchestrator. A
  reviewer needs `on_changes` when an implementer exists; a reviewer without an implementer reviews the working tree once
  (`review-only`). An implementer without `for_each` is refused. A planner without an implementer is `plan-only`.
- A reviewer equal to the implementer (same profile and same model) is refused: "a review by the same model is not a review".
- Profiles are those of `agents.md` plus the three role profiles (section 3.1). An unknown profile is refused on load.
- Shipped: `plan-implement-review`, `implement-review` (no planner: the job is the one task), `plan-only` (ends at the approved
  plan), `review-only` (a reviewer reads the working tree against the last commit and reports).

## 3. The run

### 3.1 Roles

| Role | Profile (built in) | May change files | Gives back |
|---|---|---|---|
| planner | `planner`: read-only, 40 iterations | no (enforced at execution, as `explore`) | the plan (3.2) |
| implementer | `implementer`: every enabled group, 60 iterations | yes | a report: what changed, how it was checked, what is left |
| reviewer | `reviewer`: read-only, 40 iterations | no (enforced) | the verdict (3.3) |

The three profiles are built into the engine (not in `<config>/agents`); a file of `<config>/agents` with the same name takes
precedence. A reviewer is made read-only whatever its profile says. An implementer never decides that its task is done: only
the reviewer's `pass`, or the owner, does.

### 3.2 The plan

The planner's last message must hold one fenced `json` block:

```json
{"summary": "...", "tasks": [
  {"id": "M.1", "title": "Service core", "ticket": "CYB-9868", "description": "...",
   "done_when": ["..."], "depends_on": [], "files": ["src/..."]}
]}
```

- 1..30 tasks. `id` 1..20 characters of `A-Za-z0-9._-`, unique. `title` 1..200. `description` at most 8000. `done_when` 1..8
  lines of 1..300. `depends_on` ids of other tasks, no cycle. `files` at most 30 paths. `ticket` (optional) at most 40
  characters: a key of an outside tracker, shown next to the task. `doneWhen` and `dependsOn` are accepted too. Fields that are
  not listed are ignored (a model's plan is not a config file).
- The last block of the message wins. A malformed plan (no block, invalid JSON, a rule above) is sent back **once** to the same
  planner session with every problem listed and "Return the plan as the JSON block described"; a second malformed plan fails the
  run: state `failed`, reason `the planner did not return a valid plan: <first problem>`.
- Without a planner step the plan is one task: id `T1`, title the first line of the goal, description the goal, `done_when`
  ["the goal is met"].
- The plan is saved as `<data>/runs/<runId>/plan.json` and as `plan.md` beside it.

### 3.3 The loop of one task

1. `waiting` until every task it depends on is `done` (a task whose dependency is `failed`, `dropped` or `blocked` becomes
   `blocked`). Tasks whose `files` overlap never run at the same time.
2. `implementing` (round 1): a new worker gets the goal, the task with its `done_when`, the reports of the tasks it depends on
   and the rules of its role. It ends with its report.
3. `in_review`: a new reviewer gets the task, the implementer's report and the diff of the task's files against the last
   commit (`git diff HEAD -- <files>`; without git, "no git diff: read the files"). It returns:
   `{"verdict": "pass" | "changes", "findings": [{"severity": "must" | "should", "file", "line", "what", "why"}]}` in a fenced
   `json` block. `changes` needs at least one `must`; `pass` has none. A malformed verdict is sent back once to the same
   reviewer; a second one fails the task (`the reviewer did not return a valid verdict`).
4. `pass`: `done`. Without an implementer step (`review-only`) a `changes` verdict also ends the task `done`: the review is the
   product, and the run's report lists the `must` findings. With one, `changes`: only the `must` findings go back; the `should` ones are listed in the run's report. The **same
   implementer session** is continued with them (`changes`, round 2, ...), then reviewed again by a new reviewer.
5. After round `max_rounds` (default and shipped: 3) with `changes` still: `needs_owner`, with both sides' last words. The owner
   picks `more_round` (one more round, the limit of this task is raised by one), `accept` (done as it is, marked accepted) or
   `drop` (`dropped`; its dependents are `blocked`).
6. A worker that fails (its turn ends in error, is cancelled by something else than a stop, or gives no report): the task is
   `failed` with the error; its dependents are `blocked`; other tasks go on.

States. Run: `planning`, `awaiting_approval`, `working`, `paused`, `done`, `stopped`, `failed` (`parked`, W5, is reserved).
Task: `waiting`, `blocked`, `implementing`, `in_review`, `changes` (with `round`), `needs_owner`, `done`, `dropped`, `failed`,
`stopped`. A run ends `done` when every task is `done` or `dropped` and at least one is `done`; `failed` when a task failed or is
blocked; `stopped` by the owner. The run's report (the `summary` step, written by the engine, no model) lists the tasks, the
rounds, the `should` findings and the changed files.

### 3.4 Gates, approval, limits

- `gate: owner` on the planner: the run waits in `awaiting_approval`. The owner approves, rejects with a reason (the planner
  gets the reason in its session and returns a new plan; at most 3 rejections, then the run is `stopped` with "plan rejected
  three times"), or edits the tasks (title, order, remove) before approving. Nothing is skipped: without `gate: owner` in the
  definition the plan is taken as it is (the shipped definitions with a planner have the gate). A removed task that another
  task depends on is refused. By voice the summary and the titles are spoken (the card is `speak`ed as the plan card is).
- Workers of one run never exceed `parallel` of the implementer step, and all workers count against
  `subagents.maxConcurrent` (more are queued by `agents.md`). At most `workflows.maxRuns` runs are active (`planning`,
  `awaiting_approval`, `working`) at once, default 2, 1..4.
- Pause (owner): the workers are cancelled, the tasks stay in the state they were in, the run is `paused`; Continue starts the
  stage of each interrupted task again with a new worker (the prompt says that files may already be changed). A task that was in
  `changes` gets the findings in its new worker's prompt. A paused run takes no decision about a task.

### 3.5 Storage and restart

`<data>/runs/<runId>/run.json` (mode 0600, written atomically: a temporary file in the folder, then rename), `plan.json`,
`plan.md`, `messages/<id>.txt` for long messages. The run holds a copy of its definition. At daemon start every run in a
non-final state is read: `planning` and `working` become `paused` with the reason "the daemon restarted" (never continued by
itself; Continue does it, as above); `awaiting_approval` stays (nothing was running), and so does a task that `needs_owner` inside a paused run (the owner decides after
Continue); final states stay. A file
that cannot be read is skipped with a warning and left on disk.

## 4. Communication

### 4.1 Worker to orchestrator

Two tools, offered only to a worker of a run and refused at execution everywhere else ("report_progress is for a worker of a
workflow run"):

- `report_progress {steps: [{title, status}]}`: its own step list (`pending`, `in_progress`, `completed`; at most 20, titles
  at most 120), shown under its task on the board.
- `ask {question, options?, blocking?}`: `blocking: true` (default): the call waits for the answer, which is its result (up to
  30 minutes, then "no answer came: go on with your best guess and say so"); `blocking: false`: it returns at once and the
  answer arrives later as a message of the mailbox. Questions are at most 1000 characters, at most 6 options.

### 4.2 Who answers

A question is stored on the run (`questions`) and the task shows "asks". If the run was started from a conversation, that
session is told "[question from implementer M.2 in run run_x] ..." (a stored message that starts its turn when it is idle
at the chat, `agents.md` section 3) and the Brain answers with `workflow_answer {run, question, text}`, or hands it to the owner
with `ask_owner: true`. If no answer comes within 2 minutes, or the run was started from the board, the question is
**escalated**: a card on the run (the board shows it, `Answer`), a notice (`notices.md`, key `run:<id>:question`) and the
questions list in `GET /api/runs/:id`. The owner answers with `POST /api/runs/:id/questions/:qid/answer`.

### 4.3 The mailbox

One mailbox per worker, kept in the run (`messages`):

- Every message has an id; a repeat with the same id is the same message (nothing is added).
- A message to a worker waits until the worker is running, and then is queued into its session as one message
  `[from orchestrator] <text>` (read at its next model request, as steering is). At most 8 waiting per worker, 12 KB in all; a
  text over 1500 characters is written to `messages/<id>.txt` and the message names the file.
- Coworker text is never the owner's: the prefix is added by the engine, and a text that starts with `[from ` or `[owner]` is
  refused. It reaches no approval.
- Receipts: `queued`, `delivered`, `expired` (30 minutes without a running moment). They are in `run.json` and on the board.
- The owner types to any worker from the board: that is steering (`steering.md`), not the mailbox.

## 5. Tools (group `agent`; `tools.md` section 31)

| Tool | For | Asks |
|---|---|---|
| `workflow_run {workflow, goal, project?, ticket?}` | the Brain | mutating, `always_ask`: the summary names the workflow, the project and the workers; asked out loud from voice |
| `workflow_status {run?}` | the Brain | no (read-only) |
| `workflow_stop {run}` | the Brain | mutating, `always_ask` |
| `workflow_answer {run, question, text?, ask_owner?}` | the Brain | no (it answers a worker, it approves nothing) |
| `report_progress`, `ask` | a worker | no |

The project is the calling session's folder; `project` is accepted only as one of the known projects (the projects list of
`sidebar.md`). The API takes any folder that the attach check of `sidebar.md` section 3 accepts. A subagent (and so a
worker) has none of the four `workflow_*` tools (the depth rule of `agents.md`). The system prompt has one working rule
(`WORKFLOW_RULE`) while `workflow_run` is offered.

## 6. API and events (`api.md` section 13)

`GET /api/workflows`, `GET|POST /api/runs`, `GET /api/runs/:id`, `POST /api/runs/:id/{stop,pause,continue,plan}`,
`POST /api/runs/:id/tasks/:task/decision`, `GET /api/runs/:id/tasks/:task/diff` (the working tree against the last commit, only
the files the task's worker changed; the board's Changes tab), `POST /api/runs/:id/questions/:qid/answer`. `POST /api/runs` takes
`{workflow, project, goal, ticket?, models?, session?}`: `models` maps a role word to a model ref for this run, `session` is the
conversation that answers the workers' questions. Event `runUpdated`
`{runId, project, state, done, total}` after every change of a run or a task; a window reads the run again. No answer holds a
secret: the API token and provider keys are not part of a run, and worker text passes the daemon's redactor (`token-guard.md`).

## 7. Tickets

A run is a ticket, and each of its tasks is a ticket with `parent` = the run's ticket (`tickets.md` section 9). When the run
starts, the engine creates the run's ticket (title: the first line of the goal; or uses the key the owner gave). Each task becomes a ticket when the plan is
approved. Task state to ticket state: `waiting`, `blocked` -> `todo`; `implementing`, `changes` -> `in_progress`; `in_review`,
`needs_owner` -> `in_review`; `done` -> `done`; `dropped` -> `dropped`; `failed` and `stopped` -> `backlog` with a note that says
why ("the run was stopped before this task was done"; "failed: <error>"), and the label `stopped` for a stopped one. The ticket
of a run follows the run (`working` -> `in_progress`, `done` -> `done`, `failed` and `stopped` -> `backlog` with the note).
History entries say `by: agent`; the run's ticket gets a note with the run id.

## 8. The board (`web/src/views/runs/`)

- Left: projects (the folders runs belong to), under each its runs (title: the goal's first line, a status dot), under a run
  its tasks in plan order (id, title, ticket key, status dot). Dot colours: grey waiting or blocked, blue (spinning) working,
  amber needs the owner, green done, red failed, a hollow dot for stopped and dropped; the status is also in the label for
  screen readers, never by colour alone.
- Centre: the run, in tabs: **Overview** (goal, the plan's summary, the steps as a strip with the active one marked, the
  report when done, Stop, Pause or Continue), **Plan** (the table of tasks: status, round, worker, ticket; the approval card
  while `awaiting_approval` with Approve, Reject with a reason and edit of titles and order; the decision buttons for a task
  that needs the owner; the questions with an answer field), and one tab per opened worker (the work panel's worker pane:
  transcript, steering composer) and, per task, **Changes** (`DiffView` of the task's files).
- Right: **Progress** (the selected worker's own steps, or the run's tasks, as a checklist; done ones struck through, the
  active one with an arrow) and **Agents** ("1 running" with its task and "Running for 12m 9s"; "Ended (n)" opens the list:
  name, state, first line of the report, age).
- **New run**: pick a workflow (its description and workers shown), a project (the open conversation's by default), type the
  goal, optional ticket key; Start posts `POST /api/runs`.
- A click on a task opens its worker tab (the steering pane of `steering.md`: transcript, what it is doing, the message box); the
  worker list on the right opens them too. The plan approval is a card on the Plan tab and the run opens on that tab while it waits.
- Keyboard: every list item is a button or a link in tab order, arrow keys move in the tree, Enter opens, Escape closes a
  dialog; icon buttons have labels; both themes; at 700 px the three columns become one with a back button.

## 9. Files

`crates/kagent-core/src/workflow/{mod, yaml, definition, shipped, model, plan, verdict, prompts, roles, store, git, report,
engine, control, drive, task, worker, mailbox}.rs`, `crates/kagent-core/src/tools/workflow.rs`, `workflows/*.yaml` (shipped),
`crates/kagentd/src/runs/{mod, settings, http, tickets, tests}.rs`, `web/src/views/runs/`, `scripts/test-runs-web.py`.

## 10. Tests

Scripted model (no network): the definition and plan parsers, each path of 3.3 (approve at round 1; changes then approved; three
rounds then each owner choice; a worker that fails; a worker that asks; stop in each state; restart in each state), the
parallel limit, the mailbox, the tools' refusals outside a worker, the ticket sync, the API functions; the headless suite
`scripts/test-runs-web.py`; a gated test (`KAGENT_IT=1`, Ollama `qwen2.5:7b-instruct`) that runs `plan-only` on a trivial goal.
