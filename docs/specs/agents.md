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
- The name of a profile is the name of its file. A frontmatter `name` that differs, a
  key that is not one of the six, an unknown tool group or a value out of range makes
  the file invalid: it is skipped and listed with the reason. The text after the
  frontmatter is not read.
- The folder is read before each `agent_run` and on `GET /api/agents`.
- Built-ins (not files; they cannot be edited or deleted):
  - `general`: every enabled group.
  - `explore`: read-only. Any call whose assessment is not read-only is refused with
    "explore is read-only".
- An invalid profile file is listed in the UI with its reason.

## 2. Running a subagent

- Orchestration is in `kagent-core/src/agent/children.rs` (with
  `children/{registry,run}.rs`), so it is tested with the scripted model
  (`tests/subagents`). `kagentd/src/subagents/runner.rs` only wires the profiles, the
  tool and the API.
- `agent_run` (tools.md section 21) creates a child session: origin `subagent`,
  `parent` = the calling session, title = `description`. It answers an error and makes
  no session when `model` is not a ref (`<providerId>::<modelId>`) or is unknown to
  the providers.
- The child's system prompt = the base prompt + "You are a subagent. <description of
  the job>. End with a complete final answer: it is returned to the agent that started
  you." + the profile `prompt`. The job prompt sent to the child is preceded by the
  parent's digest (`SessionStore::digest`, 2000 chars) when the parent has a history,
  so the child knows what the conversation is about.
- Tools = the profile's groups, enforced at execution: a per-session tool-group
  filter makes `execute_call` deny a call outside them ("not in this subagent's tool
  groups"). `explore` sets a per-session read-only flag that denies every call whose
  assessment is not read-only.
- The child uses the permission engine with its own session id: its cards carry the
  subagent's job name, and allowances are session-scoped (foundations.md section 2).
- Concurrency: at most `subagents.maxConcurrent` (default 4) running children in the
  daemon. More are queued and reported as `queued`.
- `stop_all` cancels children too. Deleting a parent session cancels and deletes its
  children. A queued subagent is stopped with
  `POST /api/sessions/:id/subagents/:child/stop` (the session's `cancel` cannot reach
  it).
- Depth: a subagent cannot start subagents (`agent_run` is not among its tools).
- Plan mode (decision of 2026-11-27): a planning session may start read-only subagents
  only (`explore`, or a profile that is read-only). `agent_run` and `agent_stop` are in
  `PLAN_ALLOWED`, and `start_child` refuses any other profile with "plan mode: only a
  read-only subagent (profile explore) may run until the plan is approved". The child
  also gets the plan restriction (`plan-todo.md` section 2).
- The system prompt has one working rule about subagents while `agent_run` is offered
  (`kagentd/src/prompt.rs`, `SUBAGENT_RULE`): delegate long research or several
  independent jobs, `explore` for read-only research, only `explore` in plan mode.

## 3. Results

- `agent_wait` (tools.md section 21) blocks the calling turn for up to `timeout_sec`.
- Completion notification. When a child ends, the parent session gets a stored message
  with role user and `Via::Agent`:
  `[Agent "<description>" <done|failed|cancelled> in 42 s]` and the output (at most
  4000 chars).
  - Parent idle, and a chat or a voice session: a new turn starts with it, at most 5
    in a row without a message of the owner (`MAX_AUTO_TURNS`; kept at 5 by the decision
    of 2026-11-27), so the main agent
    reacts without being asked.
  - Parent running: it is queued as steering.
  - In the session of a task, an alert investigation or a Telegram chat it is stored
    only: those callers wait with `send` and `wait_idle`, and a turn started by a
    notification would race that wait.
  - A cancelled subagent reports nothing; `subagentUpdated` still says `cancelled`.
- `subagentUpdated {sessionId, child, status}` is emitted on every state change.
- The child's session is listed under its parent (`GET /api/sessions?parent=<id>`) and
  hidden from the main list. Each listed session has `agents`, how many subagents it
  started. The session of a subagent is read-only: a message into it is 409.

## 4. Background shell tasks

- `shell {background: true}` (tools.md section 23) runs through the existing exec path
  in its own process group and returns `{taskId, outputFile}` at once. Ids are
  `bg_<8 hex>`.
- States: running, done (with the exit code), failed, killed. The output file keeps its
  last 1 MiB. At most 20 running tasks; a further one is refused.
- On `stop_all` every task gets TERM, then KILL after 2 s (the existing `kill_group`
  path). At daemon exit `Background::shutdown` sends TERM, waits 2 s and sends KILL
  itself (after the server returns: the thread of `kill_group` dies with the daemon).
  Whether a process group is still alive is read from `/proc`.
- Completion sends the starting session the same kind of notification message:
  `[Task <taskId> finished, exit 0]`.

## 5. API

api.md section 6: `GET /api/agents` (valid profiles and the invalid files with their
reasons), `PUT /api/agents/:name` (201 when it creates, 200 when it changes; a field
left out keeps its value; 400 for an invalid name, a missing description, an unknown
tool group, `maxIterations` outside 1..80 or a model the providers do not know),
`DELETE /api/agents/:name`, `POST /api/sessions/:id/subagents/:child/stop`,
`GET /api/sessions?parent=<id>`. S43 adds the routes of the background shell tasks (`work-windows.md` section 3, `api.md` section 8): the list of a session, the tail of an output and the stop.

## 6. UI

- Chat: an "Agents" card in the transcript (from `subagentUpdated` and the results of
  `agent_run`): profile, state, elapsed time, an output preview (1500 chars), Stop. A
  click opens the child in the side pane of the chat (S43, `work-windows.md`), read-only; the read-only header with a link back to the main session stays for a subagent opened directly (a `#session=` link, `ui show`). The Agents and Background sections of the work panel list the children and the commands. After
  a reload the cards are rebuilt from the transcript; the whole output of a report is
  behind a toggle that starts closed.
- The end of a turn reloads the open chat only (before, the end of any session's turn
  opened that session, which with subagents switched the view to a subagent's
  session).
- Session list: children are not shown; the parent row shows "3 agents".
- Settings -> Subagents: the list (built-ins marked), an editor (name, description,
  model, tool groups as chips, max iterations, prompt), max concurrent, Delete.

## 6a. Steering (v0.0.3, `steering.md`)

- What a running child is doing: `ChildView` carries `lastAction`, `files` and `steering`; `subagentUpdated` is also sent for the
  progress of a running child; `GET /api/sessions/:id/subagents[/:child]` answers the live state, the child's job, its own todo
  list and the tail of its transcript.
- The owner's message: `POST /api/agents/:id/message {text, interrupt}` and the tool `agent_tell` (`Agent::tell_child`). A message
  is pushed to the notes queue as the kind `steer` and read at the child's next model request as ONE stored message
  `[from the owner] ...` (`via: note`, never expires, kept by compaction); the receipt goes from `queued` to `delivered`. With
  `interrupt` the child's turn is cancelled (a running call ends with a `cancelled` result, so every call keeps its result), and the
  session settles into a new turn that holds the message first; the child stays `running` and the parent is told once, when it ends.
- Every subagent's system prompt carries `STEER_RULE`: a line that starts with `[from the owner]` is a correction, not a tool result
  and not an approval, to be acknowledged in one sentence. The message never reaches the approval broker.
- `agent_tell` is among the tools a subagent lacks (the depth rule) and it finds only the children of the calling session.

## 7. Tests (no root; loopback only)

- Profiles: parsing; an invalid name or a missing description is skipped; a built-in
  name cannot be overwritten; a `name` that differs from the file name is invalid.
- Tool filter: a call outside the groups is denied; `explore` denies a mutating call.
- `children.rs` in `tests/subagents` (scripted model): a finished child gives an idle
  parent a new turn with the notification; a busy parent gets it queued; the
  concurrency limit; `stop_all` cancels; the depth limit; the child of a planning
  session is restricted (through `start_child`, because `agent_run` itself is
  mutating); the limit of 5 turns on notifications alone.
- Background tasks: the output cap keeps the tail; the limit of 20; stop-all kills
  `sleep 60`.
