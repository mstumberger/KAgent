# Spec: hooks (S32)

Hooks are commands the owner's own scripts run at points in the agent's life: a
notification somewhere else, a log, an extra guard on one tool. They follow ZCode's
hook protocol (JSON in on stdin, JSON decision out). Configuration belongs to the owner
(`settings.json`, 0600, edited in the UI). KAgent has no project-level hooks, so it
needs no trust flow. The agent must not be able to create or change hooks.

Hooks are off until the owner configures one: when no hook matches an event, nothing is
spawned and the call path is unchanged.

## 1. Events

| Event | When | Payload extra | May decide |
|---|---|---|---|
| `SessionStart` | before the first turn of a session (its history is still empty; a subagent's first turn too) | | no |
| `UserPromptSubmit` | before the first model request of a turn | `prompt` | may add context |
| `PreToolUse` | in `execute_call`, after schema validation, before `authorize` | `tool`, `input` (redacted as in the audit), `assessment`, `summary` | allow, ask, deny; may replace the input |
| `PostToolUse` | after the tool returned | `tool`, `input` (redacted), `output` (at most 4000 chars), `isError`, `durationMs` | may add context |
| `Stop` | after `TurnFinished` | `status`, `error` | no |
| `AlertRaised` | when an alert is raised | `alert` | no |

`matcher` (tool names) applies to `PreToolUse` and `PostToolUse` only.

## 2. Running a hook (`kagentd/src/hooks/runner.rs`)

- Command: `command` with `args` (`tokio::process::Command`, no shell), or one line
  through `shell` when it is set (one program, run as `<shell> -c <command>`).
  Environment: `PATH`, `HOME`, `LANG`, `USER` only (the daemon's values).
- stdin: one JSON object `{event, ts, sessionId, via, ...extra}` (`via` of a tool
  event is the via of the session's running turn).
- Deadline `timeoutMs`: the hook runs in a process group of its own; on timeout the
  group is killed with KILL and the hook counts as failed. After the hook exits its
  pipes are given 500 ms.
- `async: true` (any event): started and not awaited; nothing waits for it. Its context
  is handed to the next `UserPromptSubmit` of the session, labelled "(from the <Event>
  hook '<id>', run in the background)".
- Every run writes an audit record (`AuditKind::Hook`: event, hook id, command, exit
  code, duration, decision, timed out).

## 3. Output

- stdout: at most 64 KB, one JSON object (whitespace tolerated):
  `{"decision": "allow"|"ask"|"deny", "reason": "...", "additionalContext": "...",
  "updatedInput": {...}}`. Other keys are ignored. Exit 0 with an empty stdout is no
  decision. Anything else that is not one JSON object, an unknown `decision` or a
  field of the wrong type fails the hook.
- Exit code 2 = deny; stderr (first 500 chars) is the reason. Any other non-zero code
  = the hook failed: a non-blocking error. The agent continues as if the hook were not
  there (allow, the input unchanged) and a `notice` (warn) is shown: "Hook '<id>'
  (<Event>) failed: <why>. The agent goes on without it."
- `decision: allow` never relaxes anything: privileged, always-ask, destructive,
  read-only mode and plan mode keep their rules (the limit `toolOverrides` has).
- `decision: ask` forces a card (it cannot be remembered). `deny` ends the call with
  "not executed: <reason>". The first denial ends the hooks.
- `updatedInput` (`PreToolUse` only) replaces the input: it is normalized, validated
  and assessed again before `authorize`; when invalid, the call fails.
- `additionalContext` is stored as a user message `[hook <event>] ...` at the next
  model-request boundary of that session, at most 4000 chars per turn. The context of
  `UserPromptSubmit` is in the first request of its turn, after the prompt.

## 4. Where the core calls hooks

- `kagent-core` does not know the daemon's hooks. `ToolServices` gains
  `hooks: Option<Arc<dyn ToolHooks>>`:
  `async fn pre(&self, session_id, call, assessment) -> HookInput` and
  `async fn post(&self, session_id, call, output, is_error, duration_ms) -> Option<String>`
  (the context), with `HookInput { verdict: Allow|Ask|Deny, reason, input, context }`.
- The other events come through `AgentDeps.hooks: Option<Arc<dyn AgentHooks>>` with
  `async fn event(&self, HookEvent) -> Option<String>` (the context). The loop calls
  it before the first turn of a session (`SessionStart`: its history is still empty),
  before the first model request of a turn (`UserPromptSubmit`) and after
  `TurnFinished` (`Stop`, not awaited since 2026-11-27: it runs in a task of its own
  with a limit of 90 s for all the Stop hooks of a turn, `STOP_HOOKS_LIMIT`, past which
  it is dropped with a warning in the log; each hook keeps its own `timeoutMs`. The
  turn, `wait_idle` and a queued next turn do not wait for it; its context reaches the
  first model request after it is added). The daemon calls the runner itself where an alert is
  raised (`AlertRaised`, `watch/mod.rs`).
- `execute_call` calls `pre` between validation and `authorize`.
- A hook that fails or is missing acts as Allow with the unchanged input.

## 5. Protecting the configuration (same stage)

- `write_file` and `edit_file` on `<config>/settings.json`, `<config>/agents/*`,
  `<config>/telegram.json`, `<data>/audit.jsonl` and `<data>/tasks.json` are
  `destructive` in every mode (the gate that asks in `full` too, the finding of S27:
  `always_ask` is let through by `full`) and never rememberable; the card says
  "potentially destructive". The paths are read from the environment at each
  assessment (`KAGENT_HOME`, else XDG).
- `shell`: a command whose text contains one of those paths together with a redirect or
  an in-place edit (`>`, `>>`, `tee`, `sed -i`, `rm`, `ln`, `mv`, or `cp` into the
  path; a `cd` in the line and the call's `cwd` are followed; `~`, `$HOME` and
  `${HOME}` are read, so are quotes and wildcards) is `destructive` as well.
- Reason: in `trusted` and `full` the agent could otherwise write a hook or a rule that
  runs with the owner's privileges at the next event.

## 6. API

api.md section 6 (`/api/hooks`). `POST /api/hooks/:id/test` runs a hook with a sample
payload and returns its output; nothing from a test reaches the agent.

## 7. UI: Settings -> Hooks

- List grouped by event. Rows (the page draws its own rows): command, matcher, async,
  an enabled toggle, Test, Edit, Delete.
- Form (it opens above the list): event, matcher (tool chips from `/api/tools`),
  command, arguments (one per line), shell, timeout, run in background. Save, Cancel.
- Test panel: event, a sample JSON (the documented example by default), Run, and the
  result (exit code, stdout, stderr, decision, duration).
- A line at the top: "Hooks run with your user's privileges while the agent works.
  Changes apply from the next turn."

## 8. Tests (no root)

- Config: parsing; an unknown key is rejected; an invalid event is rejected; a matcher
  on another event than the two tool events is rejected; a record without `enabled`
  is on.
- Runner: the stdin payload; the environment holds only the four variables; exit 0
  with each decision; exit 0 with an empty stdout; exit 2 denies with stderr; another
  exit code is a failure with a notice; a timeout kills `sleep 30` under a 200 ms
  limit; `async` returns at once; output over 64 KB.
- Core (`tests/hooks`, scripted model): allow cannot relax a destructive call; ask
  forces the broker; deny ends the call; `updatedInput` is validated; a failing hook
  is non-blocking; no hook means no change and no spawn; scenario item 4's rule (a
  `PreToolUse` hook that denies `shell` commands containing `curl`).
- Protection table: the file tools and the shell patterns ask in every mode,
  including `full`.
