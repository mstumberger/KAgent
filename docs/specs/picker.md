# Spec: model picker in the composer (S41)

## 1. The code today

- One model answers every conversation: `settings.chatModel`, read at the start of each
  turn by the config closure of `crates/kagentd/src/main.rs` (`let cfg = move |turn:
  TurnContext| ...`, which fills `AgentConfig { model_ref: s.chat_model.clone(), options:
  ModelOptions { reasoning_level: s.options.reasoning_level.clone(), ... } }`). The one
  reasoning level is the setting `options.reasoningLevel`, edited on the Defaults card S40
  puts in `web/src/settings/sections/models/Defaults.tsx`.
- The loop overrides the model for a subagent only. `turn_loop`
  (`crates/kagent-core/src/agent/mod.rs`) calls `apply_job`
  (`agent/children/run.rs`), which replaces `model_ref`, `context_tokens` and
  `options.max_output_tokens` with those of the job's model, read through `ModelWindow`
  (`agent/children.rs`: `ModelInfo { context_tokens, max_output_tokens }`), a closure the
  daemon sets in `subagents/runner.rs` over `ProviderStore::resolve`.
- `SessionMeta` (`session.rs`) has `planning`, `approved_plan` and `todo`, no model.
  `list_sessions`, `create_session` and `get_session` (`kagentd/src/api.rs`) serialise
  the whole meta; `create_session` takes a title only.
- The UI names the model in one place, the `.dot` of the header (`web/src/App.tsx`). The
  composer (`views/Chat.tsx`) is one row: the microphone, `CommandInput`, the Plan toggle
  and Send. The widget (`views/Overlay.tsx`) has no model control.

From S41 on a conversation chooses its model and its effort in the composer, as ZCode's
`ModelConfigSelect` does. The defaults stay (section E): voice, scheduled tasks, alerts
and Telegram have no composer. S40 is done: `ProviderStore::usable`, `GET /api/models`
with the roles, `Dropdown` (`settings/Dropdown.tsx`, `settings/dropdown.css`) and
`Defaults.tsx` exist.

## 2. Data (`config.md` section 12)

`SessionMeta` gains two fields:

```json
{"id": "...", "title": "...", "createdAt": "...", "updatedAt": "...",
 "model": "anthropic::claude-opus-4-6", "reasoningLevel": "high",
 "planning": false, "approvedPlan": null}
```

| Field | Meaning |
|---|---|
| `model` | a model ref `<providerId>::<modelId>`, or `None`: the default chat model answers |
| `reasoningLevel` | one of the levels the model offers (`effective.reasoningLevels`, models.md section 4), or `None`: nothing is sent and the provider's own default applies |

- Both are `#[serde(default)]`, so a meta file from before S41 loads.
- The session of a subagent or of a coder run (phase 5) never has one: the route refuses
  it (section 3) and `start_child` (`agent/children/run.rs`) creates the child with none.
- `options.reasoningLevel` is no longer read. S40's rule for `options.maxOutputTokens`
  applies to it: the field `ModelOptionsSerde::reasoning_level` (`kagentd/src/settings.rs`)
  stays so that a value in an old file survives a load and a save, and nothing reads it.
  `config.md` section 2 says so.
- The last choice of the owner is in the browser, not in a file of the daemon:
  localStorage `kagent.picker` = `{"model": "<ref> or null", "reasoningLevel": "<level> or null"}`.
  A new conversation starts with it (section 5.6).

## 3. API (`api.md` section 8)

S41 opens `api.md` section 8 (phase 6 endpoints and events) and `config.md` section 12
(phase 6 session fields); S42 and S43 add their rows to both.

| Method path | Request | Response |
|---|---|---|
| PUT `/api/sessions/:id/model` | `{model?: string\|null, reasoningLevel?: string\|null}` | `SessionMeta` |
| POST `/api/sessions` | `{title?, model?, reasoningLevel?}` | `SessionMeta` (201) |
| GET `/api/models` | | as models.md section 4, and each model of every list gains `providerId` and `providerName` |

Rules of the two writes (one function checks both):

- A key left out keeps its value; `null` or `""` clears it.
- Every check runs before anything is written. A refused request changes nothing and
  makes no session.
- 404 `not_found` "session <id> not found".
- 409 `conflict` "the session of a subagent takes the model of its job: it cannot be
  changed" for a session with origin `subagent`.
- 400 `invalid_input`:
  - a body that is not an object, a key other than `title` (on POST), `model` and
    `reasoningLevel`, or a value that is neither a string nor null: "expected an object
    with model and reasoningLevel (strings, or null to clear)".
  - a model that is not usable: "model '<ref>' cannot be used: <reason>". The reason is
    one of "unknown model" (not a ref, or the providers do not know it), "provider
    switched off", "model switched off", "not a chat model".
  - a level the model that answers does not offer: "model '<ref>' has no effort '<level>'
    (it offers: low, medium, high)", or "(it offers none)". The model that answers is
    the new `model` when the body names one, else the session's model, else
    `settings.chatModel`.
- When `model` is given and `reasoningLevel` is not, a stored level the new model does
  not offer is cleared: the level goes with the model.
- Nothing changes and no event goes out when the meta would be the same. Otherwise, after
  the file is written:

```json
{"type":"sessionMetaChanged","sessionId":"...","meta":{/* SessionMeta */}}
```

The event carries the whole meta, so a window applies it without reading anything (S42
reuses it for the folder and the pin).

The work is in `crates/kagentd/src/model_api.rs` (new), on the pattern of `plan_api.rs`:
functions over the agent and the providers, tested without a server, and the axum
handlers around them.

- `pub fn choice_of(body: &Value) -> Result<Choice, String>`: the body as
  `Choice { model: Option<Option<String>>, level: Option<Option<String>> }` (outer
  `None`: the key was left out).
- `pub fn check(store: &ProviderStore, current: Option<&SessionMeta>, default_model: &str, choice: &Choice) -> Result<(), HttpError>`:
  the two 400s of a model and of a level. The model that answers is `choice.model`
  when the body names one, else the model of `current`, else `default_model`; at
  creation `current` is `None`.
- `pub async fn set(agent: &Arc<Agent>, store: &ProviderStore, id: &str, body: &Value) -> Result<SessionMeta, HttpError>`
  and `pub async fn create(sessions: &SessionStore, store: &ProviderStore, body: &Value) -> Result<SessionMeta, HttpError>`
  (after `SessionStore::create` the two fields are set and the meta is saved again).
  Both check first and write after; `set` then calls `set_model_choice` (4.1).
- `api.rs`: `create_session` calls `model_api::create`; `server.rs` gets the route
  `put(api::model_api::put_model)` on `/sessions/{id}/model`.
- `kagent-providers`: `ProviderStore::unusable(&self, model_ref) -> Option<&'static str>`
  beside `usable` (`store.rs`): `None` when the model can be used, else the reason. `usable`
  does not change. `api/providers.rs` `model_lists` adds `providerId` and `providerName`.

## 4. Rules of the loop

### 4.1 Storing the choice

`crates/kagent-core/src/agent/picker.rs` (new) holds what S41 adds to the agent, so that
`state.rs` and `children/run.rs` stay as they are. `Agent::set_model_choice(&self, id: &str, model: Option<String>, level: Option<String>) -> anyhow::Result<SessionMeta>`
does it as `change_planning` (`agent/state.rs`) does: under the meta lock the two fields
are set and the file is written (the old values come back on an error), then, after the
lock, `AgentEvent::SessionMetaChanged { session_id, meta }` goes out.

### 4.2 What answers a turn

`turn_loop`, right after `self.apply_job(session_id, &mut config)` and before the check
of the empty model, calls `self.apply_choice(session_id, state, &mut config).await`
(`agent/picker.rs`). It leaves `updated_at` alone: the choice does not reorder the sessions.

1. A subagent is not affected: when `meta.origin` is `Subagent` (read under the same lock) nothing is done.
2. `meta.model` is read under the meta lock. `None`, or the choice equals `config.model_ref` (the default): nothing changes and no notice goes out.
3. The choice is asked of the source of section 4.3. Usable: `config.model_ref`, `config.context_tokens` and `config.options.max_output_tokens` become those of the choice, as `apply_job` does it. Not usable: the default answers, and a `notice` (level `warn`) says why, once per turn:
   `"<choice> cannot answer (<reason>) - answering with <default>"`, the reason being that of section 3.
4. The effort goes with the model that answers, never with a model that was left: `config.options.reasoning_level` is `meta.reasoning_level` when the model that answers lists it among its levels, else `None`. A stored level that is not offered is not sent, and a `notice` (level `warn`) says so, once per turn:
   `"effort '<level>' is not one <model> offers (<levels, or none>) - answering with its default"`.
5. Without a choice `reasoning_level` is `None`: the config closure no longer reads `options.reasoningLevel` (`main.rs`, one line: `reasoning_level: None`).
6. A fallback switch (`switch_to_fallback`, `compact/loop_side.rs`) keeps the level only when the fallback model offers it, asked of the same source; else the rest of the turn sends none.

So a session with a choice answers even when no default chat model is set, a task, an
alert investigation and a Telegram chat (no choice) keep the default, and a voice session
in `routed` mode thinks with its own choice.

### 4.3 The source

- `ModelInfo` (`agent/children.rs`) gains `reasoning_levels: Vec<String>`, and a second
  type beside `ModelWindow`:
  `pub type UsableModels = Arc<dyn Fn(&str) -> Result<ModelInfo, &'static str> + Send + Sync>;`
  `Ok` for a model that can be used, `Err(reason)` otherwise.
- `Children` (`agent/children/registry.rs`) holds it as it holds `model_window`
  (`set_usable_models`, `usable_models`), and `Agent::set_usable_models(&self, source: UsableModels)`
  is in `agent/picker.rs`, beside `apply_choice`.
- The daemon sets it in `subagents/runner.rs`, beside `set_model_window`: the closure is
  `providers.usable(r)` mapped to `ModelInfo { context_tokens, max_output_tokens: Some(..), reasoning_levels }`,
  and `providers.unusable(r).unwrap_or("unknown model")` as the error. The closure of
  `set_model_window` fills `reasoning_levels` too (the one field it lacked).
- Unset (a core test that sets none): the choice replaces `model_ref` alone and no
  notice goes out; a level is sent as it is stored.

Known and accepted: the memory block of the closure is sized by the window of the default
model, not of the session's choice (the closure cannot read the session). The history
budget counts the whole prompt, so the request still fits.

## 5. The UI (`ui-settings.md` section 11)

### 5.1 The composer (`views/Chat.tsx`)

The composer gets two rows, the pattern of ZCode's `V4ComposerToolbar`:

- Row 1: `CommandInput` alone, the full width.
- Row 2, a bar 32 px high: at the left the trigger of `ModelMenu` (5.2); at the right
  the microphone button, the Plan toggle and Send or Stop, as they are today.

The trigger is a pill button 28 px high (padding `0 10px`, `--glass-2`, 1 px rim,
a radius of 999 px, font 12.5 px): a chevron of 14 px that turns when the menu is open,
the model, and the effort in `--dim` after a middle dot when the session has one. The
model is the id of its ref without a leading `models/` (models.md 7.8; no request is
needed for it); at most 220 px, cut with an ellipsis. Below 900 px of viewport width the
name is hidden and the button is 28 x 28 px with the text in its `title`.

- Which model: the session's `model`, else `settings.chatModel`. A stored model that the
  last list does not have (5.6) is `--signal`, and its reason is in the `title`.
- No usable model at all (`chatModel` not usable and no usable choice): the trigger reads
  "No model" in `--bad` and Send stays disabled.
- Disabled (no session open, or the open session is a subagent's, which has no composer).
- `title`: "Model and effort (Ctrl+M)".
- The placeholder of the input when nothing can answer is "Pick a model in the menu below first".
  Send is enabled when `chatModelValid` or the session's model is in the list.
- `CommandInput` gets a prop `suspend?: boolean`: while it is true the slash picker does
  not open (the model menu is open).

### 5.2 The menu (`components/ModelMenu.tsx`)

A popup in the house style (5.8): above the trigger, its left edge on the trigger's;
at least 260 px and at most `min(360px, 100vw - 32px)` wide; at most `min(420px, 60vh)`
high, the list scrolls from 280 px. From the top:

1. The row "Default": label `Default`, the dim second line names the default chat model
   (or "none: pick a model"). A check when the session has no model. Picking it sends
   `{model: null, reasoningLevel: null}`.
2. The row of a stored model that is not usable (when there is one): above "Default", as
   models.md 7.8 keeps an option that is not in the list: its id without a leading
   `models/`, the reason as the second line, `aria-disabled`, a check when it is the
   session's. Nothing happens on a click.
3. One group per provider that has an enabled chat model, in the order of `GET /api/models`
   (`chat`): a heading with `providerName` (`.pop-head`), then a row per model: the id
   without a leading `models/`, the `image` icon (13 px, `title` "Reads images") where
   `config.supportsImage` is true, and a check on the current one.
4. A separator and the effort row, only when the model that answers offers levels: the
   word "Effort", a slider whose stops are `default` and then the model's levels in
   their order (`<input type="range" min="0" max="{n}" step="1">`, a dot per stop, the
   track filled in the ice colour up to the thumb, a thumb of 14 px), and the name of
   the current stop at the right ("Provider default", as the Defaults card of S40 names
   it, or the level as it is written). The value is 0 for a session without a level or
   with one the model does not offer, else the index of the level plus one. A change
   sends `{reasoningLevel}`: the level, or `null` for the first stop. One PUT per change.
5. A separator and "Manage models" (the `gear` icon): it closes the menu and opens
  Settings at `models` (`showView("settings", "models")`).

A pick sends `{model}` and closes the menu. The answer of every PUT replaces the
session's meta in the store (`metas[id]` and its row in `sessions`) and is remembered as
the last choice (5.6). A refusal is a notice in the header and the menu closes.

### 5.3 Keys and mouse of the menu

- The trigger: click, Enter, Space or ArrowDown open it; the focus moves to the current
  row. `aria-haspopup="listbox"`, `aria-expanded`, `aria-controls`.
- In the list: Up and Down move the cursor, skipping headings and the disabled row; Home
  and End go to the first and the last row; Enter or Space pick; letters typed jump to
  the first row after the cursor whose label starts with them (forgotten after 700 ms).
- Tab moves from the list to the slider (when it is there), then to "Manage models";
  Shift+Tab back. In the slider the arrows are those of the range input.
- Escape closes the menu and the focus returns to the trigger. A click outside, a resize
  or a scroll of the page closes it too.
- Mouse: hover moves the cursor; a click on a row picks; a press inside the popup on
  anything but a row does nothing.
- The cursor row is the only row in the tab order (`tabIndex` 0, the others -1).

### 5.4 Ctrl+M

- In the chat: a `keydown` listener on `window` (an effect of `Chat`) toggles the menu on
  `(ctrlKey || metaKey) && code === "KeyM"` and calls `preventDefault()`.
- In the widget: the key handler of `Overlay` gets the same check before its own `KeyM`
  (mute), so Ctrl+M never mutes; it opens the sheet of 5.5.
- In any other view Ctrl+M does nothing.

### 5.5 The widget (`views/Overlay.tsx`)

The status line gets a chip `.widget-model` after the label (the look of `.widget-todo`):
the model's id without `models/`, at most 90 px, and the effort after a middle dot. A
click opens the sheet "Model and effort".

`Sheet` (`components/Sheet.tsx`) is the widget's one sheet, for S41 to S43:

- Props `{ title, onClose, children }`. It fills the widget: `position: fixed; left: 8px;
  right: 8px; top: 8px; bottom: 58px` (above the bar), z-index 19 (under the approvals,
  which stay the loudest thing), `role="dialog"`, `aria-modal="true"`, `aria-label` the
  title. A head with the title and a close button (`aria-label="Close"`); the body
  scrolls.
- Only one sheet is open at a time: `sheet` in the store (`"models" | null` here; S42 and
  S43 add theirs), with `openSheet(name)` and `closeSheet()`.
- While a sheet is open the keys of the widget do nothing: the key handler of `Overlay`
  returns at once when `getStore().sheet` is set, and the sheet handles Escape itself.
  Space on a button inside the sheet never reaches the voice.
- `sheet.css` also sets `.widget-approvals { z-index: 21; }`, so an approval card stays
  above a sheet (`index.css` gives it none).
- The content of the sheet "Model and effort" is the body of the menu (5.2, items 1 to 5)
  as it is, full width. A pick there applies and keeps the sheet open, so the slider is
  at hand.

### 5.6 New conversations and the store

- `web/src/picker.ts`: the types (`PickerModel`: a `ModelView` with `providerId` and
  `providerName`), `list()` (GET `/api/models`, kept for 10 s), `label(ref)`, `last()`
  and `remember(choice)` over localStorage `kagent.picker`.
- `newSession()` posts `{model, reasoningLevel}` from `picker.last()` when the remembered
  model is still in the list (the list is read first when it is older than 10 s); a
  remembered model that is gone is forgotten and not sent.
- `setSessionModel(id, body)` sends the PUT, applies the answer and calls
  `picker.remember`.
- `handleEvent` gets `sessionMetaChanged`: it replaces `metas[sessionId]` when the store
  has it and patches the session's row.
- `init()` and every opening of the menu read the list when it is older than 10 s.
- `events.ts` and `api.ts` (`SessionMeta`) get the event and the two fields. `api.ts` is
  at its budget: the other types go into `picker.ts`.

### 5.7 The header and the Defaults card

- The `.dot` of the header goes (`App.tsx`). Its rules stay in `index.css`, which is not
  rewritten.
- `Defaults.tsx` loses the row "Reasoning level" (the `Row` with its `Dropdown` and the
  hint "No chat model offers reasoning levels."), the `level` and `levels` it reads from
  `settings.options.reasoningLevel`, and the `save("options", { reasoningLevel })`; the
  comment of the file no longer names the level. Temperature stays (models.md section
  7.6, corrected).

### 5.8 The house popup (`components/popup.css`, new)

The look is that of the composer's `.cmd-list` (`components/command.css`) and of the
dropdown list S40 wrote in `settings/dropdown.css`: `--glass`, the strong rim, the medium
radius, that shadow, 4 px of padding. One file holds it for every menu, list and sheet of
phase 6, with the values of `dropdown.css`:

| Class | Use |
|---|---|
| `.pop` | the panel: `var(--glass)`, 1 px `var(--rim-strong)`, `var(--radius-m)`, `0 12px 32px -12px rgba(0, 0, 0, 0.6)`, padding 4 px |
| `.pop-head` | a group heading: padding `6px 10px 2px`, font 10.5 px, letter spacing 0.06em, upper case, `--dim` |
| `.pop-item` | a row: flex, centred, gap 8 px, padding `6px 10px`, radius 8 px, font 13 px, `--ink`, a pointer |
| `.pop-item:hover`, `.pop-item.cursor` | the ice colour at 12 percent |
| `.pop-item.off` | a disabled row: `--dim`, no background, no pointer |
| `.pop-text` | the label: one line, cut with an ellipsis |
| `.pop-hint` | the dim second line or the dim end of a row: font 12 px, `--dim` |
| `.pop-check` | the check mark on the current one: `--ice` |
| `.pop-sep` | a separator: 1 px, margin `4px 6px`, `--rim` |

`settings/Dropdown.tsx` (S40) takes its look from the same file: its list, rows, hints
and check carry `pop`, `pop-item`, `pop-hint` and `pop-check` beside their `dd-` classes.
`dropdown.css` keeps the rules of the trigger and the placing and the size of the list
(`.dd-list`: the position, the width, `--dd-space`, `.above`, `.right`) and loses those
of the look (`.dd-option`, `.dd-hint`, `.dd-check` and the `background`, `border`,
`border-radius`, `box-shadow` of `.dd-list`), which move to `popup.css`.

## 6. Files

```
web/src/
  components/ModelMenu.tsx     the trigger, the popup, the body the sheet reuses (5.2, 5.3): < 260 lines
  components/model-menu.css    the trigger, the popup, the effort row: < 110 lines
  components/Sheet.tsx         the widget's sheet (5.5): < 60 lines
  components/sheet.css         its rules: < 50 lines
  components/popup.css         5.8: < 70 lines
  components/Icon.tsx          `chevron` and `check` (S40 adds `image`)
  components/CommandInput.tsx  the prop `suspend`
  picker.ts                    5.6: < 90 lines
  views/Chat.tsx               the composer in two rows, Ctrl+M: < 300 lines
  views/Overlay.tsx            the chip and the sheet: < 230 lines
  App.tsx                      the dot goes
  store.ts                     `setSessionModel`, the event, `sheet`: < 400 lines
  events.ts                    `sessionMetaChanged`
  api.ts                       `SessionMeta.model`, `SessionMeta.reasoningLevel` (two lines)
  settings/Dropdown.tsx        the classes of `popup.css`
  settings/dropdown.css        the look of the list leaves it
  settings/sections/models/Defaults.tsx   no reasoning level row

crates/kagent-core/src/
  session.rs            the two fields and their test: to ~320 lines
  protocol.rs           AgentEvent::SessionMetaChanged
  agent/children.rs     ModelInfo.reasoning_levels, UsableModels
  agent/children/registry.rs   the source and its two accessors: to ~250 lines
  agent/picker.rs (new) set_model_choice, set_usable_models, apply_choice: < 110 lines
  agent/mod.rs          `mod picker;` and one call in `turn_loop`
  compact/loop_side.rs  the level at a fallback switch

crates/kagent-providers/src/store.rs       ProviderStore::unusable (its test in store/tests.rs)
crates/kagentd/src/api/providers.rs        providerId and providerName in `model_lists`
crates/kagentd/src/
  model_api.rs (new)    section 3 and its tests: < 230 lines
  api.rs                `create_session` takes the choice
  server.rs             the route
  settings.rs           the comment on `reasoning_level` (kept, not read)
  main.rs               `reasoning_level: None` in the config closure
  subagents/runner.rs   `set_usable_models`, `reasoning_levels` in the window
```

## 7. Scenarios

`scenarios.md` has no S41 block at the time this spec is written; the stage agent copies
this list there as "S41 model picker in the composer".

1. **[auto]** `model_api.rs`: each 400 of section 3 with its message, the 409, the 404; a
   key left out keeps its value; a level is cleared when the model changes to one that
   does not offer it; `POST /api/sessions` with both; a refused create makes no session.
2. **[auto]** Scripted model: a session's choice replaces the model, the window and the
   output limit of the request and sends the level; a choice that cannot be used makes
   the default answer and a notice says why; a level the model does not offer is not
   sent; a subagent keeps the model of its job.
3. **[auto]** The fields round-trip; a meta from before S41 loads; `unusable` names the
   reason; an old `options.reasoningLevel` survives a load and a save.
4. Headless Chrome (dark and light, 1200x820 and 700x500): the trigger shows the model
   and the effort; Ctrl+M opens the menu; a pick sends one PUT and the trigger changes;
   the check sits on the current one; the image icon shows where the model reads images;
   the slider is hidden for a model without levels and sends one PUT per change; "Manage
   models" opens Settings at Models; "Default" clears the choice.
5. Headless: a new conversation starts with the last choice; a remembered model that is
   gone is not sent.
6. Headless, in the widget (340x460): the chip opens the sheet; a pick applies and the
   sheet stays open; Escape closes it; Space and M do nothing to the voice while it is
   open; Ctrl+M opens it and does not mute.
7. Headless: the header has no model dot; with no usable model the trigger reads "No
   model" and Send is disabled; with no default but a usable choice, Send is enabled.
8. The owner, live: pick another model in one conversation and ask something; the answer
   comes from that model, the other conversations keep the default; a task and a Telegram
   chat keep the default too.

## 8. ZCode references

ZCode is at `/home/virus/SystemInterface/ZCode/packages/`; the paths are under `ui/src/`.
Take the pattern, not the code: KAgent has no component library and no Radix.

| File | Take |
|---|---|
| `chat-input-toolbar/ComposerModelMenu.tsx` | the menu under one provider (lines 102 to 381): the "Models" label, the items, a separator, then the effort slider, an `input[type=range]` whose stops are the levels (lines 213 to 374); `showEffort` only when the model has levels (line 141) |
| `chat-input-toolbar/ComposerProviderMenu.tsx` | the providers as radio rows (lines 84 to 110), a row with its badge |
| `ModelConfigSelect.tsx` | the trigger and its props (lines 146 to 304): the selected model's label, a badge after it (line 196), the option above every group (`leadingItems`, lines 190 to 195), `onManageModels`; the menu opens above the trigger (`contentSide`, line 239) |
| `v4/composer/V4ComposerToolbar.tsx` | Ctrl+M opens the menu (lines 498 to 503); the toolbar row (lines 1304 to 1358): with no model the trigger is "Manage models", then the model trigger and the effort control after it |
| `chat-input-toolbar/thoughtLevelOptions.ts` | lines 9 to 26: `disabled` and its like are the "no thought" levels (the first stops of the slider) |

## 9. Tests

- `crates/kagent-providers/src/store/tests.rs`: `unusable` answers each of the four
  reasons and `None` for a usable model; it agrees with `usable` on every row of a store
  built as `api/providers/tests.rs` builds one. (Scenario 3.)
- `crates/kagent-core/src/session.rs`: the two fields round-trip under `model` and
  `reasoningLevel`; the meta of the existing test from before the fields loads with
  `None`. (Scenario 3.)
- `crates/kagentd/src/settings/tests.rs`: a file with `options.reasoningLevel` loads and
  saves it unchanged. (Scenario 3.)
- `crates/kagentd/src/model_api.rs`: every refusal of section 3 with its status, code and
  message; a key left out keeps its value and `null` clears it; the level cleared by a
  change of model; the same meta twice changes nothing and sends no event; the event
  carries the stored meta; a session with origin `subagent` is 409; `create` with both
  fields, and a refused `create` leaves no file. (Scenarios 1 and 2, the API half.)
- `crates/kagent-core/tests/agent_loop.rs` (the rig of `tests/common/mod.rs`, whose
  `ScriptedModel` also records `options.reasoning_level` of each request): with a source
  that knows two models, a session's choice gives the request its `model_ref`, its output
  limit and its level; a choice the source refuses makes the default answer and the
  `notice` says the reason; a level the model does not offer is not sent and the `notice`
  says so; a choice equal to the default gives no notice; with no source the choice
  replaces `model_ref` alone; a turn out of quota with a fallback that does not offer the
  level sends none to the fallback. (Scenario 2.)
- `crates/kagent-core/tests/subagents/main.rs`: the requests of a child of a session with
  a choice carry the model of the job, never the parent's choice. (Scenario 2.)
- The headless pass on an own dev server with stubbed routes: scenarios 4, 5, 6 and 7,
  each as one check of the harness, in that order.

Scenarios: `scenarios.md` S41.

## 10. Documents of the same stage

- `api.md` section 8 (opened here): the three rows of section 3 and the event
  `sessionMetaChanged`.
- `config.md` section 12 (opened here): the two session fields and the note on
  `options.reasoningLevel` (section 2 of that file).
- `ui-settings.md` section 11 (new; 10 is the Calendar page of S45): the composer's model menu, the header without the
  dot, the widget's sheet and the house popup of 5.8.
- `models.md` section 6 (the sentence on `options.reasoningLevel`) and section 7.6 (the
  Defaults card without the reasoning level).
- `scenarios.md`: S41 (section 7).
