# Spec: work windows: agents, plan, progress (S43)

Needs S41 (`Sheet`, `popup.css`) and S42 (the layout of the window). A coder run of phase 5
(`coders.md`) is a child like a subagent and shows in the same places.

## 1. The code today

- **Subagents** (S31, `agents.md`): the registry `agent/children/{registry,run}.rs` keeps
  each child as `queued`, `running`, `done`, `failed` or `cancelled` and sends
  `subagentUpdated {sessionId, child, status}` on every change; Stop is
  `POST /api/sessions/:id/subagents/:child/stop` (`kagentd/src/subagents/http.rs`). The
  web keeps them in `store.children[sessionId]` and `Chat.tsx` draws an `AgentsCard` per
  child in the transcript (`agentCards`, `placeAgents` in `components/agents.ts`). A click
  on a card calls `openSession(child.id)`: the child replaces the conversation in the
  main area, with a read-only header (`.agent-readonly`) in place of the composer.
- **Background shell tasks** (S31): `Background::start(session, command, cwd)`
  (`tools/background.rs`) answers a `TaskInfo {id, status, exit_code, seconds, output_file}`.
  A `Task` keeps its id, its session, its file and its state, not its command. Only the
  tools `task_output` and `task_stop` (`tools/tasktools.rs`), `stop_all` and the
  notification to the session reach the registry: there is no route and no event, so the
  UI knows of a running command only from a tool card, and after a reload not at all.
- **Plan** (S24): `PlanCard` (`components/PlanCard.tsx`) in the approvals area; the row
  `PlanChip` above the composer while the session plans; the Plan toggle in the composer.
- **Todo** (S23): `TodoPanel` above the composer (`components/TodoPanel.tsx`), and in the
  widget a chip "3/7" in the status line.
- The widget has no agents, no plan and no task in view.

From S43 on a capsule above the composer shows what goes on and expands into four
sections, and a side pane on the right shows a subagent, the plan and the output of a
command, the pattern of ZCode's `ConversationStatusPanel` and of its side pane. The
transcript keeps the tool cards, the plan approval card and the agents cards.

## 2. Data

Nothing new on disk. In memory (`tools/background.rs`):

- A `Task` keeps `command: String` and `started_at: DateTime<Utc>` beside `started: Instant`
  (both set where the task is made in `spawn`).
- What the API and the UI see:

```json
{"id": "bg_1a2b3c4d", "sessionId": "...", "command": "cargo test", "status": "running",
 "exitCode": null, "startedAt": "2026-11-27T10:00:00Z", "seconds": 42,
 "outputFile": "/home/me/.local/share/kagent/task-out/bg_1a2b3c4d.log"}
```

`BackgroundTaskView {id, session_id, command, status: TaskStatus, exit_code, started_at, seconds, output_file}`
is built from the task and its state (`Task::view`). `TaskInfo` and what `task_output`
answers do not change.

In the browser (`web/src/components/work/state.ts`, gone with the page):

- The tabs of the side pane: `SideTab { key, kind: "subagent" | "plan" | "output", owner, sessionId, taskId?, title }`.
  `owner` is the session whose work the tab shows (the parent of a subagent), `sessionId`
  the child session, the session of the plan, or the session that started the task.
  Keys: `subagent:<childId>`, `plan:<sessionId>`, `output:<taskId>`.
- The tasks of each session that was asked: `Record<sessionId, BackgroundTaskView[]>`.
- localStorage `kagent.work.closed`: the ids of the sections the owner closed.

## 3. API (`api.md` section 8)

| Method path | Request | Response |
|---|---|---|
| GET `/api/sessions/:id/background` | | `{tasks: BackgroundTaskView[]}` |
| GET `/api/background/:taskId/output?tail=4000` | | `{task: BackgroundTaskView, output: string}` |
| POST `/api/background/:taskId/stop` | | `BackgroundTaskView` |

- The list holds the tasks of that session only: those running, in the order they
  started, then those ended, the newest first; at most 50.
- `tail` is absent (4000) or a whole number from 100 to 20000; anything else is
  400 `invalid_input` "tail must be a whole number from 100 to 20000". It is the last
  characters of the output file, as `task_output` reads them.
- The output goes through the daemon's redactor (`services.redact`, token-guard.md) as a
  tool output does: a log that happens to hold the API token never shows it.
- 404 `not_found`: "session <id> not found" for the list; "no background task '<taskId>'"
  for the other two (unknown, or ended long ago and forgotten: the registry keeps the
  latest 200).
- 409 `conflict` on stop: "the task already ended (<status>)".

The event, over `/ws`:

```json
{"type":"backgroundTask","taskId":"bg_1a2b3c4d","sessionId":"...","status":"running"}
```

`status` is `running` at the start, `killed` on a stop, and the final `done`, `failed` or
`killed` when the task ends. It goes out before the task's notification reaches the
session. A killed task still reports nothing to the session (agents.md section 4).

The work is in `crates/kagentd/src/background_api.rs` (new), on the pattern of
`model_api.rs`: `pub async fn list(sessions: &SessionStore, tasks: &Background, id: &str) -> ApiResult<Value>`,
`pub async fn output(tasks: &Background, redact: Option<Arc<dyn Fn(&str) -> String + Send + Sync>>, id: &str, tail: Option<usize>) -> ApiResult<Value>`,
`pub async fn stop(tasks: &Background, id: &str) -> ApiResult<Value>`; `server.rs` gets
the three routes.

## 4. Rules

### 4.1 The registry

- `Background::list(&self, session: &str) -> Vec<BackgroundTaskView>`: the tasks of the
  session, running first in the order they started, then ended, the newest first; cut at 50.
- `pub type TaskEventSink = Arc<dyn Fn(&str, &str, TaskStatus) + Send + Sync>;` (task id,
  session, status) and `Background::set_event_sink(&self, sink: TaskEventSink)`, beside
  `configure`. The sink is called where the status changes: in `start` after the task is
  entered (running), in `stop` when it returns true (killed), in `supervise` once the
  status is final and before the notification (not for a task a stop already said). A
  registry without a sink (every test of S31) behaves as today.
- `protocol.rs`: `AgentEvent::BackgroundTask { task_id, session_id, status }`.
- The daemon sets the sink in `subagents/runner.rs`, where it configures the registry:
  the closure sends the event. `Subagents::start` gets the event sender for it (one
  argument more, `events.clone()` in `main.rs`).

### 4.2 What the capsule counts

Everything is about the open session:

- children: its children that are `queued` or `running` (`store.children[sessionId]`);
- background: its tasks that are `running`;
- the plan: `planning` while the session plans, else `plan` when it has an approved plan;
- todo: the items not `completed`, over all the items, when the list is not empty.

The capsule is drawn when at least one of them is not zero or empty.

### 4.3 What the pane does not do

A child answers with the model of its job whatever the pane shows; the pane reads, it
never sends. The approvals of a child stay in the approvals area of the main window.

## 5. The UI (`ui-settings.md` section 12)

### 5.1 `WorkPanel` (`components/WorkPanel.tsx`)

In Chat, above the composer, where `TodoPanel` and the `PlanChip` row are today (both
leave that place). On the composer's margins (`0 max(16px, calc((100% - 792px) / 2)) 6px`).

**The capsule**: one button, 26 px high, padding `0 10px`, radius 999 px, `--glass-2`,
1 px rim, the items 10 px apart, a chevron at the end that turns when the panel is open.
`aria-expanded`, `aria-controls`. From the left:

- the `bot` icon and the number of children waiting or running;
- the `terminal` icon and the number of commands running;
- `planning` in the ice colour while the session plans, else `plan` in `--dim` when a plan
  is approved;
- the `list` icon and "3/7" (open over all) while the session has a todo list.

The numbers are `--ink`, the icons `--dim`; the first two items carry the pulsing dot of
a running subagent (`agent-pulse`) while something runs. The `title` says it in words
("2 agents, 1 command, 3 of 7 to do").

**The panel**: under the capsule, the look of `.todo-panel` today (the rim, the medium
radius, `rgba(18, 26, 46, 0.7)`, white in the light theme), at most `min(46vh, 420px)`
high; it scrolls as a whole. Four sections, and a fifth, Activity, once there is a notice (`components/work/WorkSection.tsx`), each a
header of 28 px (a chevron, the title, a count in `--dim`, the actions at the right) and
a body; a click on the header folds and unfolds it. A section the owner has not touched
is open when it has something and closed when it has nothing; what the owner folded is
remembered (`kagent.work.closed`).

### 5.2 The sections

**Agents** (`components/work/AgentsSection.tsx`). One line per child of the open session,
in the order of `agentCards` (the events first, then those only the transcript tells
of): the dot by state and the pulsing while it runs (`.agent-dot` of `agents.css`), the
title, the profile (`general`, `explore`, `claude-edit`...), the state, the time
(`elapsedOf`, ticking while it runs), Stop while it waits or runs (the same
`stopSubagent`). The count in the header: "2 of 3 running", or "3". A click on the line
opens its Subagent tab (5.4). With no child: "No agents yet".

**Background** (`components/work/BackgroundSection.tsx`). One line per task of
`GET /api/sessions/:id/background`: the `terminal` icon, the command (one line,
monospace, 12 px, cut with an ellipsis, the whole command in `title`), the state
(running: the pulsing dot and the elapsed time from `startedAt`; `exit 0` in `--ok`;
`exit 1` in `--bad`; `killed` in `--signal`), Stop while it runs
(`POST /api/background/:taskId/stop`). A click on the line opens its Output tab. The
count in the header: the number running, or "none running". With no task: "No commands
in the background".

**Plan** (`components/work/PlanSection.tsx`).

- "Planning": on, with "Cancel plan" (`POST /api/sessions/:id/plan {active: false}`, as
  the chip does today), or off in `--dim`. The Plan toggle stays in the composer: it
  arms the next message; this only says the state and cancels.
- The approved plan (`approvedPlan` of the session's meta): its first line, and "Open",
  which opens the Plan tab (5.4). A plan that waits for an answer (`store.approvals` has
  one with `kind: "plan"` for this session): "A plan waits for your approval" and "Show",
  which opens the Plan tab. Neither: "No plan yet".

**Progress** (`components/work/ProgressSection.tsx`). `TodoPanel` as it is: a tick per
item, "3 of 7", Clear; `PUT /api/sessions/:id/todo` at each click. It loses its own box
(`components/todo.css`: no margin, border or background) and sits in the section. With no
list: "No todo list".

**Activity** (`components/work/ActivitySection.tsx`). The last 20 notices of the open
conversation (`notices.md`): one line each with the icon of its kind, the level, the text
(one line, the whole text on hover), "x3" when the same `key` and kind came again in a row
(one entry, the latest text and time) and the time. The page's own notices (no `sessionId`)
are listed in every conversation. The log lives in the page's store (`components/toasts.ts`)
for the life of the page and is not persisted; closing a toast does not remove its line.
The section is drawn only when there is at least one entry; the capsule then shows a
`bell` item with the count, and the panel opens even when nothing else is going on. At most
20 are kept per conversation. Newest first.

### 5.3 When the lists are read

- The tasks of a session are read when the session opens and when the panel opens.
- `backgroundTask` for the open session reads them again (one GET); for any other
  session its tasks are dropped and read the next time they are needed.
- Children come from the store, as today.

### 5.4 `SidePane` (`components/work/SidePane.tsx`)

380 px at the right of the view, in the full window only. With no tab for the open session
the pane is not drawn and the 380 px are not reserved.

- **Tabs** belong to the open session (the `owner` of 2): the pane shows those whose owner
  it is. Tabs of another session stay in memory and come back when that session is
  opened; a tab goes when its session is deleted. Opening a tab that is open already
  makes it the active one. Only one is active.
- **Header** (32 px): with one tab, its icon, its title and a close button
  (`aria-label="Close"`, the `close` icon). With several, a tab bar: per tab a button
  with its icon and its title (at most 140 px, cut with an ellipsis) and its own `x`; the
  active one has the ice colour and a line of 2 px under it. The bar scrolls sideways
  when the tabs do not fit (no visible scrollbar).
- **Subagent** (`components/work/SubagentPane.tsx`): the child session, read-only: under
  the tab bar its line (title, profile, state, time, Stop), then its messages with the
  bubbles and the tool cards of the conversation (`Bubble` and `ToolCard` move from
  `views/Chat.tsx` to `components/transcript.tsx`, a pure move). The messages are the
  store's: `messages[childId]`, `streaming[childId]` and `running`, which the events fill
  for every session; the first read is `GET /api/sessions/:id` into `messages` through
  `patch` (S42). It follows the end while the owner is at the bottom; scrolled up, it
  stops and shows a floating button with the `arrowDown` icon ("Jump to the end") that
  resumes.
- **Plan** (`components/work/PlanPane.tsx`): the approved plan as Markdown
  (`approvedPlan.text`), else the plan that waits as Markdown (`preview` of its approval)
  with the note "Approve or reject it in the conversation", else "No plan yet".
- **Output** (`components/work/OutputPane.tsx`): the command, the state and the elapsed
  time; Stop while it runs; the output in a `pre` (monospace, 12 px, wrapped). While the
  tab is the active one and the task runs, it is read again every second
  (`?tail=8000`) and follows the end; a last read at the end event, then it stops.
  Scrolled up (more than 8 px from the bottom), following pauses; the floating
  `arrowDown` button resumes it.
- Escape with the focus inside the pane closes the active tab.
- Below 900 px of viewport width the pane is an overlay: `position: absolute; right: 0;
  top: 0; bottom: 0; width: min(380px, 100vw)` over the view, with a scrim; the same
  header and close button.

### 5.5 What changes in Chat

- A click on an `AgentsCard` in the transcript opens its Subagent tab instead of the main
  area: the conversation stays. The read-only header of a subagent opened directly (a
  link with `#session=`, `ui show`) stays.
- `PlanCard` stays in the approvals area, in Chat, Voice and the widget.
- `PlanCard.tsx` loses `PlanChip` (its CSS rule in `plan.css` stays unused).

### 5.6 The widget (`views/Overlay.tsx`)

- The capsule (5.1, font 11 px) above the bar, in the place of the todo chip, which goes.
  A click opens the sheet "Work" (`Sheet` of S41; `sheet` is `"work"`).
- The sheet holds the four sections, full width. The lines open nothing in a pane: the
  side pane is for the full window only (section E). Stop works there.

### 5.7 The layout (`App.tsx`)

`main` holds the view and, while the view is `chat`, the pane:
`<main className={pane ? "with-pane" : ""}> <div className="view">...</div> <SidePane/> </main>`.
The rules are in `components/work.css`: `main.with-pane { flex-direction: row; }`,
`.view { flex: 1; min-width: 0; display: flex; flex-direction: column; min-height: 0; }`,
`aside.side-pane { flex: 0 0 380px; border-left: 1px solid var(--rim); display: flex; flex-direction: column; min-height: 0; }`.

## 6. Files

```
web/src/
  components/WorkPanel.tsx              5.1: the capsule and the panel: < 190 lines
  components/work/WorkSection.tsx       a section that folds: < 60 lines
  components/work/AgentsSection.tsx     < 110 lines
  components/work/BackgroundSection.tsx < 110 lines
  components/work/PlanSection.tsx       < 90 lines
  components/work/ProgressSection.tsx   < 50 lines
  components/work/SidePane.tsx          5.4: the pane and its tab bar: < 150 lines
  components/work/SubagentPane.tsx      < 120 lines
  components/work/PlanPane.tsx          < 70 lines
  components/work/OutputPane.tsx        < 150 lines
  components/work/state.ts              2 and 5.3: the tabs, the tasks, the handlers on `events`: < 170 lines
  components/work.css                   5.1, 5.4, 5.7: < 300 lines
  components/transcript.tsx             Bubble and ToolCard, moved from views/Chat.tsx
  components/AgentsCard.tsx             the click opens a tab
  components/PlanCard.tsx               no PlanChip
  components/todo.css                   no box of its own
  components/Icon.tsx                   bot, terminal, list, close, arrowDown
  views/Chat.tsx                        WorkPanel; no TodoPanel, no PlanChip row
  views/Overlay.tsx                     the capsule and the sheet
  App.tsx                               5.7
  events.ts                             backgroundTask
  store.ts                              unchanged (the work state is its own module)

crates/kagent-core/src/
  tools/background.rs           `command` and `started_at` in a task, the sink and its calls
  tools/background/views.rs (new)   BackgroundTaskView, `Task::view`, `Background::list`, TaskEventSink
  protocol.rs                   AgentEvent::BackgroundTask

crates/kagentd/src/
  background_api.rs (new)       section 3 and its tests: < 200 lines
  server.rs                     the routes
  subagents/runner.rs           the event sink
  main.rs                       `events` to `Subagents::start`
```

## 7. Scenarios

`scenarios.md` has no S43 block at the time this spec is written; the stage agent copies
this list there as "S43 work windows: agents, plan, progress".

1. **[auto]** `tools/background`: `list` gives the running tasks first and the ended ones
   newest first, with the command and `startedAt`, and only those of the session asked;
   the sink sees `running` at the start, `killed` on a stop and the final status at the
   end; a registry without a sink behaves as before (the tests of S31 unchanged).
2. **[auto]** `background_api.rs`: the 404s, the 400 of a `tail` out of range, the 409 of
   a task that ended; `output` gives the tail and nothing of a token planted in the log.
3. **[auto]** Scripted model: `shell {background: true}` gives `backgroundTask running`
   and the list holds the command; at the end the final status and the notification of
   S31; `stop_all` gives `killed`.
4. Headless Chrome (dark and light, 1200x820): with two subagents and a todo list the
   capsule shows `2` and "3/7"; a click opens the panel; Agents lists both with state,
   time and Stop (the route of S31); Background lists a command and its state; Plan says
   "Off", "Planning" with Cancel plan, then the approved plan; Progress ticks and clears
   as before.
5. Headless: a click on an Agents card opens the Subagent tab and the conversation stays;
   two more tabs make a tab bar; a tab closes with its `x`; the Plan tab shows the
   approved plan; the Output tab follows a running command and stops following when
   scrolled up.
6. Headless at 700x500: the panel leaves the composer its room; the pane is an overlay
   that closes.
7. Headless, in the widget (340x460): the capsule above the bar opens the sheet "Work"
   with the four sections; Stop works there.
8. The owner, live: "Research in parallel what uses the disk and what uses the memory,
   then summarize", with a background command: the capsule counts, the pane follows a
   subagent and the output, Stop ends one.

## 8. ZCode references

ZCode is at `/home/virus/SystemInterface/ZCode/packages/`; the paths are under `ui/src/`.
Take the pattern, not the code: KAgent has no Radix, no resizable-panel library and no
virtual lists.

| File | Take |
|---|---|
| `v4/composer/ConversationBackgroundWorkTrigger.tsx` | the whole file (151 lines): a typed count (an icon and a number per kind, lines 115 to 135), nothing drawn at zero (line 66) |
| `v4/ConversationStatusPanel.tsx` | the kinds of section and how high each scrolls (lines 230 to 249); `StatusSection` and its header (251 to 360): a section that folds, with a capped body; `PlanStatusSection` (879 to 918); `RunningStatusItem` (975 to 1024): the title, the time, Stop; `SubagentStatusSection` (from 1433); the chain that decides what the capsule shows first (1722 to 1848) |
| `lib/workspaceSidePane.ts` | the records of a tab: `BackgroundBashSidePaneTab` (lines 173 to 179), `SubagentSessionSidePaneTab` (181 to 195), `PlanDetailSidePaneTab` (224 to 237); a tab belongs to the session whose work it shows (`ownerTaskId`) |
| `app-shell/SubagentSessionSidePane.tsx` | the whole file (64 lines): a child session drawn read-only in a pane |
| `app-shell/BackgroundBashOutputSidePane.tsx` | the whole file (134 lines): the status line, following while at the bottom (89 to 108), the button that resumes (117 to 130) |
| `app-shell/PlanDetailSidePane.tsx` | the whole file (111 lines): a plan as Markdown in a pane |
| `app-shell/sidePaneLayout.ts` | lines 1 to 35: the least width of a tab and when the bar overflows |

## 9. Tests

- `crates/kagent-core/src/tools/background/tests.rs` and `views.rs`: `list` in its order
  and cut, for one session only, with the command and `startedAt`; the sink and its
  three moments; the tests of S31 pass unchanged with no sink. (Scenario 1.)
- `crates/kagent-core/tests/agent_loop.rs` or a new `tests/background.rs` on the rig of
  `tests/common/mod.rs`: a background `shell` call emits the events and the list holds
  the task; `stop_all` emits `killed`. (Scenario 3.)
- `crates/kagentd/src/background_api.rs`: every refusal of section 3; the tail; the
  redactor over a planted token. (Scenario 2.)
- The headless pass on an own dev server with stubbed routes: scenarios 4, 5, 6 and 7.

Scenarios: `scenarios.md` S43.

## 10. Documents of the same stage

- `api.md` section 8: the three routes and the event `backgroundTask`.
- `agents.md`: section 5 (the three routes) and section 6 (the cards open the pane; the
  Agents and Background sections).
- `plan-todo.md`: section 1 and section 2 (the panel and the plan state live in the work
  panel; the chip above the composer is gone).
- `ui-settings.md` section 12 (new): section 5 of this spec.
- `scenarios.md`: S43 (section 7).
