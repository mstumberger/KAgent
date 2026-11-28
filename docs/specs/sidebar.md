# Spec: left sidebar and projects (S42)

Needs S41 (`sessionMetaChanged`, `Sheet`, `popup.css`) and, for the two rules on a coder
run, phase 5 (`coders.md`). If S35 to S39 are not in the tree when S42 runs, section 4.5
says what to do.

## 1. The code today

- The navigation is the tab row of the header (`web/src/App.tsx`: `<nav>{VIEWS.map(...)}`,
  `VIEWS` in `nav.ts`; the daemon's `ui.rs` holds the same ids for `ui show`). The session
  list is Chat's own `<aside className="sessions">` (`views/Chat.tsx`): "+ New session"
  and one `.session-item` per session (title, the count of its agents, a delete button).
  `index.css` hides it under 720 px (`@media (max-width: 720px) { .sessions { display:
  none; } }`), so a narrow window has no list at all. The Voice view has a `<select>` of
  sessions of its own.
- Sessions are one flat list, newest first by `updatedAt`: `GET /api/sessions`
  (`kagentd/src/api.rs::list_sessions`) calls `subagents::listed` over
  `SessionStore::visible` (`session.rs`). `SessionMeta` has no folder and no pin.
- The projects exist for the tools only: `xdg::projects(&ctx.desktop_prefs().project_roots)`
  (`crates/kagent-core/src/tools/xdg.rs`) lists the child folders of the roots of
  `settings.desktop.projectRoots` (empty: `DEFAULT_PROJECT_ROOTS`) that hold a marker
  file (`is_project`); `xdg::find_project` finds one by name. Its readers are the
  `projects` tool (`tools/editor.rs`) and the context of a live voice session
  (`kagentd/src/live/context.rs`). The prompt (`kagentd/src/prompt.rs::build`) knows
  nothing of a folder.
- `start_child` (`agent/children/run.rs`) creates a child with `store.create_with(description, SessionOrigin::Subagent, Some(parent))`.
- The settings screen has its own menu (`settings/SettingsPage.tsx`), and the widget
  (`views/Overlay.tsx`) has no list.

From S42 on a left sidebar is the navigation of the whole window, on the pattern of
ZCode's `project-sidebar`, and a project is a folder attached to a session (section E):
the sidebar groups the sessions by folder and the prompt names the folder with its
`AGENTS.md`. No per-project settings in phase 6.

## 2. Data (`config.md` section 12)

`SessionMeta` gains two fields:

```json
"project": "/home/me/code/app",
"pinned": true
```

| Field | Meaning |
|---|---|
| `project` | the folder attached to the conversation: an absolute canonical path (links resolved, `~` expanded), or `None` |
| `pinned` | the session stands in the section "Pinned" of the sidebar; default `false` |

- Both `#[serde(default)]`: a meta file from before S42 loads.
- A child (a subagent, a coder run) has the `project` of its parent (4.2) and is never pinned.
- The browser remembers the sidebar, not the daemon: localStorage `kagent.sidebar` =
  `{"view": "projects" | "recent", "collapsed": boolean, "closed": ["<path>", ...]}`.

## 3. API (`api.md` section 8)

| Method path | Request | Response |
|---|---|---|
| PUT `/api/sessions/:id/project` | `{path: string\|null}` | `SessionMeta` |
| PUT `/api/sessions/:id/pin` | `{pinned: boolean}` | `SessionMeta` |
| POST `/api/sessions` | gains `project?: string` | `SessionMeta` (201) |
| GET `/api/sessions?project=<path>` | | the sessions of that folder, newest first, as the plain list |
| GET `/api/projects` | | `{roots: string[], projects: [{path, name, agentsMd: boolean}]}` |

`PUT /api/sessions/:id/project`:

- `{path: null}` or `{path: ""}` detaches the session. A path that starts with `~/` is
  expanded (`xdg::expand`), then `std::fs::canonicalize` gives what is stored.
- 400 `invalid_input`:
  - no `path` key, or a value that is neither a string nor null: "expected {path: a folder, or null to detach}".
  - a path that is neither absolute nor `~/...`: "path must be absolute, or start with ~/".
  - a path that does not exist or is not a directory: "not a directory: <path as given>".
  - a folder that is `<config>` or `<data>` of KAgent, or inside them (`protect::dirs`):
    "<path> is KAgent's own folder (<config> or <data>): a conversation cannot be attached to it".
    It is the rule of `coders.md` section 3 for a coder's project.
- 404 `not_found` "session <id> not found".
- 409 `conflict` "the session of a subagent cannot be changed: it has the folder of the session that started it".
- The same path again changes nothing: no event, the meta is answered. Otherwise
  `sessionMetaChanged {sessionId, meta}` goes out after the file is written (the event of S41).
  `updated_at` stays: attaching does not reorder the sessions.

`PUT /api/sessions/:id/pin`:

- 400 `invalid_input` "expected {pinned: true or false}" when `pinned` is missing or not a boolean.
- 404 as above. 409 `conflict` "the session of a subagent is not in the list: it cannot be pinned".
- The answer is the meta; `sessionMetaChanged` when the flag changed.

`POST /api/sessions` with `project`: the checks of the PUT run first; a refused create
makes no session. `model` and `reasoningLevel` are those of S41.

`GET /api/sessions?project=<path>`: the top-level sessions whose `project` is that path.
The path is canonicalized when it is a directory, else compared as it is. With `parent`
too: 400 `invalid_input` "give parent or project, not both". No window calls it (the
sidebar groups the one list it loads); it is the list of a folder for scripts and the
headless pass.

`GET /api/projects`: the projects of `coders.md` section 3, as `xdg::projects` lists them
(the roots of `settings.desktop.projectRoots`, empty: the built-in ones), in its order:
`name` is the folder's name, `agentsMd` is true when `<path>/AGENTS.md` is a regular
file; `roots` are the roots as expanded paths.

The work is in `crates/kagentd/src/project_api.rs` (new), on the pattern of
`model_api.rs`: functions over the agent and the settings, tested without a server.

- `pub fn path_of(body: &Value) -> Result<Option<String>, String>`: `None` for null or `""`.
- `pub async fn check(path: &str) -> Result<PathBuf, HttpError>`: the four 400s; `Ok` the canonical path.
- `pub async fn set_project(agent: &Arc<Agent>, id: &str, body: &Value) -> Result<SessionMeta, HttpError>`,
  `pub async fn set_pinned(agent: &Arc<Agent>, id: &str, body: &Value) -> Result<SessionMeta, HttpError>`,
  `pub async fn create_in(sessions: &SessionStore, meta: SessionMeta, path: PathBuf) -> Result<SessionMeta, HttpError>`,
  `pub async fn projects(settings: &Settings) -> Value`,
  `pub fn in_project(all: Vec<SessionMeta>, path: &str) -> Vec<SessionMeta>` (canonicalized when it is a directory).
- `api.rs`: `list_sessions` reads `project` (`subagents::listed` gets the filter);
  `create_session` checks the folder first (`project_api::check`; a refusal makes no
  session), calls `model_api::create`, then gives the new session its folder with
  `project_api::create_in` (the field is set and the meta saved once more; no event:
  nobody lists the session yet).
  `server.rs` gets the two PUT routes on `/sessions/{id}/project` and `/sessions/{id}/pin`
  and `GET /projects`.

## 4. Rules

### 4.1 Storing

`crates/kagent-core/src/project.rs` (new) holds what S42 adds to the agent, as
`agent/picker.rs` does for S41. `Agent::set_project(&self, id: &str, path: Option<String>) -> anyhow::Result<SessionMeta>`
and `Agent::set_pinned(&self, id: &str, pinned: bool) -> anyhow::Result<SessionMeta>`:
under the meta lock the field is set and the file is written (the old value comes back
on an error), then, after the lock, `AgentEvent::SessionMetaChanged { session_id, meta }`
when something changed. `Agent::project(&self, id: &str) -> anyhow::Result<Option<String>>`
reads the field.

### 4.2 Children

`start_child` (`agent/children/run.rs`) and `Agent::start_coder` (`agent/children/external.rs`,
coders.md section 2) give the child the `project` of its parent: the meta `create_with`
returns gets it and is saved before the first turn. `pinned` stays `false`.

### 4.3 The prompt block

`turn_loop` (`agent/mod.rs`), after `apply_choice` (S41) and before the check of the empty model:

```rust
config.system_prompt.push_str(&self.project_block(session_id, state).await);
```

- `Agent::project_block` (`project.rs`): `meta.project` under the meta lock; `None` gives
  an empty string; else `project::block(&path, &project::agents_md(&path).await)`.
- `project::agents_md(dir) -> Option<String>`: reads `<dir>/AGENTS.md` and no other file.
  The path is resolved (`canonicalize`, so a link is followed); `None` when there is no
  such file or it is not a regular file. `None` too when the resolved path names a
  credential file (`protect::is_credential`, token-guard.md): a link must not bring the
  API token into a prompt. At most 64 KiB are read (`take(64 * 1024)`), as lossy UTF-8;
  the first `PROJECT_AGENTS_CHARS = 4000` characters, and "\n(cut: AGENTS.md is longer)"
  when the file has more.
- The text goes through the daemon's redactor (`services.redact`) as a tool output does
  (`agent/calls.rs`, after `budget_output`): `None` in a test, the token guard in the daemon.
- `project::block(path, agents_md) -> String`:

```

Project: /home/me/code/app
This conversation is attached to that folder. Relative paths in the owner's requests are relative to it, and a tool that asks for a folder takes this one unless the owner names another.
Project notes (AGENTS.md; they guide work inside this folder, and the rules above win where they differ):
<the text, when there is one>
```

The block begins with an empty line, as the skill block of `prompt.rs` does.

- The block is part of `config.system_prompt`: every model request of the turn has it,
  the history budget counts it (`system.len()`), a compaction or a fallback cannot lose
  it, and it is never stored in the session.
- It is read once per turn: a folder attached while a turn runs is in the prompt from the
  next turn.
- A child has the folder too, so its turn gets the block after the block of its job.
- Tools are unchanged: the agent passes the folder itself to a tool that needs one.

### 4.4 What does not change

- The model, the effort, the permission mode and the tools of a session with a folder are
  those of any session: there are no per-project settings in phase 6 (section E).
- A session made by a task, an alert investigation, a Telegram chat or the widget has no
  folder until the owner attaches one.
- The Voice view keeps its `<select>` of sessions.

### 4.5 A coder run

When phase 5 is in the tree:

- `coder_run` (`tools/coders.rs`, coders.md section 4): a call that names no `project`
  takes the folder of the calling session when that folder is a known project (the same
  folder as one of `xdg::projects`, compared after `canonicalize` on both sides). The
  tool asks `Agent::project(&ctx.session_id)` through the handle it reaches the agent
  with (`AgentAccess`, as `agent_run` does).
  Otherwise the rule of `coders.md` stands: `project` is a name from the list, never a path.
- The run's session has the folder (4.2).

When it is not: S42 does neither, and writes both rules into `coders.md` (section 2: the
child inherits the folder; section 4: the default `project`) for S35, with a note that
says they come from S42.

## 5. The UI (`ui-settings.md` section 1 rewritten, section 11 new)

### 5.1 The window (`App.tsx`)

```
<div className="app">
  <header> [toggle] KAgent [notice] ... voice chip, permission, stop all, theme, window controls
  <div className="shell">
    <Sidebar />
    <main> the view
```

- The tab row of the header goes. `nav.ts` `VIEWS` stays: the sidebar and `ui.rs` use it.
- The toggle is the first control of the header (32 x 30, as `icon-tab`; the `sidebar`
  icon; `aria-label="Toggle the sidebar"`, `aria-expanded`, `aria-controls="sidebar"`).
- Three states of the sidebar:
  - expanded: 240 px.
  - the rail: 48 px, the icons alone.
  - the drawer: below 900 px of viewport width the sidebar is never inline. The toggle
    opens it over the content, inside `.shell` (so it starts under the header):
    `position: absolute; left: 0; top: 0; bottom: 0; width: min(280px, 85vw)`, z-index
    15, with a scrim over the rest of `.shell` (a click closes). The focus moves into the
    drawer and comes back to the toggle when it closes. Escape closes it.
- At 900 px and more the sidebar is expanded or the rail as `collapsed` says (remembered);
  the toggle switches the two. Below 900 px `collapsed` is kept and applies again when
  the window grows.
- The settings screen keeps its own menu (220 px, a select under 900 px) beside the
  sidebar.
- The widget layout of KAgent's own window has no header and no sidebar: 5.8.

### 5.2 The sidebar (`components/Sidebar.tsx`), from the top

1. **New session**: a primary button, the full width (the rail: the `plus` icon, `aria-label="New session"`).
   `newSession()` (`store.ts`, which gains an optional folder): `POST /api/sessions` with
   the last model choice (S41) and no folder; the session opens, the view is `chat`, the
   drawer closes.
2. **Search**: an input `type="search"`, placeholder "Search sessions" (not in the rail).
   While it has text the list is flat: the sessions whose title or folder path contains
   it (either case), newest first, each row with its folder's name as a dim first word.
   Escape clears it.
3. **Projects | Recent**: a segmented control of two buttons (`aria-pressed`), remembered
   as `view`, `projects` by default (not in the rail).
4. The list (5.3). It scrolls; the rest of the sidebar does not.
5. **The views**, under a separator at the bottom: Voice, Tasks, Alerts, Changes,
   Security, System, Settings (the ids of `VIEWS` but `chat`), one row each: an icon of
   15 px and the label, the open one `aria-current="page"` with the ice colour at 14 %.
   Alerts carries the badge of open alerts, as the tab does today. In the rail: the icons
   alone, the label in `title`. A click sets the view and closes the drawer.

Icons: Voice `mic`, Tasks `sliders`, Alerts `bell`, Changes `box`, Security `shield`,
System `monitor`, Settings `gear`.

### 5.3 The list (`components/sidebar/SessionList.tsx`)

The pure grouping is `components/sidebar/sections.ts`, over the store's `sessions`
(newest first):

- `projectSections(sessions, closed)`: `pinned` (every pinned session, newest first),
  then one section per folder among the sessions that are not pinned, in the order of
  the newest session of each, then `machine` (no folder, not pinned). A pinned session
  is not repeated in the section of its folder.
- `recentSections(sessions, now)`: all the sessions, newest first, cut by calendar day
  of `updatedAt` (local time) into "Today", "Yesterday", "Previous 7 days", "Previous 30
  days" and "Older"; a heading only where the bucket changes.
- `searchSessions(sessions, query)`.

**Projects** view (the default):

```
Pinned
  [pin] Fix the Wi-Fi driver                2 agents
KAgent  (3)                            [+]
  Free the disk                           running
  Unit tests for the loop                 1 agent
This machine  (5)
  Why is the fan loud
```

- A section heading is a button: the `folder` icon (14 px) for a folder, the name (its
  last path component; `title` is the path), the count in `--dim`, a chevron that turns
  when the section is closed. "Pinned" and "This machine" have no icon. A closed section
  shows its heading only; the closed folders are remembered (`closed`), "This machine" is
  never closed.
- The heading of a folder has a `+` button on hover and on focus (`aria-label="New
  session in this project"`): `newSession(path)`, so one `POST /api/sessions` with
  `project` and the last model choice; the session opens.
- The section "Pinned" is drawn only when a session is pinned.

**Recent** view: the date headings, then the rows, each with its folder's name as a dim
first word when it has one.

**A row** (`components/sidebar/SessionRow.tsx`): 32 px high, padding `0 6px 0 10px`,
radius `--radius-s`, font 13 px; the title on one line with an ellipsis ("New session"
while it has none); a pin of 12 px in `--ice` before the title when pinned; at the
right, in `--dim`: the count of agents as today ("2 agents") and a pulsing dot while a
turn runs in the session (`store.running`). Hover: white at 3 %; the open session: the
look of `.session-item.active` today (ice at 10 %, the bar of 2 px). A click opens the
session in `chat`. Keyboard: the row is a button; Up and Down move the focus among the
rows and the headings of the open sections, Enter and Space open; the `+` of a heading
and the `⋯` of a row come after it in the tab order.

**The row menu**: a `⋯` button (24 x 24, the `more` icon, `aria-label="More"`) shows on
hover, on focus and on the open row. It and a right click on the row open the same menu
(`components/Menu.tsx`: a list of actions in a portal, `position: fixed` at the anchor,
the house style, z-index 20; Up, Down, Enter, Escape (back to the `⋯`), Tab closes, a
  click outside closes, letters jump):

- Pin, or Unpin when it is pinned.
- Attach folder..., or Change folder... when it has one.
- Detach folder, when it has one.
- a separator, then Delete in `--bad` (as the delete of the list does today: no confirmation).

### 5.4 Attaching a folder (`components/sidebar/AttachMenu.tsx`)

"Attach folder...", "Change folder..." open a popup in the house style at the place of
the menu:

- An input at the top, placeholder "Type a path, or pick a project". Enter sends what was
  typed as a path.
- Under it the projects of `GET /api/projects` (read each time it opens): the `folder`
  icon, the name, the path in `--dim`, a tag "AGENTS.md" where `agentsMd` is true, a
  check when the session has that folder. The list keeps the projects whose name or path
  contains the text.
- With no project at all: "No projects under the project folders" and a row "Project
  folders in Settings" that opens the `desktop` section.
- A pick, or Enter: `PUT /api/sessions/:id/project {path}`. The answer replaces the
  session's meta and its row; the popup closes. A refusal shows its message under the
  input in `--bad` and the popup stays.

### 5.5 `/project` in the composer

`BUILTIN_COMMANDS` (`commands.ts`) gains `{ name: "project", description: "Attach a folder to this conversation; alone, it detaches" }`.
`sendMessage` (`store.ts`) does it before anything is sent, as `/compact`:

- `/project` alone: `PUT .../project {path: null}`; the notice says "Project detached".
- `/project <path>`: `PUT .../project {path}`; the notice says "Project: <the path the daemon answers>".
- A refusal is a notice with the daemon's message. The session of a subagent is read-only:
  the notice of `sendMessage` says so. The UI never canonicalizes the path: the daemon does.

### 5.6 The store

- `api.ts` is at its budget: the session types and their helpers (`SessionMeta`, `TodoItem`,
  `Session`, `CompactResult`, `putTodo`, `postPlan`, `requestCompaction`) move to
  `web/src/sessionApi.ts`, and `api.ts` re-exports them (`export * from "./sessionApi";`),
  so no importer changes. `SessionMeta` gains `project?: string | null` and `pinned?: boolean`.
- `store.ts` exports its `set` as `patch`, and `newSession(project?: string)` takes a
  folder. The rest is in `web/src/components/sidebar/store.ts` (new): `setProject(id, path)`,
  `setPinned(id, pinned)`, `refreshProjects()`, and the state of the sidebar
  (`sidebarCollapsed`, `sidebarDrawer`), written through `patch` and read through
  `useStore` like the rest.
- `handleEvent` already applies `sessionMetaChanged` (S41): the row moves at once in
  every open window.
- A folder that no longer exists stays listed with its sessions: the owner detaches them.

### 5.7 What the daemon is told

- `diag.ts`: `uiState()` gains `sidebar` ("expanded", "rail", "drawer" or "hidden": under
  900 px with the drawer closed) and `project` of the open session.
- `ui show` and `ui.rs` do not change.

### 5.8 The widget (`views/Overlay.tsx`)

A `wicon` button with the `sidebar` icon (`aria-label="Sessions"`) in `.widget-top`,
before the hide button, opens the sheet "Sessions" (`Sheet` of S41; `sheet` is `"sessions"`):

- "New session", the search, "Projects | Recent", the list (5.3, the same components), then
  the seven views.
- A session opens and the sheet closes. A view expands the window into the full layout
  (`onExpand`) and then opens that view. The row menus work in the sheet.

### 5.9 The rules (`components/sidebar.css`)

The tokens are those of `index.css`, which is not rewritten (the rules of `.sessions`
stay there, unused). `.shell { flex: 1; display: flex; position: relative; min-height: 0; }`;
`main` as it is. `aside.sidebar.inline` (240 px, `var(--panel)`, the rim on its right, a
column of: `.sb-top`, the search, the switch, `.sb-list` that scrolls, `.sb-nav`) and its
modifier `.rail` (48 px); `aside.sidebar.drawer` and `.sidebar-scrim`, both absolute in
`.shell`. The component renders the drawer only under 900 px and the inline sidebar only
at 900 px and more (it reads `matchMedia("(max-width: 899px)")`), so no rule hides
either. `.sb-section` (a heading button), `.sb-row` (`.active`, `.running`, the pin, the
count, `.sb-more`); `.sb-nav-item` (`.active`, `.sb-badge`).

## 6. Files

```
web/src/
  components/Sidebar.tsx             5.2, the frame and the views: < 220 lines
  components/sidebar/SessionList.tsx 5.3: < 200 lines
  components/sidebar/SessionRow.tsx  a row and its menu: < 120 lines
  components/sidebar/sections.ts     the grouping and the search: < 130 lines
  components/sidebar/AttachMenu.tsx  5.4: < 130 lines
  components/sidebar/store.ts        5.6: < 130 lines
  components/sidebar.css             5.9: < 260 lines
  components/Menu.tsx                the action menu: < 110 lines
  components/Icon.tsx                sidebar, folder, pin, more, search
  sessionApi.ts (new)                the session types leave api.ts
  commands.ts                        /project
  store.ts                           exports `patch`; `newSession(project?)`; `sendMessage` does `/project`: < 400 lines
  App.tsx                            5.1, no tab row
  views/Chat.tsx                     no session list
  views/Overlay.tsx                  5.8
  diag.ts                            5.7
  nav.ts, settings/SettingsPage.tsx  unchanged

crates/kagent-core/src/
  session.rs               the two fields and their test: to ~325 lines
  project.rs (new)         agents_md, block, set_project, set_pinned, project, project_block; tests: < 230 lines
  agent/mod.rs             `mod project;`, one line in `turn_loop`
  agent/children/run.rs    the child inherits the folder (start_child)
  agent/children/external.rs   the same in start_coder (phase 5)
  tools/coders.rs          the default project (phase 5)
  tools/xdg.rs             unchanged

crates/kagentd/src/
  project_api.rs (new)     section 3 and its tests: < 240 lines
  api.rs                   `list_sessions` and `create_session`
  subagents/mod.rs         `listed` with the folder filter
  server.rs                the routes
```

## 7. Scenarios

`scenarios.md` has no S42 block at the time this spec is written; the stage agent copies
this list there as "S42 left sidebar and projects".

1. **[auto]** `project_api.rs`: each refusal of section 3 with its message (the shape of
   the body, a relative path, a folder that is not there, KAgent's own folder, both
   filters, the 404, the two 409); `~` is expanded and the canonical path is stored;
   detach; pin and unpin; the event when something changed and none when nothing did;
   `GET /api/projects` (the markers, `agentsMd`, the roots); the list of a folder.
2. **[auto]** `project.rs`: the block with and without `AGENTS.md`; the cut at 4000
   characters; a link to a credential file gives none; a missing file and one that is not
   a regular file give none; a session without a folder has no block.
3. **[auto]** Scripted model: every request of a session with a folder carries the block
   and a session without one does not; a child of a session with a folder has the folder
   in its meta and the block in its prompt; a folder attached while a turn runs is in the
   prompt of the next turn only. With phase 5: `coder_run` without `project` in a session
   attached to a known project runs there, and in any other folder answers the error of
   `coders.md`.
4. **[auto]** The fields round-trip under `project` and `pinned`; a meta from before S42
   loads with `None` and `false`.
5. Headless Chrome (dark and light, 1200x820): the header has no tab row; the sidebar
   lists the seven views and a click opens one; "Projects | Recent" switches; the pinned
   on top, a group per folder, "This machine" last; the toggle gives the rail; the state,
   the view and the closed folders survive a reload.
6. Headless at 700x500: the sidebar is a drawer; the toggle, Escape and a click on the
   scrim close it; the focus comes back to the toggle.
7. Headless: "Attach folder" lists the projects and attaches one with one PUT (its group
   appears); a typed path that is not a folder shows the daemon's message and stays;
   "Detach folder" and `/project` alone detach; `/project <path>` attaches; "New session
   in this project" sends one POST with `project`; pin and unpin move the row; the search
   finds a title and a folder.
8. Headless, in the widget (340x460): the sessions sheet opens; a session opens from it
   and the sheet closes; a view expands the window.
9. The owner, live: attach a folder that has an `AGENTS.md` and ask "what are the rules of
   this project": the answer quotes them. With phase 5: "use Claude Code to add a test
   here" starts the run in that folder.

## 8. ZCode references

ZCode is at `/home/virus/SystemInterface/ZCode/packages/`; the paths are under `ui/src/`.
Take the pattern, not the code: KAgent has no dnd-kit and no remote hosts.

| File | Take |
|---|---|
| `project-sidebar/ProjectSidebarNavigation.tsx` | the whole file (62 lines): the navigation under the "new" button, one row per view |
| `project-sidebar/ProjectSidebarListHeader.tsx` | lines 17 to 99: the search and the view switch at the head of the list |
| `project-sidebar/view.ts` | `selectProjectRows` (lines 84 to 101): the pinned rows first, and a project's rows without its pinned ones |
| `project-sidebar/view-preference.ts` | the view the client remembers, `projects` by default (19 lines) |
| `project-sidebar/timelineDates.ts` | lines 24 to 57: date headings put into a list that is already in order, by local calendar day |
| `project-sidebar/ProjectTaskRows.tsx` | lines 137 to 149: the section "Pinned" over the list |
| `project-sidebar/ProjectNodeContent.tsx` | the project row: the actions show on hover (line 313), Pin and Unpin in one menu (lines 145 to 150), the pin mark (lines 239 to 253) |
| `project-sidebar/useProjectSidebarPreferences.ts` | what one client remembers (the view, the closed rows), in localStorage under one key (lines 15 to 135) |
| `app-shell/WorkspaceShellLayout.tsx` | lines 126 to 140: a fixed width of the sidebar, kept in localStorage; its collapsing |

## 9. Tests

- `crates/kagentd/src/project_api.rs`: every refusal of section 3 with its status, code
  and message; the canonical path of `~` and of a link; detach; pin and unpin; no event
  when nothing changed; `projects()` over a temporary root with and without the markers;
  `in_project`. (Scenario 1.)
- `crates/kagent-core/src/project.rs`: `agents_md` and `block` on temporary folders (a
  file, none, a link to a file that `protect::is_credential` names, a folder named
  `AGENTS.md`, a file of 5000 characters); `set_project` and `set_pinned` on a temporary
  store: the file, the event, a left-out change. (Scenario 2.)
- `crates/kagent-core/src/session.rs`: the fields round-trip; the old meta of the existing
  test loads with `None` and `false`. (Scenario 4.)
- `crates/kagent-core/tests/agent_loop.rs`: the block in the system prompt of every
  request, none without a folder, a change between two turns. (Scenario 3.)
- `crates/kagent-core/tests/subagents/main.rs`: a child has the folder of its parent and
  the block in its prompt; the same in `tests/subagents/coder.rs` for a coder run and the
  default `project` of `coder_run` (phase 5: `coders.md` section 11 names that file).
  (Scenario 3.)
- `crates/kagentd/src/subagents/mod.rs`: `listed` with and without a folder.
- The headless pass on an own dev server with stubbed routes: scenarios 5, 6, 7 and 8.

Scenarios: `scenarios.md` S42.

## 10. Documents of the same stage

- `api.md` section 8: the five rows of section 3.
- `config.md` section 12: the two session fields.
- `ui-settings.md`: section 1 rewritten (the sidebar is the navigation; the header has no
  tabs; `VIEWS` unchanged) and section 11 (new): sections 5 of this spec.
- `coders.md` section 2 (the child inherits the folder) and section 4 (the default
  `project` of `coder_run`).
- `scenarios.md`: S42 (section 7).

## Amendment 2026-11-27: the header becomes a title bar (owner's decision)

Milestone v0.0.2, item 5. This overrides the header layout of section 5 where they
differ.

- The header keeps only: the sidebar toggle, "KAgent", Stop all, the voice chip (and
  the screen watch chip of `screen-watch.md`), and the window controls. Its height
  drops to 36 px; it stays the drag area of the frameless window.
- The permission picker and the theme switch move to the foot of the sidebar, above
  the Settings entry: the permission picker as a full-width row (its dot, the mode's
  name, the same menu, the same `PUT /api/settings`), the theme as an icon button
  beside it (light, dark, auto). In the collapsed rail: the permission dot (its
  `title` is the mode; a click opens the menu to the right) and the theme icon.
- The notice line of the header goes (`notices.md`: toasts).
- The widget layout (340 px) is unchanged: it has no header and no sidebar; the
  permission picker stays in its sheet.
- Headless check, added to the S42 scenarios: the header holds exactly those five
  things; the permission mode changes from the sidebar foot in the full and the
  collapsed state; the keyboard order is toggle, Stop all, voice chip, then the sidebar.

### Amendment 2026-11-28 (v0.0.3): the Tickets entry

The views under the list are now eight: Voice, Tasks, **Tickets**, Alerts, Changes, Security, System, Settings (`nav.ts` `VIEWS`,
the daemon's `ui.rs` allow-list, the icon `list`). The view is `tickets.md` section 6. `scripts/test-sidebar-web.py` expects the
eight and, since the list is one entry shorter, scrolls the row it right-clicks into view.

### Amendment 2026-11-28 (v0.0.3): the window controls come last

In the app window the window controls (Shrink to widget, Hide to tray, Maximize) come last in
the keyboard tab order, after the sidebar and the view. They are no longer children of the
header in the DOM: `.win-controls` is a later child of `.app`, drawn at the right end of the
title bar (`position: absolute`, 36 px high; the header keeps room for it; the strip between
the buttons still drags and a double click on it still maximizes). The header holds the
toggle, KAgent, Stop all and the voice chip. Check: `scripts/test-sidebar-web.py`.

### Amendment 2026-11-28 (v0.0.3): the Runs entry

The views under the list are now nine: Voice, Tasks, Tickets, **Runs**, Alerts, Changes, Security, System, Settings (`nav.ts` `VIEWS`, the
daemon's `ui.rs` allow-list, the icon `bot`). The view is the board of `workflows.md` section 8. It is called **Runs**, not Work: "Work" is
the work panel and the widget's sheet of `work-windows.md`. `scripts/test-sidebar-web.py` expects the nine.

### Amendment 2026-11-28 (v0.0.3): the shell (`shell.md`)

- **Back and Forward** are two icon buttons in the title bar after the sidebar toggle (`aria-label` Back and Forward, disabled with
  nowhere to go). The header holds the toggle, the two buttons, KAgent, Stop all and the voice chip, so the keyboard order is the toggle,
  Back and Forward (when enabled), Stop all, the voice chip, then the sidebar.
- **The row menu** (section 5.3) is now: Rename (F2), Pin, Move to project..., Attach folder... (or Change folder...), Detach folder,
  Archive (Unarchive in the Archived group), Delete (Del). It opens on the `...` button, on a right click, on the menu key and on
  Shift+F10. The first four of S42 are unchanged; "Move to project..." opens a second menu of the known projects (the projects of
  `GET /api/projects` and the folders the sessions use) with a check on the current one and "No project", and "Attach folder..." stays the
  way to type a path. The menu takes `useOverlay`: Back closes it.
- **Rename** edits the title in the row (Enter or a blur saves, Escape cancels); **drag** a row onto a project's section (or "This machine")
  in the Projects view to move it (the section is lit while the row is over it); **archive** puts the row in a collapsed "Archived (n)" group
  at the foot of the list (Projects and Recent alike); the search leaves archived rows out unless its "Archived" filter button is pressed;
  **delete** hides the row, shows an Undo toast for 10 s, and the daemon deletes when the toast ends (`shell.md` section 5).
- The widget's sessions sheet draws the same list, so it has the same menu.
