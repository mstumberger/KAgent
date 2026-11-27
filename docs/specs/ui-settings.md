# Spec: settings screen and navigation (S19)

Settings live in five places today (the Settings, Tools, Providers and Voice views and
the header), with three ways of saving and no grouping. From S19 on there is one
settings screen laid out like ZCode's: a left menu of sections in three groups, one
section on the right. The new capabilities (skills, memory, subagents, hooks, Telegram)
become sections there, never new main tabs.

## 1. Main navigation

- Header tabs, in order: `chat`, `voice`, `tasks`, `alerts`, `changes`, `security`,
  `system`, `settings`. Text labels as today; `settings` also gets a gear icon
  (`Icon.tsx`).
- `providers` and `tools` stop being tabs. `#view=providers`, `#view=tools` and
  `ui show {view: "providers"}` / `{view: "tools"}` open `settings` at the sections
  `models` and `tools`.
- The tab list is defined once, in `web/src/nav.ts` (`VIEWS`), and used by `App.tsx`
  and `diag.ts`. The daemon's allow-list (`ui.rs`, `VIEWS`) holds the same ids.
- The widget layout (`views/Overlay.tsx`) is unchanged.

## 2. Settings screen (`web/src/settings/SettingsPage.tsx`)

- Two panes: left `nav.settings-nav` (220 px), right `section.settings-detail`. Below
  900 px of viewport width the menu is a `<select>` at the top of the page.
- Menu: three groups with a small heading each; items have an icon and a label; the
  active item gets `aria-current="page"`. The menu does not collapse (as in ZCode).
- Content: a `SectionPage` with a header (title, one-line description, actions on the
  right) and the section body. Max width 860 px; the page scrolls.
- Deep link: `#view=settings&section=<id>`. A missing or unknown `section` opens the
  last one used (localStorage `kagent.settings.section`), else `general`.
- `ui show {view: "settings", section: "<id>"}` opens a section. The daemon checks
  `section` against its own list (`ui.rs`, `SECTIONS`) and drops an unknown one.
- `diag.ts` reports the open `section` in the UI state it posts to `/api/ui/report`.

## 3. Sections

| Group | id | Title | Content | Stage |
|---|---|---|---|---|
| Basics | `general` | General | Persona (name, address, language); startup (start with session, greeting); start as widget (`startWidget`); root helper status and privilege method | S19 |
| Basics | `appearance` | Appearance | Theme: auto (follows the system's light or dark setting; the default when the browser has no choice stored), dark or light. The header toggle stays and picks dark or light. The page carries the theme in use as `data-theme` and the choice as `data-theme-pref` (`web/src/theme.ts`; v0.0.2) | S19 |
| Basics | `models` | Models | The Providers page as it is (provider list, catalog, models, roles) plus "Roles" (fallback, vision, fast) and "Advanced" (reasoning level, max output tokens, temperature, max iterations, approval timeout) | S19 |
| Basics | `voice` | Voice | Live model, voice, pause media; brain and context from S26 | S19 |
| Basics | `browser` | Browser | Backend, port, binary; attach controls from S27 | S19 |
| Basics | `desktop` | Desktop and apps | Editor, project folders | S19 |
| Basics | `watchers` | Watchers | Quiet hours | S19 |
| Basics | `web` | Web | Search engine, Brave key, SearXNG URL (`web.md`) | S30 |
| Agent capabilities | `skills` | Skills | `skills.md` section 9 | S20 |
| Agent capabilities | `memory` | Memory | `memory.md` section 7 | S25 |
| Agent capabilities | `agents` | Subagents | Profiles and concurrency (`agents.md`) | S31 |
| Agent capabilities | `tools` | Tools and permissions | Permission mode, always-allow rules, per-tool overrides, tool groups for chat and for MCP, the contracts table (today's Tools view) | S19 |
| Agent capabilities | `tasks` | Automations | The defaults of `settings.tasks`: approval policy, delivery, time limit (`timeoutMin`), runs at once (`maxConcurrent`) | v0.0.2 |
| Agent capabilities | `hooks` | Hooks | `hooks.md` section 7 | S32 |
| Agent capabilities | `calendar` | Calendar | `calendar.md` section 2: four cards (Connection, Calendars, Places and travel, Morning report), after Automations | S45 |
| Channels | `telegram` | Telegram | `bots.md` section 7 | S29 |

- The tasks themselves are in the Tasks tab (`automations.md`); the `tasks` section holds
  only the defaults of `settings.tasks` (decision of 2026-11-27).
- The registry is `web/src/settings/registry.ts`:
  `SECTIONS: {id, title, group, description, keywords, component}[]`. The menu, the
  deep-link check and the render switch all come from it. A new section is one file
  under `sections/` plus one entry.
- A section that a later stage completes shows a one-line "comes with stage SN" note
  until then. No empty placeholders.

## 4. Shared components (`web/src/settings/ui.tsx`)

| Component | Props | Use |
|---|---|---|
| `SectionPage` | `title, description, actions?, children` | the frame of every section |
| `GroupCard` | `title?, description?, children` | one card of related rows |
| `Row` | `label, hint?, children, stacked?` | label and hint on the left, control on the right (ZCode's `SettingsRow`) |
| `Field` | `label, hint?, children` | stacked label above an input |
| `Toggle` | `checked, onChange, label?, disabled?` | switch (one markup, replaces the two copies in Providers) |
| `Badge` | `tone: neutral\|ok\|warn\|bad\|accent, children` | source and state badges |
| `EmptyState` | `title, hint?, actions?` | empty lists |
| `Confirm` | `title, body, confirmLabel, danger?, onConfirm, onCancel` | replaces the native `confirm()` |
| `ListDetail` | `items, keyOf, renderRow, selectedKey, onSelect, detail, actions?` | list on the left, detail on the right (generalised from `Providers.tsx` `.pv-*`) |

Each stays under 120 lines. `ui.tsx` may be split into `ui/` files if it grows.

## 5. Saving

- One hook, `useSettingPatch()`: `{save(sectionKey, patch), busy, error}`. It sends
  `PUT /api/settings` with `{[sectionKey]: patch}` and returns the daemon's answer.
- Toggles, selects and chips save on change. Text forms keep a local draft, save with
  a button, and keep the draft across store updates (today a refresh wipes unsaved
  persona and startup text).
- The store applies `settingsChanged.settings` directly; it no longer calls
  `GET /api/state` for that event (today every change costs two GETs).
- Every section shows the result of a save in one place: the `SectionPage` header
  ("Saved" / "Not saved: reason").

## 6. Files

```
web/src/nav.ts                      VIEWS
web/src/settings/SettingsPage.tsx   shell (menu + content), < 200 lines
web/src/settings/registry.ts        SECTIONS
web/src/settings/ui.tsx             section 4
web/src/settings/settings.css       shell, menu, list-detail, form controls
web/src/settings/sections/          one file per section, < 300 lines each
  General.tsx  Appearance.tsx  Voice.tsx  Browser.tsx  Desktop.tsx  Watchers.tsx  Tools.tsx
  models/ModelsSection.tsx  models/ProviderDetail.tsx  models/AddProvider.tsx  models/Catalog.tsx
```
- `views/Settings.tsx`, `views/Tools.tsx` and `views/Providers.tsx` are deleted once
  their content lives in the sections (413-line `Providers.tsx` split in four).
- `views/Voice.tsx` keeps the live stage (orb, talk button, transcript); its three
  selects move to the Voice section and stay reachable from the tab with a "Voice
  settings" link.
- `index.css` is not rewritten; new rules go to `settings.css`.

## 7. Developer proxy

`web/vite.config.ts` proxies `/api`, `/ws` and `/live` to `http://127.0.0.1:7878`
(`ws: true`, and `changeOrigin` with the Host header set to `127.0.0.1:7878`, which
the daemon's Host check requires). `pnpm dev` then works against the running daemon;
until now UI work needed a production build and a reload for every change.

## 8. Rules for later stages

- A new settings section never adds a main tab.
- It uses `SectionPage`, `Toggle`, `Badge` and `useSettingPatch`; a page whose rows
  carry a switch (Skills, Hooks) draws its own rows instead of `ListDetail`.
- It adds its id to `registry.ts` and to `ui.rs` `SECTIONS`.
- It is verified in headless Chrome, dark and light, at 1200x820 and 700x500.

## 9. Verification (no test runner exists in `web/`)

- `pnpm -C web lint && pnpm -C web build` is clean.
- Headless Chrome (`--use-fake-device-for-media-stream`, unminified build), dark and
  light: every section opens from the menu, shows its content without console errors,
  and looks right at 1200x820 and at 700x500 (menu as a select).
- Behaviour: a persona draft survives a toggle in another card; one save causes one
  PUT; `#view=providers` opens Models; `ui show settings/skills` opens Skills.

## 10. The Calendar page (S45)

One section file, `sections/Calendar.tsx` (state, actions, the read of `GET /api/calendar` on
open and after each `calendarChanged`), and one file per card under `settings/calendar/`
(`Connection`, `Calendars`, `Places`, `Report`, `api.ts`, `calendar.css`). It uses the shared
components and `useSettingPatch`; the client secret, the token and the Maps key are typed into
password fields that are emptied after a save, and the page only hears whether they exist.

- Connection: the badge of the state (Not connected, Waiting for Google, Connected, Reconnect
  needed, Error), the account's address, the two client fields, Save client, Connect (Reconnect
  when the state is `reconnect`), Disconnect, the "Testing expires the token after 7 days" note
  and a fold with the five steps. Connect posts, then opens the returned address with
  `window.open` in the browser; it is disabled until a client id and secret are saved.
- Calendars: one row per calendar of the account (colour, name, a "read-only" badge, the default
  as a radio, a switch). A read-only calendar cannot be the default; the last calendar that is on
  cannot be switched off; switching the default off gives the default to a writable one that stays
  on. The rows save through `PUT /api/settings`, which validates them.
- Places and travel: the masked Maps key with Save, Test and Delete, the home address, the margin.
- Morning report: the switch, the two times (each with a switch for "no report on these days"),
  Spoken, Telegram (disabled with "Pair the bot first" while no chat is paired), Tomorrow,
  Reminders, Leave by, Send now (shows the text and where it went) and the last report.
- Verified by `scripts/test-calendar-web.py`: a real daemon under a temporary home behind a proxy
  that mocks the calendar and Telegram routes (no Google host is reached), headless Chrome, the
  seven states of the connection in dark and light at 1200x820 (top and bottom of the page,
  screenshots in `/tmp/kagent-s45-shots/`), 700x500, the behaviours above and no console error.

## 11. The model picker, the widget sheet and the house popup (S41)

Spec: `picker.md`. (The number is 11, not 10: S45 took 10.)

- Composer: two rows. Row 1 is `CommandInput`; row 2 (32 px) has the model trigger at the left
  and the microphone, the Plan toggle and Send or Stop at the right. The trigger is a 28 px pill:
  a chevron, the model's id without a leading `models/`, and the effort in the dim colour after a
  middle dot. Below 900 px it is a 28 x 28 px button with the text in its `title`. "No model" in
  the bad colour when nothing can answer (Send is then disabled); a chosen model that the list
  no longer has is in the signal colour with the reason in its title.
- Menu (`components/ModelMenu.tsx`): above the trigger. "Default" (names the default model),
  the chosen model if it cannot be used (disabled, with the reason), one group per provider
  with its enabled chat models (the image icon where the model reads images, a check on the
  current one), the Effort slider (hidden when the answering model offers no levels; stops are
  "Provider default" and the levels; one PUT per change) and "Manage models" (Settings at Models).
  Keys: the trigger opens on click, Enter, Space, ArrowDown or Ctrl+M; Up, Down, Home, End, Enter,
  Space and typed letters in the list; Tab goes to the slider and to "Manage models"; Escape closes
  and returns the focus. A click outside, a resize or a scroll of the page closes it.
- The `.dot` with the model left the header; Defaults has no reasoning-level row.
- Widget: a chip `.widget-model` in the status line opens the sheet "Model and effort"
  (`components/Sheet.tsx`, the widget's one sheet; `sheet` in the store). Ctrl+M opens it and never
  mutes; while a sheet is open the widget's keys do nothing and Escape closes it. The sheet keeps
  the night tokens in both themes; an approval card stays above it.
- House popup (`components/popup.css`): `.pop`, `.pop-head`, `.pop-item`, `.pop-text`, `.pop-hint`,
  `.pop-check`, `.pop-sep`; the settings `Dropdown` takes its look from the same file.
- Verified by `scripts/test-picker-web.py` (with `scripts/picker_web_lib.py`): a real daemon behind
  a proxy that mocks the model and session routes, headless Chrome, dark and light at 1200x820,
  700x500 and the widget size 340x460, screenshots in `/tmp/kagent-s41-shots/`.
