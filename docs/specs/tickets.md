# Tickets: a project's list and board

Status: v0.0.3, stage I1 (plan `plans/tickets-and-steering.md` section 2, local). The word is **ticket** everywhere (the
owner's decision of 2026-11-28): the sidebar entry "Tickets", keys like `KA-12`, the store `<data>/tickets/`, the tool
`tickets`, the routes `/api/tickets`. "Task" stays for scheduled tasks (`automations.md`). The per-conversation todo list
(`plan-todo.md` section 1) stays as it is: it is the working list of one multi-step job; a ticket is the owner's own list
that outlives a conversation.

## 1. What is built, and what is not

Built: the store, the routes, the event, the tool, the list, the board, "Mine now", the detail pane. Not built (later
stages of the plan, the data model has room for them): workflow runs and their tasks as tickets (I4), related-work
detection and "went off" signals (I5), align on change (I6), the Jira mirror (I7). `parent`, `links` and `assignee` exist in
the model; `assignee` is `"owner"` or nothing in this item, and no engine reads `parent`.

## 2. A ticket

```json
{"key": "KA-12", "project": "/home/me/code/kagent", "title": "Fix the overlay focus", "description": "markdown",
 "state": "todo", "priority": "medium", "labels": ["ui"], "parent": null,
 "links": [{"kind": "conversation", "value": "<session id>", "label": ""},
           {"kind": "range", "value": "master..v0.0.3", "label": ""}],
 "assignee": null, "doneWhen": ["the overlay keeps focus after a click"],
 "notes": [{"at": "...", "by": "owner", "text": "..."}],
 "history": [{"at": "...", "by": "owner", "what": "state: todo -> in_progress"}],
 "createdAt": "...", "updatedAt": "..."}
```

- `key`: `<prefix>-<number>`. Unique across projects (prefixes are unique), never reused: the number comes from the
  project's counter `next`, which only grows, also when a ticket is deleted.
- `project`: the folder path of the project, or `null` for the **Inbox** (conversations without a project; prefix `IN`).
- `state`: `backlog`, `todo`, `in_progress`, `in_review`, `done`, `dropped`. A new ticket is `todo` unless it says
  otherwise. `priority`: `none` (default), `low`, `medium`, `high`, `urgent`.
- `labels`: strings. `parent`: the key of another ticket of the same project, or `null`; a parent that does not exist or
  would make a cycle is refused. `links[].kind`: `conversation` (a session id), `commit` (a commit id: 7 to 40 hex
  characters), `range` (a git range `from..to` or one ref, for the diff view, `diff.md` section 6) or `url` (an http(s)
  URL). `assignee`: `null` or `"owner"`.
- `history`: one entry per change: who (`owner` for the API, the window and the owner's voice; `agent` for the `tickets`
  tool), what (`created`, `state: todo -> done`, `title`, `priority: none -> high`, `labels`, `description`, `doneWhen`,
  `links`, `parent`, `assignee`, `note added`). The newest 200 entries are kept. A description is not copied into its
  entry.

**Limits** (a refusal says which and what to do): title 1..200 characters, description at most 20 000, 500 tickets per
project (done and dropped count: delete or use another project), at most 20 labels of 1..40 characters (no comma, trimmed,
duplicates dropped), at most 20 `doneWhen` lines of 1..300, at most 50 links (a `value` of at most 500), at most 200 notes
of 1..2000 characters, a `by` that is not `owner` or `agent` is not accepted from a body.

## 3. The store

`<data>/tickets/<id>.json`, mode 0600, written atomically (a temporary file in the same folder, then rename); `<id>` is
`inbox` for the Inbox and `<folder name, lower case, [a-z0-9-] only, at most 24>-<8 hex of the sha-256 of the path>` for a
project. File shape (`config.md` section 16):

```json
{"project": "/home/me/code/kagent" | null, "prefix": "KA", "next": 13, "tickets": [ {Ticket} ]}
```

Each stored ticket also carries `project` (the same value); the file's own `project` wins when the daemon reads it.

- The **prefix** of a new project: the initials of the words of its folder name (split at `-`, `_`, space, `.` and at a
  lower-to-upper letter change), upper case, 2 to 4 letters; a one-word name gives its first two letters (`KAgent` is `KA`,
  `web-ui` is `WU`); a name without letters gives `PR`. A prefix another project has gets a digit appended (`KA2`), then
  `KA3`. It is fixed when the file is created and never changes.
- A file that cannot be read (damaged JSON) is skipped with a warning and left on disk; a change to that project answers 500
  "the tickets file of <project> is damaged: <path>". The daemon never overwrites it.
- One lock serializes every change; the event goes out after the file is written.

## 4. API (`api.md` section 12)

| Route | Body | Answer |
|---|---|---|
| GET `/api/tickets` | query `project` (a path, or `inbox`), `state`, `label`, `q` (text in key, title, description), all optional | `{projects: [{id, path, name, prefix, count}], tickets: [Ticket]}`; every project that has a file, and the Inbox always |
| POST `/api/tickets` | `{project?: path or null, title, description?, state?, priority?, labels?, parent?, links?, assignee?, doneWhen?}` | `201` the ticket; the project must be an existing folder (the check of `sidebar.md` section 3), else 400 |
| PUT `/api/tickets/:key` | any of the fields above, and `note?: string` (appends a note); a field left out is kept | `200` the ticket; `project` cannot be changed (400) |
| DELETE `/api/tickets/:key` | | `204`; a ticket that has this one as `parent` loses its parent |

Errors: 400 `invalid_input` (a limit, a bad state, a parent that does not exist), 404 `not_found` for an unknown key, 409
`conflict` for the 500 limit. Request types are `deny_unknown_fields`, camelCase.

Event: `{"type":"ticketChanged","key":"KA-12","project":"/home/me/code/kagent"|null,"removed":false}`, sent after every create,
change and delete (also for a change by the tool). A window reads `GET /api/tickets` again.

## 5. The tool `tickets` (group `agent`)

`tickets {action: add | update | list | close, ...}` (`tools.md` section 30). `list` is read-only; `add`, `update` and `close`
are changes that ask like any mutating tool (risk low, `needs_approval`) and are `always_ask` like the calendar changes
(`calendar.md`): a card in chat in every mode but `full`, and a call from live voice always asks out loud in every mode
(`voice.md` section 12, `voice-control.md` section 4). The project is the folder of the calling session (a subagent has its
parent's), else the Inbox; `project` in the input is not accepted. Results are worded per `agent-loop.md` section B:

- `add {title, description?, state?, priority?, labels?, done_when?, parent?}`: "Added KA-14 \"Title\" (todo, medium) to the
  project kagent."
- `update {key, title?, description?, state?, priority?, labels?, done_when?, parent?, note?}`: "KA-14 updated: state todo ->
  in_progress." Nothing to change is `not_done("nothing to change on KA-14", "pass a field to change")`.
- `close {key, note?}`: state `done`; "KA-14 is done." A ticket already done is `not_done`.
- `list {state?, label?, text?, all_projects?}`: at most 30 lines `KA-14 [in_progress, high] Title (labels)`, and "(N more)"; a
  list of nothing says "No tickets in kagent." An unknown key is `not_done("no ticket KA-99", "call tickets list to see the keys")`.
  The tool reads the calling project's tickets, or all with `all_projects: true`.
- History entries of the tool's changes say `by: agent`.

The description says: "Use it when the owner says 'add to the todo: ...', 'add a ticket', 'remind me to ...' and no multi-step
job is running. `todo_write` is only the working list of a multi-step job you are doing now." One line of the system prompt
(`prompt.rs`, the rules table) says the same: guidance, not enforcement. A call by an MCP client (`mcp`) or the window's
session `ui` has no project: the Inbox.

## 6. The view

The sidebar gets an entry **Tickets** (icon `list`, between Tasks and Alerts; `nav.ts` and the daemon's `ui` allow-list
`VIEWS` hold `tickets`). The view, `web/src/views/Tickets/`:

- A header with the view switch **List | Board | Mine now** (a group of `aria-pressed` buttons, remembered in this browser),
  the filters (a project select with "All projects" and the Inbox, a label select, a text box "Filter tickets"), and the
  button **New ticket** (also the key `c` when no text field has focus).
- **List** (default): sections per state in the order in progress, in review, todo, backlog, done, dropped (done and
  dropped closed by default), each with its count. A row: key, title, priority mark, labels, project name (when "All
  projects"), assignee mark. A click, or Enter on the focused row, opens the detail pane. Quick add: an input at the top of
  every section, "Add to <state>", Enter creates a ticket in that state in the selected project (with "All projects": the
  Inbox).
- **Board**: a column per state, a card per ticket (key, title, priority, labels), quick add at the top of every column. A
  card is dragged to another column to change its state (HTML drag and drop; the column is highlighted); the keyboard
  alternative is the **Move to** menu on every card (a button `Move <key>`, a menu with the six states, Escape closes it).
  A moved card also works with a focused card and the keys `[` and `]` (previous and next state).
- **Mine now**: tickets in `in_progress` and `in_review` with the assignee `owner` or none, grouped by state.
- **The detail pane** (right side of the view, below it at 700 px): every field edited in place and saved on blur or Enter
  with one PUT (title, description as a markdown text area with the rendered preview under it, state, priority, labels as
  chips, parent as a select of the project's tickets, assignee, `doneWhen` lines), notes (add), links (add and remove: a
  conversation opens in the chat), the history (newest first, "owner" or "agent", the time), Delete (asks "Delete KA-12?"),
  and a tab **Changes** (next to **Details**) with the diff view (`diff.md` section 6): with a `range` or `commit` link it
  shows its diff, without one it says so and has a field to link one. A refusal of the daemon is shown in the pane as an alert.
  In a window narrower than 900 px the pane takes the whole view (the list is behind it) and Close goes back to the list.
- The view reads `GET /api/tickets` when it opens and on every `ticketChanged`. An empty project says "No tickets yet.
  Press c to add one."; a refusal of the daemon is shown as its message under the field.
- Accessibility: every control is reachable by keyboard, icon buttons have labels, the board has a keyboard way to move a
  card, the state of a drag is announced in a live region ("KA-12 moved to in review").

## 7. Files

`crates/kagentd/src/tickets/{mod.rs (types, limits), store.rs, ops.rs (the changes and their history), http.rs, tool.rs,
tests*.rs}`; `web/src/views/Tickets/{index.tsx, List.tsx, Board.tsx, Detail.tsx, api.ts, model.ts}`. Files stay under 400
lines. No new settings key is added (none to classify in `settings_tool/keys.rs`).

## 8. Tests

Unit tests in the module (store round trip, atomic write, never-reused keys, prefixes and collisions, each limit, history,
parent cycles, the tool's words and its project), API tests through the same functions the routes call, the headless suite
`scripts/test-tickets-web.py` (scenarios in `scenarios.md`, S-tickets).
