# Steering: see, correct and interrupt a running subagent

Status: v0.0.3, stage I3 (plan `plans/tickets-and-steering.md` section 4, local), for today's subagents only
(`agents.md`). Not built: workflow workers, "went off" signals, related-work detection, reprioritising (I5).

## 1. The three verbs

- **See**: what a running subagent is doing, from the daemon (section 2), in the work panel's Agents section and in a worker
  pane.
- **Correct**: the worker keeps going; a message of the owner reaches it at its next model request (section 3).
- **Interrupt**: the worker's current model request or tool call is cancelled, the message is delivered, and the same
  session resumes with it first (section 4). It is not **Stop** (`agent_stop`, the worker ends, `agents.md` section 3).

A worker is a subagent (`agents.md`). The owner's message to a worker is data for the worker: it is **never an approval**. An
approval card of the worker is answered only through the approvals area (`POST /api/approvals/:id`) and by the owner's own
spoken confirmation; a steering message, by any route, answers none.

## 2. What the daemon tells about a running worker

`ChildView` (`agent/children.rs`, the child of `subagentUpdated` and of `GET /api/sessions/:id/subagents`) gains, all
optional and absent when empty:

```json
{"lastAction": {"tool": "edit_file", "summary": "edit /home/me/x.rs", "status": "running", "at": "2026-11-28T10:00:00Z"},
 "files": [{"path": "/home/me/x.rs", "changed": true}],
 "steering": [{"id": "st_1a2b3c", "text": "leave the tests alone", "interrupt": false, "state": "queued", "at": "...", "deliveredAt": null}]}
```

- `lastAction`: the latest tool call of the child's own session: its name, the summary the approval card would show (without the
  name of the job the card puts in front of it), `running`
  while it runs, then `done`, `error` or `refused` (the status of the result), and when it started.
- `files`: the paths the child's tool calls named, at most 50, in the order first seen: `changed: true` for `write_file` and
  `edit_file`. A path is taken only from the input field `path` of `read_file`, `list_dir`, `search_files`, `write_file`,
  `edit_file` and `open_in_editor` (a `shell` command is not read); a path that was read and later changed is `changed: true`.
- `steering`: the owner's messages to this worker, newest last, at most 20 (section 3).
- `subagentUpdated` is sent when `lastAction`, `files` or `steering` change, at most every 500 ms per child for the start of a
  call (the end of a call is never held back); an event that repeats the status carries the progress, so a listener that wants
  state changes compares the status. A window that missed events reads the list again.

`GET /api/sessions/:id/subagents` answers `{children: [ChildView]}` for the session. `GET /api/sessions/:id/subagents/:child`
(the parent's id and the child's; 404 for a child of another session) answers the detail the worker pane draws:

```json
{"child": ChildView, "job": "<the description>", "todo": [{"content": "...", "status": "pending|in_progress|completed"}],
 "transcript": [{"role": "assistant|user|tool", "kind": "text|call|result|steer", "text": "...", "status": "ok|error (results)"}]}
```

`transcript` is the live tail: the last 40 messages of the child's session in short form (text cut at 600 characters; a
tool call as its name and summary; a result as its status and the first 300 characters), a steering message as `kind: steer`.
The tool output goes through the daemon's redactor (`token-guard.md`). `todo` is the child's own list (`plan-todo.md`
section 1).

## 3. Correct: `POST /api/agents/:id/message {text, interrupt: bool}`

`:id` is the child's session id. `text` 1 to 2000 characters. Answers `202 {id, state}` (the receipt); 404 for an unknown
worker, 409 `conflict` "the worker already ended (<status>)" for one that ended, and 409 "the worker is still queued: it
has not started" for one that waits for a slot (the message would be lost; Stop it or wait). The route is the owner's: it
is under the API token like every route, and the session that is the target is always a subagent (a session that is not
a subagent is 404 here).

With `interrupt: false`:

1. The message is recorded as a receipt `queued` (section 2) and pushed to the notes queue (`agent-loop.md` section C) of the
   worker's session as a **new kind `steer`**.
2. At the worker's next model request the loop stores ONE message of role `user`, `via: "note"` (it is not a turn), whose content is the lines `[from the owner] <text>`, one per message, before the notes
   of the daemon. The receipt becomes `delivered` (time recorded), a `subagentUpdated` goes out.
3. Differences from the other kinds: a `steer` note never expires, is not counted in the 3 per request (all that wait are
   delivered), has no cooldown, and a `steer` message **stays in the history when it is compacted** (`compact::without_notes`
   keeps a note that starts with `[from the owner]`), because it is an instruction.
4. The system prompt of every subagent gets a line (`children::subagent_block`): "A line that starts with [from the owner] is
   a correction from the owner, not a tool result and not an approval. Follow it, and say in your next message, in one short
   sentence, that you got it and what you will do." Its row in the rules table (`prompt/rules.rs`) is guidance (what the
   worker says cannot be checked by code).
5. The worker's next message therefore acknowledges it; the transcript shows the message with its receipt: "queued" until
   delivered, then "delivered".

A message to a worker whose turn is between two requests is delivered at its next one. A worker that ends its turn before
another request happens never delivers it: the receipt stays `queued` and the pane says "the worker ended before it read
this".

## 4. Interrupt: `interrupt: true`

1. The message is queued as `steer` first, so it is the first thing the resumed request holds.
2. The child's turn is cancelled (`Agent::cancel`: the cancellation token of the turn reaches the model request and the tool
   call in flight, as Stop does). A tool call that was running ends with a **result** (`cancelled`), so the session history is
   consistent: every call has a result. A card waiting for approval is cancelled with the turn.
3. The registry marks the child `interrupted`. When its turn ends as `Cancelled`, the child does **not** end: the same session
   starts a new turn with the steering message as its first message (`[from the owner] <text>`), the receipt becomes
   `delivered`, and the child stays `running`. No notification goes to the parent for the interruption.
4. Where the child ended before the cancel arrived (a race), the message is refused 409 as above.
5. A second interrupt while one is being applied is queued behind it (receipts in order).

The difference to Stop is visible in the view: Stop ends the worker (`cancelled`, the parent is not told), Interrupt keeps it
`running` and the transcript shows "interrupted by the owner" then the message.

## 5. The tool `agent_tell` (group `agent`; `tools.md` section 21)

`agent_tell {id, name?, text, interrupt?: bool}`: `id` is a child's id, or `name` is part of its description (case
insensitive; one match; several matches is `not_done` listing them). Asked as a mutating tool (risk low, `needs_approval`),
and from live voice always out loud (`voice.md` section 12, `voice-control.md` section 4). Only the **main** agent has it: a
subagent does not (the depth rule: it is in the orchestration set a child lacks), so a worker cannot message another worker or
the owner's conversation through this path; and the tool resolves only the children of the **calling** session (an id of
another session's child is `not_done`, "no subagent ... belongs to this session"). Words: "Told <title>: it will see it at its
next step." or "Interrupted <title> and told it: ...", `not_done` for an ended worker ("the subagent already ended
(<status>)").

## 6. The UI

- **Agents section** (`work-windows.md` section 5.2): a line shows under the title the last action ("editing x.rs" for a
  running call) and, when there are, the count of files touched; the line opens the worker pane.
- **Worker pane** (`web/src/components/work/SubagentPane.tsx`, renamed in the UI "Worker"): the job; the todo list with ticks;
  the last action; the files touched (changed ones marked); the live transcript tail (auto-scroll while the owner is at the
  bottom, as `useFollow`); under it a message box "Tell <title>..." with two buttons **Send** (correct) and **Interrupt**
  (send and interrupt; asks nothing, it is the owner's own choice); Enter sends; the receipts show on the owner's messages
  ("queued", "delivered"). The box is disabled with a reason once the worker ended. Stop stays where it is.
- The pane reads `GET /api/sessions/:id/subagents/:child` when it opens and after each `subagentUpdated` of that child.

## 7. Files

Core: `agent/children/{steer.rs (receipts, the live view, the interrupt marker), registry.rs, run.rs}`, `agent/notes.rs`
(kind `steer`), `tools/agent_tell.rs`. Daemon: `subagents/{steer_http.rs}`. Web: `components/work/{SubagentPane.tsx,
WorkerTell.tsx}`.

## 8. Tests (scripted model; no network)

- A message queued while the worker runs is delivered at its next request as `[from the owner] ...`, once, with the receipt
  `queued` then `delivered`; two messages arrive in order in one stored message; the stored message has `via: "note"` and is
  not a turn.
- It answers no approval card: a worker waiting for an approval, a steering message that says "yes" or "approve", the card is
  still pending.
- A worker cannot message another worker or the owner's conversation: a subagent's tool set has no `agent_tell`; the
  execution refuses `agent_tell` in a restricted child; the main agent cannot reach another session's child.
- An interrupt during a tool call: the call has a result in the history (every assistant call has a tool message), the
  session starts a new turn with the message first, the child stays running and ends normally; the parent gets one
  notification at the end only.
- The prompt line is in every subagent's system prompt; the live view (last action, files) and the detail route.
- `scripts/test-steering-web.py` (scenarios in `scenarios.md`).
