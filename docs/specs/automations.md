# Spec: automations: scheduled tasks (S28)

The tasks module exists (`crates/kagentd/src/tasks/`): reminders and unattended agent
prompts with a schedule in words ("in 20m", "every 30m", "weekdays 08:30"), stored in
`<data>/tasks.json` and run by a 5-second loop. It has no spec (the code cites a
`docs/specs/tasks.md` that does not exist) and no API in api.md. This spec is that
spec, extended with what ZCode's automations have: real intervals, cron, a run policy,
delivery channels and a run history.

## 1. Task record (`<data>/tasks.json`, array, atomic write, camelCase)

```json
{
  "id": "task_1a2b3c4d", "title": "Morning health check", "what": "Run doctor. If the score is 90 or more reply NOTHING.",
  "kind": "agent", "schedule": {"type": "interval", "unit": "day", "n": 1, "at": "08:00", "weekdays": [1,2,3,4,5]},
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
  - `{"type": "interval", "unit": "minute"|"hour"|"day"|"week"|"month", "n": 1..200, "at"?: "HH:MM", "weekdays"?: [0..6]}`
    (`at` only for `day` and longer, default `08:00`; `weekdays` only for `day`,
    default all days)
  - `{"type": "cron", "expr": "<5 fields>"}`
  - `null`: no schedule, run on demand.
- Old records load through serde aliases: `At{at}` -> `once`; `Every{minutes}` ->
  `interval` minute; `Daily{time, days}` -> `interval` day with `at` and `weekdays`.
- Policy fields default to `settings.tasks` (config.md section 11) when a record or a
  request leaves them out.
- `runs` keeps the last 50 runs; a run's `result` is at most 500 chars; `lastResult`
  2000.

## 2. Schedules (pure: `tasks/schedule.rs`; local time)

- In words (existing parser, extended): `once` for "in 20m", "at 14:30", "tomorrow
  07:00", "monday 09:00"; `interval` for "every 30m", "every 2 hours", "every day at
  09:00", "weekdays 08:30", "mon,fri 18:00", "every 2 weeks on monday 09:00".
- Cron: 5 fields (minute, hour, day of month, month, day of week) with `*`, lists,
  ranges and steps. Detected when the text starts with `cron ` or has four spaces and
  contains `*` or `/`.
- The tool and the API also accept `delayMinutes` (1..525600). The daemon computes
  `at` from its own clock: the model never does date arithmetic (ZCode's rule).
- `next_run(schedule, after) -> Option<DateTime<Local>>` is computed from `after`,
  never from a finish time (today `Every` drifts by the run's duration). Cron and
  interval search minute by minute for minute-level schedules and day by day for the
  others, bounded to 366 days.
- A `once` task whose time passed more than 60 seconds before it could run is not run:
  the run is recorded `missed` and the task is `done`.
- A scheduled run more than 5 minutes late (the daemon was down or busy) is recorded
  `missed` and skipped. Missed runs are never replayed.
- Text that does not parse -> 400 with three examples.

## 3. Runner (`tasks/mod.rs`)

- One loop every 15 seconds: find due tasks (`enabled`, `nextRun <= now`), claim each
  before starting it (set `lastStatus: running` and the next `nextRun`).
- At most `tasks.maxConcurrent` (default 2) unattended turns at once; the rest wait.
- `remind`: a `Notification` to the `deliver` channels (`notify.rs`).
- `agent`: a session per the `session` policy (`new`: a session with origin `task`,
  titled "Task: <title>"; `reuse`: `sessionId`, created on the first run). Prompt:
  "Scheduled task '<title>' runs unattended. Reply with exactly NOTHING if nothing
  needs the owner's attention.\n<what>". Sent with `Via::Task`. The runner waits with
  `wait_idle` for `timeoutMin` minutes, then cancels (status `timeout`).
- Approvals in an unattended run:
  - `deny` (default): the call is refused with "unattended task: this needs approval
    on the desktop", and the final message lists what was skipped.
  - `wait`: the card stays in the UI (and in Telegram when S29 is on) until it is
    answered or `approvalTimeoutSec` passes.
  - Session-scoped allowances (foundations.md section 2) keep allowances given in chat
    out of these runs.
- Result: the final assistant text (at most 2000 chars). A result that is exactly
  `NOTHING` sends no notification.
- Delivery: `desktop` (Notifier), `session` (always stored; the session is reachable
  from Tasks and listed in Chat), `telegram` (does nothing until S29).
- At daemon start, runs left `running` are marked `failed` ("daemon restarted").
- Events: `tasksChanged` after every save, `taskRun` after every run. The
  `Notice{level: "tasks"}` convention is removed.

## 4. Tool `tasks` (group `tasks`; tools.md section 23)

- Actions unchanged: list, add, update, delete, run, pause, resume, done. New: `cancel`
  (a running run) and `history` (the last 10 runs).
- `add`: `{title, what, kind: remind|agent, schedule?: string | delay_minutes?:
  integer, deliver?, on_approval?, session?, timeout_min?}`.
- Assessment unchanged: list and history are read-only; the rest is MUT low without
  approval.
- The description tells the model: give the schedule in words or as cron; for "in N
  minutes" use `delay_minutes`; tasks run without the owner, so a change that needs
  approval is skipped unless the permission mode allows it; prefer `remind` for plain
  notifications.

## 5. API (api.md section 6)

The existing routes (`GET`/`POST /api/tasks`, `PUT`/`DELETE /api/tasks/:id`,
`POST /api/tasks/:id/run`) plus `GET /api/tasks/:id/runs` and
`POST /api/tasks/:id/cancel`. Each task in a response also carries `scheduleText`
(human summary from the parser, for example "every 30 minutes" or "weekdays at 08:30")
and `nextRun`.

## 6. UI: Tasks tab (`web/src/views/Tasks.tsx`)

- Form: schedule presets as chips (In ..., Every ..., Daily, Weekdays, Weekly, Custom),
  a free-text field for words or cron, and a "Runs: <scheduleText>" preview from the
  parsed schedule; delivery checkboxes; approval policy; session policy.
- Row detail: history of the last 10 runs (time, status, duration, result, open
  session) and Cancel while a run is active.
- Status pills: running, ok, failed, missed, timeout, cancelled.
- Templates, shown while there are fewer than three tasks. Each adds a paused task:
  - Morning health check: weekdays 08:00, agent: "Run doctor. If the score is 90 or
    more reply NOTHING, otherwise summarize what lowers it."
  - Disk report: monday 09:00, agent: "Report disk usage per filesystem and the three
    largest folders in the home directory."
  - Heartbeat: every 30 minutes, agent: "Check open alerts and failed units. Reply
    NOTHING when there is nothing new."

## 7. Tests (no root, no network)

- Schedule parser table: the words of section 2 and cron strings -> schedule; bad
  strings.
- `next_run` for each type with a fixed `now`: month lengths, a daylight-saving day
  (Europe/Ljubljana, spring forward), no drift over three runs (computed from the
  scheduled time), `once` in the past -> missed, 6 minutes late -> missed,
  `delayMinutes` from a fixed clock.
- Storage: old record shapes load through the aliases; round trip; an unknown schedule
  field is rejected.
- Runner, with a trait `TaskRunner {send, wait_idle, cancel}` and a fake: the NOTHING
  rule; a timeout cancels; approval policy `deny` versus `wait`; the concurrency limit;
  `reuse` keeps one session; startup marks stale runs failed.
- API: validation errors for a bad schedule and an unknown policy value.
