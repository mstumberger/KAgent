# Spec: todo lists and plan mode (S23, S24)

Two small agent features from ZCode. A todo list keeps multi-step work on track and
shows the owner the progress. Plan mode makes the agent research first and present a
plan for a single approval before it changes anything.

## 1. Todo lists (S23)

- State: one list per session in `SessionMeta.todo` (config.md section 11), kept in
  memory in `SessionState` and saved with `save_meta` on every change.
- Tools: `todo_write`, `todo_read` (tools.md section 19), group `agent`.
- Tool description of `todo_write` (the guidance lives there, not in the prompt): use it
  for work with three or more steps; not for single answers; one item in progress at a
  time; mark an item done as soon as its work is done; update the list together with
  starting the next step.
- Event: `todoUpdated {sessionId, items}` after every change.
- UI:
  - Chat: a panel above the composer while the list is not empty. Checkbox per item
    (a click marks a pending or in_progress item completed, and a completed one
    pending), a mark for `high`, "3 of 7", and Clear.
    Clicks go to `PUT /api/sessions/:id/todo`. The daemon normalises what it receives
    like `todo_write` does.
  - Widget: a progress chip "3/7".
  - Opening a session shows its list (`GET /api/sessions/:id`).
- The list is in the session file's meta, so it survives a restart.
- The compaction prompt asks for open todo items (`compaction.md` section 3).

## 2. Plan mode (S24)

- Purpose: for a request that changes the system in several steps, the agent researches
  read-only, writes a plan, the owner approves it once, then it executes.
- Enter:
  - Composer: `/plan <request>` or a "Plan" toggle next to Send. The request carries
    `plan: true` (`POST /api/sessions/:id/messages` gains `plan?: boolean`); a text
    that starts with `/plan ` also sets it and loses the prefix. The session's
    `planning` flag is set before the turn starts.
  - The model calls `plan_enter` (tools.md section 19).
- While planning, `PermissionEngine::evaluate` gets a new step between read-only mode
  and the destructive step: `planning && !read_only && tool not in PLAN_ALLOWED` ->
  Deny "plan mode: research only until the plan is approved".
  `PLAN_ALLOWED` = `todo_write`, `todo_read`, `plan_enter`, `plan_exit`, `load_skill`,
  `memory_search`, `memory_get`, `session_search`.
  - It sits in the engine, so rules, overrides, hooks, `trusted` and `full` cannot
    bypass it.
  - A subagent started from a planning session gets the same restriction
    (`agents.md` section 2).
- Exit: `plan_exit {plan}` raises an `ApprovalRequest` with `kind: "plan"`, tool
  `plan`, summary "Approve plan", `preview` = the plan text, `can_remember: false`.
  - Approved: `planning` is cleared. The plan is saved to
    `<data>/plans/<sessionId>-<n>.md` and `SessionMeta.approvedPlan` holds its path
    and text (at most 12 KB). The tool result asks the model to carry the plan out step
    by step with the normal permissions.
  - Denied with a reason: still planning; the result carries the reason.
- Cancel: "Cancel plan" in the chat header area (`POST /api/sessions/:id/plan
  {active: false}`) clears `planning` and denies the plan card that waits for an
  answer; the rest of a running turn follows the normal mode.
- Every change of the flag emits `planUpdated {sessionId, active}`.
- The flag is the session's, not the turn's: it stays until a plan is approved or the
  owner cancels it.
- While planning, every request carries a plan-mode block that names `plan_exit`
  (`plan::PLAN_PROMPT`).
- After compaction the prompt carries the approved plan ("Approved plan, follow it:"
  plus the plan) while `approvedPlan` is set: the agent loop appends it to the system
  prompt of every model request of the session, read from the meta before each request
  (the config closure gets no session).
- Voice: a plan card is answered like any other card (`voice.md`); the changes that
  follow still go through the normal permission mode.
- UI: a `PlanCard` in the approvals area (title "Plan", Markdown body, Approve, Reject
  with a reason field); a "planning" chip next to the composer with Cancel plan.

## 3. Tests (no root, no network)

- todo: `todo_write` normalises two `in_progress` items and a missing priority; the
  list round-trips through the meta; over 50 items -> error; the event is emitted.
- plan: `evaluate` table in plan mode (read-only allowed, `PLAN_ALLOWED` allowed,
  mutating denied even in `full` and with an allow rule); `plan_exit` outside plan mode
  -> error; approve clears the flag and stores the plan; deny keeps the flag and
  returns the reason.
- `tests/agent_loop.rs` (scripted model): a planning turn where the model tries a
  mutating call, gets the denial, then `plan_exit`; the approved plan is in the next
  request's system prompt.
- API: `plan: true` on a message; the `/plan ` prefix; cancel.
