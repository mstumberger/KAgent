# Spec: hooks (S32)

Hooks are commands the owner's own scripts run at points in the agent's life: a
notification somewhere else, a log, an extra guard on one tool. They follow ZCode's
hook protocol (JSON in on stdin, JSON decision out). Configuration belongs to the owner
(`settings.json`, 0600, edited in the UI). KAgent has no project-level hooks, so it
needs no trust flow. The agent must not be able to create or change hooks.

## 1. Events

| Event | When | Payload extra | May decide |
|---|---|---|---|
| `SessionStart` | before the first turn of a session | | no |
| `UserPromptSubmit` | before the first model request of a turn | `prompt` | may add context |
| `PreToolUse` | in `execute_call`, after schema validation, before `authorize` | `tool`, `input` (redacted as in the audit), `assessment`, `summary` | allow, ask, deny; may replace the input |
| `PostToolUse` | after the tool returned | `tool`, `input`, `output` (at most 4000 chars), `isError`, `durationMs` | may add context |
| `Stop` | after `TurnFinished` | `status`, `error` | no |
| `AlertRaised` | when an alert is raised | `alert` | no |

`matcher` (tool names) applies to `PreToolUse` and `PostToolUse` only.

## 2. Running a hook (`kagentd/src/hooks/runner.rs`)

- Command: `command` with `args` (`tokio::process::Command`, no shell), or one line
  through `shell` when it is set. Environment: `PATH`, `HOME`, `LANG`, `USER` only.
- stdin: one JSON object `{event, ts, sessionId, via, ...extra}`.
- Deadline `timeoutMs`: on timeout the process group is killed and the hook counts as
  failed.
- `async: true` (any event): started and not awaited; nothing waits for it, and only its
  context output is used, for the next turn.
- Every run writes an audit record (`AuditKind::Hook`: event, hook id, command, exit
  code, duration, decision).

## 3. Output

- stdout: at most 64 KB, one JSON object (whitespace tolerated):
  `{"decision": "allow"|"ask"|"deny", "reason": "...", "additionalContext": "...",
  "updatedInput": {...}}`.
- Exit code 2 = deny; stderr (first 500 chars) is the reason. Any other non-zero code =
  the hook failed: a non-blocking error. The agent continues as if the hook were not
  there and a `notice` (warn) is shown.
- `decision: allow` never relaxes anything: privileged, always-ask, destructive,
  read-only mode and plan mode keep their rules (the limit `toolOverrides` has).
- `decision: ask` forces a card. `deny` ends the call with "not executed: <reason>".
- `updatedInput` (`PreToolUse` only) replaces the input and goes through schema
  validation again; when invalid, the call fails.
- `additionalContext` goes into the next model request of that session as a stored user
  message `[hook <event>] ...`, at most 4000 chars per turn.

## 4. Where the core calls hooks

- `kagent-core` does not know the daemon's hooks. `ToolServices` gains
  `hooks: Option<Arc<dyn ToolHooks>>`:
  `async fn pre(&self, call, assessment, session_id, input) -> HookInput` and
  `async fn post(...)`, with `HookInput { verdict: Allow|Ask|Deny, reason, input }`.
- The other events come through `AgentDeps.hooks: Option<Arc<dyn AgentHooks>>` with
  `async fn event(&self, event: HookEvent<'_>) -> Option<String>` (the additional
  context). The loop calls it before the first turn of a session (`SessionStart`),
  before the first model request of a turn (`UserPromptSubmit`; its context is added
  to that turn's prompt) and after `TurnFinished` (`Stop`). The daemon calls the runner
  itself where an alert is raised (`AlertRaised`, `watch/mod.rs`).
- `execute_call` calls `pre` between validation and `authorize`.
- A hook that fails or is missing acts as Allow with the unchanged input.
- When no hook matches an event, nothing is spawned and the call path is unchanged.

## 5. Protecting the configuration (same stage)

- `write_file` and `edit_file` on `<config>/settings.json`, `<config>/agents/*`,
  `<config>/telegram.json`, `<data>/audit.jsonl` and `<data>/tasks.json` are
  `always_ask` in every mode and never rememberable (the rule `browser eval` has).
- `shell`: a command whose text contains one of those paths together with a redirect or
  an in-place edit (`>`, `>>`, `tee`, `sed -i`, `rm`, `mv`, or `cp` into the path) is
  `always_ask` as well.
- Reason: in `trusted` and `full` the agent could otherwise write a hook or a rule that
  runs with the owner's privileges at the next event.

## 6. API

api.md section 6 (`/api/hooks`). `POST /api/hooks/:id/test` runs a hook with a sample
payload and returns its output; nothing from a test reaches the agent.

## 7. UI: Settings -> Hooks

- List grouped by event. Rows: command, matcher, async, an enabled toggle, Test, Edit,
  Delete.
- Form: event, matcher (tool chips from `/api/tools`), command, arguments (one per
  line), shell, timeout, run in background. Save, Cancel.
- Test panel: event, a sample JSON (the documented example by default), Run, and the
  result (exit code, stdout, stderr, decision, duration).
- A line at the top: "Hooks run with your user's privileges while the agent works.
  Changes apply from the next turn."

## 8. Tests (no root)

- Config: parsing; an unknown key is rejected; an invalid event is rejected; a matcher
  on another event than the two tool events is rejected.
- Runner: the stdin payload; exit 0 with each decision; exit 2 denies with stderr;
  another exit code is a failure with a notice; a timeout kills (`sleep 30` with a
  200 ms limit); `async` returns at once.
- Core (`tests/agent_loop.rs`): allow cannot relax a destructive call; ask forces the
  broker; deny ends the call; `updatedInput` is validated; a failing hook is
  non-blocking; no hook means no change.
- Protection table: the file tools and the shell patterns are `always_ask`.
