# Spec: automations: scheduled tasks (S28)

The tasks module existed (`crates/kagentd/src/tasks/`): reminders and unattended agent
prompts with a schedule in words ("in 20m", "every 30m", "weekdays 08:30"), stored in
`<data>/tasks.json` and run by a 5-second loop, without a spec (its code cited a
`docs/specs/tasks.md` that never existed) and without routes in api.md. This spec is
that spec, extended with what ZCode's automations have: real intervals, cron, a run
policy, delivery channels and a run history.

## 1. Task record (`<data>/tasks.json`, array, atomic write, camelCase)

```json
{
  "id": "task_1a2b3c4d", "title": "Morning health check",
  "what": "Run doctor. If the score is 90 or more reply NOTHING.",
  "kind": "agent",
  "schedule": {"type": "interval", "unit": "day", "n": 1, "at": "08:00", "weekdays": [1,2,3,4,5]},
  "deliver": ["desktop", "session"], "onApproval": "deny", "session": "new", "timeoutMin": 30,
  "enabled": true, "createdAt": "2026-11-24T10:00:00+01:00", "nextRun": "2026-11-25T08:00:00+01:00",
  "lastRun": null, "lastStatus": null, "lastResult": "", "lastSession": "sess_...", "sessionId": null,
  "runs": [{"at": "2026-11-25T08:00:12+01:00", "status": "ok", "durationSec": 42, "sessionId": "sess_...", "result": ""}],
  "done": false
}
```

- `kind`: `remind` (a notification) or `agent` (a prompt run unattended).
- `schedule` variants, `deny_unknown_fields` on each:
  - `{"type": "once", "at": "<RFC 3339 with offset>"}`
  - `{"type": "interval", "unit": "minute"|"hour"|"day"|"week"|"month", "n": 1..200, "at"?: "HH:MM", "weekdays"?: [0..6], "from"?: "YYYY-MM-DD"}`
    (`at` only for `day` and longer, default `08:00`; `weekdays` for `day` and `week`;
    `from` is the first day of the periods of an interval of several days, of weeks or
    of months, set by the parser; without it the count starts on 1970-01-05)
  - `{"type": "cron", "expr": "<5 fields>"}`
  - `null`: no schedule, run on demand.
- Old records (`At{at}`, `Every{minutes}`, `Daily{time, days}`) load through serde
  aliases.
- `id` is `task_` and 8 hex; `lastRun` is the start of the last run; `session` is `new`
  or `reuse` (default `new`); `timeoutMin` is 1..1440; `delayMinutes` is 1..525600. A
  record or a request that leaves a policy field out gets it from `settings.tasks`
  (config.md section 11). `schedule` is also accepted as the stored object, and under
  its old name `when`.
- `runs` keeps the last 50 runs, with a `running` entry while a run goes; `runsBefore`
  is the count of a file from before the history; a run's `result` is at most 500
  chars; `lastResult` 2000. A record that cannot be read is left out with an error in
  the log and is lost at the next save.

## 2. Schedules (pure: `tasks/schedule.rs`, `schedule/parse.rs`, `cron.rs`; local time)

- In words (the parser, extended): `once` for "in 20m", "at 14:30", "tomorrow 07:00",
  "monday 09:00"; `interval` for "every 30m", "every 2 hours", "every day at 09:00",
  "weekdays 08:30", "mon,fri 18:00", "every 2 weeks on monday 09:00".
- Cron: 5 fields (minute, hour, day of month, month, day of week) with `*`, lists,
  ranges and steps. Detected when the text starts with `cron ` or has four spaces and
  contains `*` or `/`. Day of month and day of week both set means either.
- The tool and the API also accept `delayMinutes` (1..525600). The daemon computes `at`
  from its own clock: the model never does date arithmetic (ZCode's rule).
- `next_run(schedule, after) -> Option<DateTime<Local>>` is computed from `after`,
  never from a finish time. Cron searches minute by minute, within 366 days (a cron
  that matches nothing in that time has no next run); day, week and month intervals
  are computed from their period. A clock time the clocks skipped (spring forward)
  runs an hour later; one that comes twice runs at the first.
- A `once` task whose time passed more than 60 seconds before it could run is not run:
  the run is recorded `missed` and the task is `done`.
- A scheduled run more than 5 minutes late (the daemon was down or busy) is recorded
  `missed` (one entry, for the slot that was due) and skipped; the schedule goes on
  from now. Missed runs are never replayed. "Run now" on a task whose slot is late
  runs it.
- Text that does not parse -> 400 with three examples.

## 3. Runner (`tasks/runner.rs`)

- One loop every 15 seconds: find due tasks (`enabled`, `nextRun <= now`), claim each
  before starting it (set `lastStatus: running` and the next `nextRun`).
- At most `tasks.maxConcurrent` (default 2) unattended turns at once; the rest wait.
- `remind`: a `Notification` to the `deliver` channels (`notify.rs`).
- `agent`: a session per the `session` policy (`new`: a session with origin `task`,
  titled "Task: <title>"; `reuse`: `sessionId`, created on the first run). Prompt:
  "Scheduled task '<title>' runs unattended. Reply with exactly NOTHING if nothing
  needs the owner's attention." plus `what`. Sent with `Via::Task`. The runner waits
  with `wait_idle` for `timeoutMin` minutes, then cancels (status `timeout`); the
  limit of 45 minutes of S17 is gone, and after a timeout or a cancel the runner waits
  5 s more for the turn to end and keeps its partial text.
- Approvals in an unattended run:
  - `deny` (default): the runner answers the card at once (it shows for a moment and
    is resolved as denied, in the audit log too), the call is refused with "unattended
    task: this needs approval on the desktop", and the final message lists what was
    skipped.
  - `wait`: the card stays in the UI (and in Telegram when S29 is on) until it is
    answered or `approvalTimeoutSec` passes.
  - Session-scoped allowances (foundations.md section 2) keep allowances given in chat
    out of these runs.
- Result: the final assistant text (at most 2000 chars). A result that is exactly
  `NOTHING` is stored empty and sends no notification. A turn without an answer is
  `failed` ("the agent gave no answer"); a cancelled run sends no notification.
- Delivery: `desktop` (Notifier), `session` (always stored; the session is reachable
  from Tasks and listed in Chat), `telegram` (the paired Telegram chat; nothing while
  the bot is off). Every delivery
  is one log line that says whether the desktop sink is off (`KAGENT_NO_NOTIFY`).
- At daemon start, runs left `running` are marked `failed` ("daemon restarted").
- Events: `tasksChanged` after every save, `taskRun` after every run. The
  `Notice{level: "tasks"}` convention is removed.
- `TaskRunner` is the trait the loop depends on (`send`, `wait_idle`, `cancel`,
  `session`, `deny`, `events`); the tests use a fake and pass the clock.

## 4. Tool `tasks` (group `tasks`; tools.md section 23)

- Actions unchanged: list, add, update, delete, run, pause, resume, done. New: `cancel`
  (a running run) and `history` (the last 10 runs).
- `add`: `{title, what, kind: remind|agent, schedule?: string | delay_minutes?:
  integer, deliver?, on_approval?, session?, timeout_min?}`; `what` is one text or its
  lines (`anyOf`, joined; the S20 trap); `timeout_min` 1..1440; `delay_minutes`
  1..525600; `update` takes the same fields; a key that is not a field is refused.
  `list` is one line per task; `history` the last 10 runs, oldest first.
- Assessment unchanged: list and history are read-only; the rest is MUT low without
  approval.
- The description tells the model: give the schedule in words or as cron; for "in N
  minutes" use `delay_minutes`; tasks run without the owner, so a change that needs
  approval is skipped unless the permission mode allows it; prefer `remind` for plain
  notifications.

## 5. API (api.md section 6)

The existing routes (`GET`/`POST /api/tasks`, `PUT`/`DELETE /api/tasks/:id`,
`POST /api/tasks/:id/run`) plus `GET /api/tasks/:id/runs` (`{runs: TaskRun[]}`) and
`POST /api/tasks/:id/cancel`. Every route answers the whole list `{tasks}` (201 for
`POST /api/tasks`), 404 for an unknown id, 409 for a run on a running task. Each task
in a response also carries `scheduleText` (human summary from the parser, for example
"every 30 minutes" or "weekdays at 08:30") and `nextRun`. `when` is still accepted as
the name of the schedule text.

Three routes for the UI: `GET /api/tasks/preview?schedule=<text>`
(`{schedule, scheduleText, nextRun}`, 400 when the text does not parse),
`GET /api/tasks/templates` (`{templates: [{id, title, what, kind, schedule,
scheduleText}]}`) and `POST /api/tasks/templates/:id` (201, `{tasks}`: the task of the
template, paused).

## 6. UI: Tasks tab (`web/src/views/Tasks.tsx`, `views/tasks/*`)

- Form: schedule presets as chips (In ..., Every ..., Daily, Weekdays, Weekly, Custom),
  a free-text field for words or cron, and a "Runs: <scheduleText>" preview from the
  daemon's parse (`GET /api/tasks/preview`, so it cannot disagree with what is
  stored); delivery checkboxes; approval policy; session policy; the time limit of a
  run in minutes (`timeoutMin`, 1..1440, shown with the default of `settings.tasks`). A
  policy field the owner does not touch is left out of the POST, and the daemon takes
  `settings.tasks`.
- Settings: the defaults of `settings.tasks` have the page Automations (`tasks`,
  `ui-settings.md` section 3).
- Row detail: history of the last 10 runs (time, status, duration, result, open
  session) and Cancel while a run is active.
- Status pills: running, ok, failed, missed, timeout, cancelled.
- Templates (`GET /api/tasks/templates`), shown while there are fewer than three
  tasks. Each adds a paused task:
  - Morning health check: weekdays 08:00, agent: "Run doctor. If the score is 90 or
    more reply NOTHING, otherwise summarize what lowers it."
  - Disk report: every monday 09:00 (weekly: `monday 09:00` alone is a single run),
    agent: "Report disk usage per filesystem and the three largest folders in the home
    directory."
  - Heartbeat: every 30 minutes, agent: "Check open alerts and failed units. Reply
    NOTHING when there is nothing new."

## 7. Tests (no root, no network; no wall-clock waits)

- Schedule parser table: the words of section 2 and cron strings -> schedule; bad
  strings.
- `next_run` for each type with a fixed `now`: month lengths, a daylight-saving day
  (Europe/Ljubljana, spring forward), no drift over three runs (computed from the
  scheduled time), `once` in the past -> missed, 6 minutes late -> missed,
  `delayMinutes` from a fixed clock.
- Storage: old record shapes load through the aliases; round trip; an unknown schedule
  field is rejected; an unreadable record is left out.
- Runner, with the `TaskRunner` trait and a fake: the NOTHING rule; a timeout cancels;
  approval policy `deny` versus `wait`; the concurrency limit; `reuse` keeps one
  session; startup marks stale runs failed.
- API: validation errors for a bad schedule and an unknown policy value.
- End to end, by hand with a test daemon (a temporary `KAGENT_HOME`, the tray and
  notifications off, only while the owner's daemon is stopped): scenario S28 items 3
  to 6.
