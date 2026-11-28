# Spec: the shell - a router, back and forward, pages that resize, conversation actions (v0.0.3, H1 to H4)

Milestone: `docs/milestones/v0.0.3.md`. Stages H1 (the matrix), H2 (the router), H3 (layout and shared
parts), H4 (conversations). Owner's words: "in fullscreen some settings pages get wrecked: go over all
pages and make them consistent and resizable; introduce a router and forward/backward buttons that
behave like a browser; support the side mouse buttons"; "move sessions to a project, delete or archive
them". Read with `sidebar.md` (the sidebar and the title bar), `ui-settings.md` (the settings pages),
`notices.md` (toasts) and `api.md` (section 8, sessions).

## 1. What there was

- No router: the view, the settings page and the open conversation lived in the store; the URL hash
  (`#token=..&session=..`, `#view=settings&section=models`) was read once at load and never written. The
  browser's Back did nothing useful.
- Settings pages built one stage at a time, each with its own widths (860 px caps, a two-pane grid on Models,
  fixed-size dialogs); native `<select>` elements in 15 places next to the themed `Dropdown`.
- The sidebar row had a menu (Pin, Attach or Change folder, Detach, Delete) from S42. Delete asked nothing and
  could not be undone. There was no rename, no archive and no way to drag a row.

## 2. H2: the router (`web/src/router/`)

**Paths, not state.** The History API is enough: no router dependency (`web/package.json` has none and gets none).
The daemon already serves `index.html` for a path that is not a file; `/api/...` paths that match no route answer
404 JSON, never the page (section 7).

| Address | Place |
|---|---|
| `/` , `/chat` | the chat, the open conversation (a plain `/chat` is filled in to `/chat/<id>` once one is open) |
| `/chat/<session>` | that conversation |
| `/voice`, `/tasks`, `/alerts`, `/changes`, `/security`, `/system` | the view |
| `/tickets`, `/tickets/<KEY>` | the tickets view, with the ticket open in its pane |
| `/runs`, `/runs/<run>`, `/runs/<run>/<task>` | the runs view; a run; a task of it with its worker's tab shown |
| `/settings`, `/settings/<section>`, `/settings/<section>/<item>` | the settings page (a plain `/settings` is filled in with the page used last); `<item>` is a provider, a skill... opened in the page |

- A **place** is `{view, section, id, sub, session}`; `route.ts` turns a path into a place and back (pure, tested by
  `scripts/test-shell-web.py` through the window and by the build). An address that is not in the table is replaced
  (`replaceState`) by the chat. `?app`, `?compact` and any other query string are kept on every address.
- The store stays the owner of the state (`view`, `settingsSection`, `activeSession`, `placeId`, `placeSub`);
  `router.ts` mirrors it into the address. A store change that makes a different place **pushes** an entry; a
  change that only fills an empty part of the place (the first conversation opened at load, the default settings
  page, a conversation opened after the last one was deleted) **replaces**. Not places, so no entry: tabs inside a
  card, a filter, a dialog, a menu, the sidebar collapsing.
- **The token.** `captureToken` runs first and is unchanged in what it does: the token in `#token=` goes to
  `localStorage` and the fragment is removed with `replaceState` before any route is read, so the token is never in
  an address the router writes and never in the history. The old fragments `#view=..&section=..&session=..` (the
  KRunner link and `ui show` use `session`) are read once, turned into a place, and replaced by its path.
- `ui show`, a toast's "Open conversation" and a task's "open the last session" go through the store, so they are
  history entries like the owner's own clicks.

### 2.1 Back and Forward

- Two buttons in the title bar after the sidebar toggle: Back and Forward (`aria-label`, disabled when there is
  nowhere to go, `title` "Back to Settings: Models"). The widget layout has no title bar and no buttons; the keys
  and the mouse buttons work there too.
- `Alt+Left` and `Alt+Right` (not while a text field has the Alt+arrow of its own: they always win; text fields
  have no Alt+arrow). The mouse's side buttons: `mouseup` with `button` 3 (Back) and 4 (Forward) with
  `preventDefault`, and `mousedown`/`auxclick` of those buttons prevented too, so the browser does not also
  navigate.
- The app keeps a mirror of the stack (`{idx, entries: [{path, label, scroll}]}` in `sessionStorage`, restored on
  reload) because the History API cannot say whether Forward is possible. `history.state` carries the index of
  its entry.
- **Overlays close first.** A dialog, a menu, a popup or a sheet registers itself (`useOverlay(open, close)`). Back
  (button, key or mouse button) closes the topmost one and does not change the place. This is done by the app's own
  Back, not by a history entry per overlay: an entry per open menu would cost an entry per menu opened and race with
  a pick that navigates. A browser's own Back (outside the app window) closes every overlay and then changes the place.
- **Scroll** of a page is kept per entry: the element marked `data-scroll-root="page"` (the page frame, section 4)
  is recorded on scroll and put back after a Back or Forward once its content is tall enough (tried for 2 s).

## 3. H1: the matrix (`scripts/shell-matrix.py`)

On the shared test library (`picker_web_lib.py`): every view and every settings page at 1920x1080, 1200x820, 700x500
and 340x460 (the widget size, only for what the widget shows: the widget is its own layout, `Overlay.tsx`, so the
cell is the widget itself), dark and light. Screenshots go to `/tmp/kagent-v003-shell-shots/<run>/`. Measured per
page and size, as findings:

| Finding | Meaning |
|---|---|
| `h-overflow` | the document is wider than the viewport (`scrollWidth > clientWidth`) |
| `wide-child` | an element wider than the viewport, or wider than its scroll-less parent, outside a scroller made for it |
| `main-width` | the page's main column leaves more than 40% of a window of 1200 px or more empty, or is wider than 1600 px (the plan's caps), outside the two-pane pages |
| `cut-off` | an element whose text or box is clipped (`overflow: hidden` with `scrollWidth > clientWidth`) without an ellipsis, or a control (button, input, row) partly outside the viewport |
| `nested-scroll` | a scroll container inside a scroll container, both scrolling |
| `native-select` | a native `<select>` |

The result is the table in section 8 (written by the first run, before any product change). `--out after` writes
`after/`; the run is a test in `scripts/test-shell-web.py` and fails on any finding.

## 4. H3: layout rules and shared parts

Rules, written once (CSS in `web/src/styles/shell.css`, parts in `web/src/components/page/`):

1. **One page frame** (`.page-frame`): fills the area beside the sidebar, scrolls on its own (`overflow: auto`,
   `data-scroll-root="page"`), is the only scroller of a page, and holds the content in a column. The column is
   `min(100%, 960px)` for forms and reading (`.page-frame` default), `min(100%, 1600px)` for pages with a real
   second pane (`.wide`: Models, Skills, Runs, Tickets, Changes). Below the cap the column is the whole width; above
   it, it is centered. Nothing has a fixed pixel width except icons.
2. **Two panes** split at a divider (kept per page where one exists today) and stack under 900 px.
3. **One page header** (`PageHeader`): the title (one heading size per level: h1 for a page, h2 for a card), a
   one-line description, and the page's actions on the right, wrapping under the title when narrow.
4. **One form row** (`FormRow`): label and hint left, control right at 720 px and above; stacked below. **One card**
   (`.card`): a bordered, padded block with an optional h2. Existing `settings/ui.tsx` parts (`Section`, `Row`, `Toggle`)
   are the base: they are changed to use the frame and the row rules above, not duplicated.
5. **Tables and code scroll inside their card** (`.scroll-x`); the page never scrolls sideways. Long words wrap
   (`overflow-wrap: anywhere`) or end in an ellipsis with a `title`.
6. **Dialogs**: `min(92vw, 720px)` wide, `max-height: 86vh` with their own scroll (the dialog is the one scroller
   while it is open).
7. **No native `<select>`**: the existing `Dropdown` (`settings/Dropdown.tsx`) in its place, with its keyboard
   behaviour. The mobile settings jump (`.settings-jump`) is a `Dropdown` too.
8. No nested scroll containers on a page: a list inside a page frame grows with the page; a pane that must scroll on
   its own (a transcript, a diff) is a page of its own (the frame is `overflow: hidden` there, `.fill`).

No visual redesign beyond what these rules need, and no change to what a control does. The order the pages are
fixed in is the order of the table in section 8.

## 5. H4: conversations

**What was there** (S42): the row menu with Pin, Attach or Change folder, Detach folder and Delete (no confirm, no
undo), `PUT /api/sessions/:id/project` and `/pin`, `DELETE /api/sessions/:id` (cancels a running turn and deletes the
subagents; its images folder goes with it, `session.rs` `delete`). Extended, not duplicated: `SessionRow.tsx` and
`sidebar/store.ts`.

- **Menu** on a row (the `...` button, a right click, the menu key and `Shift+F10`): Rename, Move to project (a
  submenu: the projects of `GET /api/projects` and the folders conversations already use, a check on the current one,
  and "No project"), Pin, Archive or Unarchive, Delete. `F2` renames and `Delete` deletes the focused row.
- **Rename**: the row's title becomes a text field (Enter saves, Escape cancels, a blur saves); 1 to 120 characters.
  `PUT /api/sessions/:id/title {title}`; the title the owner chose is never replaced by the automatic one (the
  automatic title only replaces the placeholder).
- **Move**: the existing `PUT /api/sessions/:id/project {path}`; "No project" is `{path: null}`. A conversation with
  history gets an info toast "The next turn works in <folder>". **Drag** a row onto a project heading (or "This
  machine" for none) moves it with the same call; the heading is highlighted while a row is over it. The keyboard
  way is the submenu.
- **Archive**: `archived: boolean` in the session's meta (`#[serde(default)]`, so old sessions are not archived);
  `PUT /api/sessions/:id/archive {archived}` answers the meta and sends `sessionMetaChanged`; 409 while a turn runs
  ("the conversation is running: stop it first"). An archived conversation leaves the list and stands in a collapsed
  "Archived" group at the foot of the list (its rows have Unarchive and Delete). The search leaves archived ones out
  unless its "Archived" filter button is pressed. An archived conversation can still be opened.
- **Delete with undo**: no confirm. The row is hidden at once and a toast says "Deleted <title>" with **Undo** for 10 s.
  When the toast ends (the 10 s, or the owner closing it) the page calls `DELETE /api/sessions/:id`; Undo cancels
  that and shows the row again. If the open conversation is deleted, the next one opens (a replace, section 2). The
  daemon refuses a running conversation with 409 `conflict` ("the conversation is running: stop it first") for the
  archive and for this delete; the row comes back and the reason is a notice. The daemon deletes the log, the meta, the
  images folder and the subagent conversations (agents.md section 2).
- **Closing the window during the 10 s** (decision): the delete is **done**, not lost. A `pagehide` sends the pending
  `DELETE`s at once with `fetch(..., {keepalive: true})`; the owner asked for the delete, and undo only exists in the
  window that showed the toast. If that request does not arrive (a crash), nothing was changed: the conversation is
  simply still there at the next start. Nothing is ever half deleted (the daemon deletes in one call).
- Not built: multi-select with Ctrl and Shift, the `sessions` tool's `archive` and `move` actions, a trash folder on disk
  (the undo window is the toast; the plan's 30 day trash is dropped).

## 6. API (also in `api.md` section 8)

| Route | Body | Answer |
|---|---|---|
| `PUT /api/sessions/:id/title` | `{title: string}` | `SessionMeta`; 400 for an empty title or over 120 characters, 404, 409 for a subagent's session |
| `PUT /api/sessions/:id/archive` | `{archived: boolean}` | `SessionMeta` (`archived`); 404, 409 for a subagent's session or a running conversation (archiving only) |
| `DELETE /api/sessions/:id` | | 204; 404; 409 `conflict` while a turn runs (it was 204 and a cancel before) |

`GET /api/sessions` lists archived sessions too, with `archived: true`; the window groups them.

## 7. The daemon's page rules

`/settings/models`, `/tickets/KA-1` and any other path that is not a file answer 200 with `index.html` and
`Cache-Control: no-cache` (`server.rs`, `page_cache_control`); a path under `/api/` that matches no route answers 404
`{"error": {"code": "not_found", ...}}` (the API router has its own fallback), never the page.

## 8. What breaks where

First run of `scripts/shell-matrix.py` (before any product change), dark and light, 28 cells per theme and size:
**276 findings**: 1920x1080 74, 1200x820 74, 700x500 124, 340x460 (the widget) 4. By kind: `native-select` 102,
`main-width` 112, `wide-child` 30, `cut-off` 26, `nested-scroll` 6, `h-overflow` 0. The table is ordered by findings
(both themes counted): this is the order H3 fixes the pages in; the shared causes come first because they clear most rows.

| # | Page | Findings | What and where |
|---|---|---|---|
| 1 | voice | 42 | `cut-off` 700x500; `main-width` 1920x1080; `native-select` 1920x1080, 1200x820, 700x500; `wide-child` 1920x1080, 1200x820, 700x500 |
| 2 | settings-general | 20 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 1920x1080, 1200x820, 700x500 |
| 3 | chat | 18 | `nested-scroll` 1920x1080, 1200x820, 700x500; `wide-child` 1920x1080, 1200x820, 700x500 |
| 4 | settings-models | 16 | `main-width` 1200x820, 700x500; `native-select` 700x500; `wide-child` 1200x820 |
| 5 | tasks | 14 | `main-width` 1920x1080; `native-select` 1920x1080, 1200x820, 700x500 |
| 6 | settings-voice | 14 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 1920x1080, 1200x820, 700x500 |
| 7 | settings-desktop | 14 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 1920x1080, 1200x820, 700x500 |
| 8 | settings-tasks | 14 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 1920x1080, 1200x820, 700x500 |
| 9 | settings-skills | 12 | `main-width` 1200x820, 700x500; `native-select` 1920x1080, 1200x820, 700x500 |
| 10 | settings-memory | 12 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 1920x1080, 1200x820, 700x500 |
| 11 | tickets | 8 | `main-width` 1920x1080; `native-select` 1920x1080, 1200x820, 700x500 |
| 12 | settings-appearance | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 13 | settings-local-models | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 14 | settings-browser | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 15 | settings-watchers | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 16 | settings-web | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 17 | settings-tools | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 18 | settings-agents | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 19 | settings-calendar | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 20 | settings-telegram | 8 | `main-width` 1920x1080, 1200x820, 700x500; `native-select` 700x500 |
| 21 | settings-hooks | 6 | `main-width` 1200x820, 700x500; `native-select` 700x500 |
| 22 | runs | 2 | `main-width` 1920x1080 |
| 23 | alerts | 2 | `main-width` 1920x1080 |
| 24 | changes | 2 | `main-width` 1920x1080 |
| 25 | security | 2 | `main-width` 1920x1080 |
| 26 | system | 2 | `main-width` 1920x1080 |
| 27 | widget | 2 | `native-select` 340x460 |
| 28 | widget-sheet | 2 | `native-select` 340x460 |

Causes, in the order they are fixed:

1. **`main-width` on every settings page** (112): the page header (`.ui-section-head-inner`, a flex row with `margin: 0 auto`)
   shrinks to its content, so the title sits at a different left edge than the cards under it (General at 1920: title at
   985 px, cards at 760 px), and at 1920 the 900 px column is not what the rows below use. Views without a frame (Alerts,
   Changes, Runs, Security, System, Tasks) lay their content out from the left edge: 340 to 460 px of a 1680 px area, or a
   column that is not centered (Changes: 320 px left, 663 px right; Runs: 10 left, 312 right).
2. **`native-select`** (102): 15 selects on General, Voice, Desktop, Memory, Skills, Tasks, Tickets, Voice (view) and the
   settings jump at the top of every page below 900 px; the widget has two (its model and its voice).
3. **Voice view** (42): the live transcript lets a long unbroken word spill out of its box at every size, and at 700 px the
   `.voice-stage` is 2006 px wide and clipped by `main`.
4. **Chat** (18): a user message with a long unbroken string spills out of its bubble (2071 px of text in a 638 px box); the
   code block of a reply is a horizontal scroller inside the scrolling transcript (`nested-scroll`).
5. **Models** (16): at 1200 px the provider detail's section is 765 px in a 740 px box; the two panes do not use the wide cap.
6. **Tickets** (8): at 1920 the content is 1640 px, over the 1600 px cap.
7. The screenshots of each cell are in `/tmp/kagent-v003-shell-shots/before/` and the raw findings in
   `/tmp/kagent-v003-shell-before.json` (not kept in the repo).

The widget cell is the widget itself (`Overlay.tsx`) plus its sessions sheet; the pages are measured in the full layout (the
drawer sidebar below 900 px). Data is the mock of `picker_web_lib.py` with a few conversations, tickets, runs and changes
(`shell_matrix_lib.py data()`), so a page that needs the daemon's own data (Security, System) is measured with the
daemon stand-in's empty answers.

## 9. Tests

- Rust: the title and archive calls (refusals, events, old metas), the 409 of a running delete, the page and the
  `/api` 404 over a real listener.
- `scripts/test-shell-web.py`, dark and light: each address opens its place, reload and a pasted address, the old
  `#view` links, the token never in the address or in `history`, Back and Forward by button, by `Alt+Arrow` and by
  mouse buttons 3 and 4, an open dialog closes first, scroll is restored; the matrix reports zero findings; rename,
  move (menu and drag), archive (group, search filter), delete and undo, delete after 10 s, delete on `pagehide`.
- The other web suites are run as before; the places where the router or the layout rules changed what they test are
  listed in `IMPLEMENTATION.md` section E.
