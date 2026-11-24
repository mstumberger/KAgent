# Spec: subagents and background tasks (S31)

A subagent is a second agent loop with its own session, system prompt, model and tools,
started by the main agent for one job. It keeps long research or parallel work out of
the main conversation. Background shell tasks are commands that keep running while the
turn goes on. Both come from ZCode (its `Agent` tool, `Explore` agent and background
Bash).

## 1. Profiles (`<config>/agents/<name>.md`, `kagentd/src/subagents/profiles.rs`)

```markdown
---
name: disk-audit
description: use for disk usage investigations; read-only
model:
toolGroups: [files, system, hardware]
maxIterations: 25
prompt: |
  Extra system prompt text for the subagent.
---
```

- Frontmatter: the skills parser (`skills.md` section 1). Fields: `name`
  (`^[a-z0-9][a-z0-9-]{0,63}$`), `description` (required, at most 500 chars),
  `toolGroups` (absent = every enabled group), `maxIterations` (1..80, default 25),
  `model` (a model ref; empty = the chat model), `prompt` (at most 8000 chars).
- Built-ins (not files; they cannot be edited or deleted):
  - `general`: every enabled group.
  - `explore`: read-only. Any call whose assessment is not read-only is refused with
    "explore is read-only".
- An invalid profile file is skipped with a warning and listed in the UI with the
  reason.

## 2. Running a subagent

- Orchestration is in `kagent-core/src/agent/children.rs`, so it is tested with the
  scripted model of `tests/agent_loop.rs`. `kagentd/src/subagents/runner.rs` only wires
  profiles, the tool and the API.
- `agent_run` (tools.md section 21) creates a child session: origin `subagent`,
  `parent` = the calling session, title = `description`.
- The child's system prompt = the base prompt + "You are a subagent. <description of
  the job>. End with a complete final answer: it is returned to the agent that started
  you." + the profile `prompt`. The job prompt sent to the child is preceded by the
  parent's digest (`SessionStore::digest`, 2000 chars) when the parent has a history,
  so the child knows what the conversation is about.
- Tools = the profile's groups, enforced at execution: `Agent::set_session_tool_groups
  (session_id, groups)` makes `execute_call` deny a call outside them ("not in this
  subagent's tool groups"). `explore` sets a per-session read-only flag that denies
  every call whose assessment is not read-only.
- The child uses the permission engine with its own session id: its cards carry the
  subagent's name, and allowances are session-scoped (foundations.md section 2).
- Concurrency: at most `subagents.maxConcurrent` (default 4) running children in the
  daemon. More are queued and reported as `queued`.
- `stop_all` cancels children too. Deleting a parent session cancels and deletes its
  children.
- Depth: a subagent cannot start subagents (`agent_run` is not among its tools).
- Plan mode: a child of a planning session gets the plan restriction
  (`plan-todo.md` section 2).

## 3. Results

- `agent_wait` (tools.md section 21) blocks the calling turn for up to `timeout_sec`.
- Completion notification. When a child ends, the parent session gets a stored message
  with role user and `Via::Agent`:
  `[Agent "<description>" <done|failed|cancelled> in 42 s]` and the output (at most
  4000 chars).
  - Parent idle: a new turn starts with it, so the main agent reacts without being
    asked.
  - Parent running: it is queued as steering.
- `subagentUpdated {sessionId, child, status}` is emitted on every state change.
- The child's session is listed under its parent (`GET /api/sessions?parent=<id>`) and
  hidden from the main list.

## 4. Background shell tasks

- `shell {background: true}` (tools.md section 23) runs through the existing exec path
  in its own process group and returns `{taskId, outputFile}` at once. Ids are
  `bg_<8 hex>`.
- States: running, done (with the exit code), failed, killed. The output file keeps its
  last 1 MiB. At most 20 running tasks; a further one is refused.
- At daemon exit and on `stop_all` every task gets TERM, then KILL after 2 s (the
  existing `kill_group` path).
- Completion sends the starting session the same kind of notification message:
  `[Task <taskId> finished, exit 0]`.

## 5. API

api.md section 6: `/api/agents` and `GET /api/sessions?parent=`.

## 6. UI

- Chat: an "Agents" card in the transcript (from `subagentUpdated` and the tool
  result): profile, state, elapsed time, an output preview (1500 chars), Stop. A click
  opens the child session read-only, with a link back to the main session.
- Session list: children are not shown; the parent row shows "3 agents".
- Settings -> Subagents: the list (built-ins marked), an editor (name, description,
  model, tool groups as chips, max iterations, prompt), max concurrent, Delete.

## 7. Tests (no root; loopback only)

- Profiles: parsing; an invalid name or a missing description is skipped; a built-in
  name cannot be overwritten.
- Tool filter: a call outside the groups is denied; `explore` denies a mutating call.
- `children.rs` in `tests/agent_loop.rs` (scripted model): a finished child gives an
  idle parent a new turn with the notification; a busy parent gets it queued; the
  concurrency limit; `stop_all` cancels; the depth limit.
- Background tasks: the output cap keeps the tail; the limit of 20; stop-all kills
  (`sleep 60`, as the existing exec tests do).
