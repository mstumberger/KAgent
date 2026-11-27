# Spec: models and providers (S40)

The Models section of the settings screen has one global form for every model: a
reasoning level, an output size and a temperature in cards under the provider list,
and the roles as buttons on the rows. Nothing can be set per model, although
`providers.json` has held a per-model `config` patch since S2 and `api.md` promised
`config?` on `PUT /api/providers/:id`. "Fetch models" also marks every model it finds
as `chat` (text-to-speech, image and music models included), adds the same Gemini
model twice (`gemini-3.8-flash` and `models/gemini-3.8-flash`) and switches new models
on while the list is small. And a fallback whose provider is switched off turns "out
of quota" into "provider 'ollama' is disabled".

From S40 on the section follows ZCode's Model settings: a provider card that saves as
you leave a field, a row per model with a pencil that opens a per-model dialog with a
"Smart configuration" switch, and one card for what KAgent needs beyond ZCode (the
default model of each role).

## 1. Model entry (`config.md` section 4)

```json
{"id": "models/gemini-2.5-flash", "name": "", "enabled": true, "roles": ["chat"], "smart": true,
 "config": {"contextWindow": 1000000}}
```

| Field | Meaning |
|---|---|
| `id` | what the provider's API takes as `model`; unique inside the provider |
| `enabled` | on: the model can be chosen and used |
| `roles` | the jobs the model can take. `chat`: text turns, and eligible for `fastModel`, `fallbackModel` and `visionModel`. `live`: Gemini Live voice. `[]`: no job (an embedding, text-to-speech, image or music model), listed by no `/api/models` list. A model can have both |
| `smart` | `true` (the default, also for a file written before S40): `config` holds only what the owner changed and the rest comes from the built-in rules (`config.md` section 3). `false`: the editable fields come from `config` alone (section 2) |
| `config` | the personal patch (`ModelConfigPatch`) |

- The editable fields are `contextWindow`, `maxOutputTokens`, `supportsToolCall`,
  `supportsImage`, `reasoningLevels` and `reasoningPatches`. The rest of the patch
  (`enabled`, `roles`, `supportsLive`, `liveProtocol`, `requestPatch`) stays what the
  built-in rules set; the dialog does not show it.
- A row without `roles` (the first format of `config.md` section 4) is a chat model.
  An empty list is no job, where `list_models` reads it as `["chat"]` today. So the
  field loads with the default `["chat"]` when it is missing (`serde`), not `[]`.

## 2. Resolution

`kagent_providers::rules::resolve` takes one more argument, `smart: bool`.

- `smart = true`: as today. Defaults, then the built-in rules class by class, then the
  personal patch on top.
- `smart = false`: defaults, the built-in rules, then the six editable fields are reset
  to their defaults (context window 128000, max output 8192, tool calls on, images
  off, no levels, no patches), then the personal patch. The rules still decide what
  the owner cannot edit (the request patch of a site, the live protocol) and nothing
  else. This is ZCode's manual rule (`model-config.ts`, `ModelConfigRules.resolve`).

Two values are reported for a model:

- **inherited**: the resolved config with an empty personal patch and `smart = true`.
  The dialog shows it as the placeholder of each field.
- **effective**: the resolved config, what is sent today.

A manual model must hold `contextWindow`, `maxOutputTokens`, `reasoningLevels` (it may
be `[]`), `supportsToolCall` and `supportsImage` in `config`; `reasoningPatches` may be
left out (then the per-API default of `config.md` section 3 applies). The daemon
enforces this when an entry is saved (section 4), not when a file is loaded: a file
edited by hand that breaks the rule loads, and a missing field is the default.

`ProviderStore` gets `usable(model_ref) -> Option<ResolvedModel>`: the model exists, is
enabled, has role `chat` and its provider is enabled. The daemon uses it for the
chat, fast and fallback model where it only called `resolve` before.

## 3. Fetching models

`kagent_providers::discovery::merge_models`:

1. **One model, one row.** A fetched id that differs from a row's id only by a leading
   `models/` (either way round) is that row: nothing is added and the row keeps its
   id, flags and config. (Gemini's OpenAI-compatible `/models` answers
   `models/gemini-2.5-flash`; its template lists `gemini-3.8-pro`.)
2. **A job from the name.** A new row gets its roles from its id, lower case, first
   match wins:

| The id contains | `roles` |
|---|---|
| `-live`, `native-audio`, `realtime` | `["live"]` |
| `embed`, `tts`, `transcribe`, `audio`, `whisper`, `lyria`, `imagen`, `image`, `banana`, `veo`, `robotics`, `computer-use`, `deep-research`, `antigravity`, `aqa` | `[]` |
| anything else | `["chat"]` |

3. **New rows arrive disabled**, whatever the size of the list (the rule "on while
   fewer than five" is gone). The owner switches on what they want, so the picker of
   S41 never lists a music model.
4. **Rows that are wrong today.** Once, at the first load of a `providers.json` with
   `schemaVersion` below 2, a row is given the roles of the table when three things
   hold: its `config` is empty, its `roles` are `["chat"]`, and its id matches the first
   or the second row of the table. The next save writes `schemaVersion: 2`, so a role
   the owner gives such a row afterwards is never taken back at a start. The correction is idempotent;
   when it changes a row, that is logged once per provider with the number of rows
   ("gemini: 31 models are not chat models"). A row named in settings (`chatModel`,
   `fastModel`, `fallbackModel`, `visionModel`) is left alone. Nothing else moves in
   an existing `providers.json`.

Rows a template brings (`ProviderStore::add_from_template`) keep today's rule: its
chat models are on when the template lists fewer than five, its live models are
always on.

## 4. API (`api.md` section 3)

`GET /api/providers`: every model gains four fields.

```json
{"id": "models/gemini-2.5-flash", "name": "", "enabled": true, "roles": ["chat"], "smart": true,
 "config": {"contextWindow": 1000000},
 "inherited": {"contextWindow": 1000000, "maxOutputTokens": 128000, "supportsToolCall": true,
               "supportsImage": false, "reasoningLevels": ["disabled", "enabled"], "reasoningPatches": {},
               "supportsLive": false, "liveProtocol": null, "requestPatch": {}, "roles": ["chat"], "enabled": true},
 "effective": {"the same keys"}}
```

`PUT /api/providers/:id` takes the keys of today (`name`, `baseUrl`, `apiKey`,
`headers`, `enabled`) and then, in this order:

| Key | Meaning | Refused with |
|---|---|---|
| `apiType?` | one of the four API formats | 400 for another value |
| `addModels?: [{id}]` | new rows: disabled, smart, roles from section 3 | 400 when the id exists (also as a `models/` twin) or is not 1 to 200 characters without whitespace |
| `models?: [{id, enabled?, roles?, smart?, config?}]` | changes rows that exist; a key left out keeps its value; `config` replaces the patch and `roles` replaces the roles | 400 for an unknown id, a role outside `chat` and `live`, a repeated role, and what the next list says |
| `removeModels?: [id]` | deletes rows | 409 `conflict` while settings name the model in `chatModel`, `fastModel`, `fallbackModel`, `visionModel` or `liveModel` (the message says which) |
| `modelOrder?: [id]` | the order of the rows after the keys above | 400 unless it names every row exactly once |

Every check runs before anything is written: a refused request changes nothing.

Validation of `config` and `smart` (400 `invalid_input`; the message names the field):

- `contextWindow`, `maxOutputTokens`: whole numbers above 0.
- `reasoningLevels`: no blank entry, no repeat.
- `reasoningPatches`: every key is one of the effective levels and every value is a
  JSON object.
- With `smart: false`: `contextWindow`, `maxOutputTokens`, `reasoningLevels`,
  `supportsToolCall` and `supportsImage` must be in `config`.

`POST /api/providers` gains two templates, both extras of
`scripts/import-zcode-providers.py`: `custom` ("Custom (OpenAI-compatible)":
`openai-chat-completions`, no base URL, no models) and `nvidia` ("NVIDIA":
`openai-chat-completions` at `https://integrate.api.nvidia.com/v1`, three models; the
rules of its models are in `config.md` section 3). For `custom` `name` and `baseUrl`
are required (400), and it takes `apiType?` too (400 for another value), so the message
format is chosen when the provider is created.

`GET /api/models`: `chat`, `fast` and `vision` list the models with role `chat`, `live`
the models with role `live`.

`GET /api/state`: `chatModelValid` is true only when `usable(chatModel)` is some. (It
was true for any model the providers know.)

`PUT /api/settings`: a model without role `chat` cannot become `chatModel`,
`fastModel`, `fallbackModel` or `visionModel` (400); the check that a live model
cannot answer text stays.

## 5. Fallback

- A turn gets `fallback_model_ref` only when `usable(fallbackModel)` is some (the rule
  the fast model already follows in `main.rs`). An unusable fallback is none: the turn
  then fails with the model's own error ("provider error 429 ... exceeded your current
  quota"), not with "provider 'ollama' is disabled".
- The notice of a switch says why: `X is out of quota - answering with Y` when the
  error has 429 or "quota"; `X rejected the API key (Providers) - answering with Y` as
  today; otherwise `X is busy - answering with Y`.

## 6. Settings

- `options.temperature` stays, in the Defaults card. `options.reasoningLevel` is no longer
  read since S41: the effort is chosen per conversation (`picker.md`; `config.md` section 2).
- `options.maxOutputTokens` is no longer read: the model's own value is the only
  source. A value in an old file is ignored (`config.md` says so).
- `maxIterations` and `approvalTimeoutSec` move to Tools and permissions, card "Limits
  of a turn", unchanged otherwise.

## 7. The section (`web/src/settings/sections/models/`)

The page: the description; "Refresh" and "Add provider" on the right; one card with
two columns, the provider list on the left and the detail on the right (KAgent's
`ListDetail`, as today); the Defaults card under it.

### 7.1 Provider list (`ProviderList.tsx`)

- The owner's providers in their order: logo letter, name, status dot (as today). A
  drag reorders them (`PUT /api/providers/order`, which exists). Below 900 px of
  viewport width the list is a `Dropdown` (7.8).
- "Add provider" at the end of the list opens 7.2.

### 7.2 Add provider (`AddProvider.tsx`)

- The template picker stays (search, the templates, "added"). It gains "Create custom
  provider", the form of the `custom` template: name, base URL, API format (a select
  of the four, OpenAI chat completions first) and API key (optional).
- A template provider asks for its key only, as today.

### 7.3 Provider card (`ProviderCard.tsx`)

- Rows: Name (an input in the title), Enabled (a toggle), API format (a `Dropdown` of
  the four), Base URL (an input; read-only with the template's address for every template
  but `custom`), API key (a password field with Show and the "Get a key" link).
- A field saves when it is left or Enter is pressed: one `PUT` per field. The key
  field is cleared after its save. A line under the card says "Saved" or "Not saved:
  reason".
- Buttons: Test connection, Fetch models, Delete (a `Confirm`, as today).

### 7.4 Model rows (`ModelList.tsx`)

- Head: "Models", "n of m enabled", the filter, "Enabled only" (as today), "Add model".
- A row, left to right: the drag handle (reorders, `modelOrder`), the enabled toggle,
  the id, the badges, the buttons.
- Badges: the effective context window (`1M`, `128k`, `4096`), an image icon with the
  title "Reads images" when `effective.supportsImage`, and the roles in use (`chat`,
  `fallback`, `fast`, `vision`, `voice`), as today.
- Buttons: Test (not for a model whose only role is `live`), the pencil, the trash (a
  `Confirm`; a 409 is shown as the reason).
- The buttons "Use for chat", "Fallback" and "Use for voice" go: roles are given in
  the Defaults card.

### 7.5 The model dialog (`ModelDialog.tsx`)

Over the page with `settings/dialog.tsx` (`wide`). Title "Edit model" or "Add model".

- Header row: the "Smart configuration" toggle and its hint, "Take the built-in
  values; only what you change here is saved."
- Model ID: read-only unless the model is not in its provider's template lists.
- Context window, Max output tokens: with Smart on the placeholder is the inherited
  value and an empty field means "inherit". A field the patch holds has the accent
  border and an "override" tag. A number that was typed stays an override even when
  it equals the inherited value; clearing the field removes the override.
- "Advanced settings", collapsed (a chevron button; it opens by itself when the error
  is in one of its fields):
  - Accepts images, Supports tool calls: toggles.
  - Roles: the chips `chat` and `live`.
  - Reasoning levels: an ordered list with add, remove, up and down.
  - Reasoning parameter mapping: one JSON text field (an object: a level and the body
    patch for it). With Smart on the placeholder is the inherited mapping.
- Smart switched off: the inherited values become the values of the fields, so nothing
  is lost by the switch. Switched on again: every field that equals the inherited
  value stops being an override.
- Footer: Reset (the stored values; in "Add model" it clears the form), Cancel, Save.
  Enter in a text field saves.
- Save checks as the server does (section 4), in the same order; the first error is
  shown under its field and that field gets the focus. Then one `PUT`: `addModels` for
  a new model and `models` for the change, in the same request. With Smart off the
  patch holds the six editable fields (`reasoningPatches` only when the text is not
  empty); with Smart on only the overrides.

### 7.6 Defaults (`Defaults.tsx`, replaces `Roles.tsx`)

- One `GroupCard`, "Defaults": Default chat model, Fallback, Fast, Vision. Four
  dropdowns (7.8) over the models with role `chat` (the list of the Roles card today;
  an empty value for none).
- A dropdown whose model cannot be used keeps its option and shows why under it: "provider
  switched off", "model switched off" or "unknown model".
- Temperature, as today. The reasoning level left the card in S41: the effort is chosen per
  conversation in the composer (`picker.md`).
- The live model stays in the Voice section.

### 7.7 Files

```
web/src/settings/sections/models/
  ModelsSection.tsx   the section: header actions, the two columns, Defaults
  ProviderList.tsx    7.1
  AddProvider.tsx     7.2 (changed)
  Catalog.tsx         unchanged
  ProviderCard.tsx    7.3
  ModelList.tsx       7.4
  ModelDialog.tsx     7.5
  Defaults.tsx        7.6
  shared.ts           unchanged
```

`ProviderDetail.tsx`, `Roles.tsx` and `Advanced.tsx` are deleted. Every file stays
under 300 lines. The new CSS goes into `settings.css`; `index.css` gets one rule, the
one at the end of 7.8. `Tools.tsx` gets the card of section 6.
`web/src/settings/Dropdown.tsx` is 7.8.

### 7.8 Dropdown (`web/src/settings/Dropdown.tsx`)

Every picker of this section is `Dropdown`, not a native `<select>`: the four of the
Defaults card, the API format of the provider card and the provider list under 900 px.
The popup of a native select is drawn by the browser, and in Brave on Linux it is a
white list, also in the dark theme; the owner rejected it on 2026-11-27 ("fix dropdown
styles": the Fallback picker, pale text on white and `gemini::models/...` as labels).

- The options are `{ value, label, hint?, disabled? }`; `label` and `hint` are React
  nodes.
- The trigger is a button (`aria-haspopup="listbox"`, `aria-expanded`) with the label
  of the current option, or the placeholder in the dim colour when the value is empty
  or unknown, and a chevron. It has the size, the border, the background and the focus
  ring of the other controls, and the width of the select it replaces.
- The list (`role="listbox"`) is a panel in the style of the composer's `.cmd-list`
  (`components/command.css`): `--glass`, the strong rim, the medium radius, that
  shadow. It opens under the trigger and above it when there is no room below; at
  least as wide as the trigger, at most 420 px; it scrolls from 280 px of height, and
  no card clips it.
- A row (`role="option"`, `aria-selected` true for the current value): the label, the
  optional dim second line, a check mark on the current value. Hover and the keyboard
  cursor are the ice colour at 12 percent. A disabled row is dim and the keyboard
  skips it.
- Mouse: the trigger toggles; a row picks and closes; a click outside closes; a
  resize or a scroll of the page closes.
- Keyboard: Enter, Space or ArrowDown on the trigger opens with the cursor on the
  current value, scrolled into view; Up, Down, Home and End move; Enter or Space
  pick; Escape and Tab close; the focus returns to the trigger. Typing letters jumps
  to the first enabled label that starts with them.
- No library and no new dependency.

A model option shows its id without a leading `models/` and the provider's name as
the dim second line, never `gemini::models/...`. An option for the current value that
is not in the list keeps its place. The reason of 7.6 ("provider switched off", "model
switched off", "unknown model") stands under the closed control and is the second
line of that row in the list.

The native selects of the other sections stay, with one rule in `index.css` next to
the `input, select, textarea` rules, so that their popups are readable in both themes
(`--glass-2` is light in the light theme):

```css
select option { background-color: var(--glass-2); color: var(--ink); }
```

## 8. ZCode references

ZCode is at `/home/virus/SystemInterface/ZCode/packages/`. Paths are under
`ui/src/settings/` unless stated. Take the pattern, not the code: KAgent has no
component library and no drag-and-drop library (a row is `draggable`, with up and down
buttons for the keyboard).

| File | Take |
|---|---|
| `model-provider-section/ProviderModelMetadataDialog.tsx` | the order of the dialog: switch, ID, context window, max output, advanced, footer (lines 128 to 376) |
| `model-provider-section/ProviderModelMetadata.ts` | `resolveProviderModelDraftCommit` (lines 108 to 325): the order of the checks, and "only what differs from the inherited value is saved" |
| `model-provider-section/ModelEditorAdvanced.tsx` | the block that is collapsed and opens on an error |
| `model-provider-section/ProviderModelReasoningLevelEditor.tsx`, `ProviderModelMetadataFields.tsx` | the ordered list of levels; the JSON field |
| `model-provider-section/ProviderFormControls.tsx` | `ModelRowInput` (line 16): what a row shows |
| `model-provider-section/SortableProviderModelList.tsx` | the row list and its ordering |
| `model-provider-section/InlineEditableProviderCard.tsx` | the card that saves on blur |
| `model-provider-section/SectionLayout.tsx`, `ProviderTemplatePicker.tsx` | the two columns; "Add provider" |
| `provider/src/config/model-config.ts` | `ModelConfigRules.resolve` (lines 387 to 423) |

## 9. Tests

- `crates/kagent-providers/src/rules.rs`: smart as today; manual resets the six fields
  and keeps `requestPatch`.
- `crates/kagent-providers/src/config.rs`: an entry without `smart` loads as `true`;
  an entry without `roles` loads as `["chat"]` and one with `[]` as no job.
- `crates/kagent-providers/src/discovery.rs`: the `models/` twin; the table of section
  3 (`models/gemini-3.8-flash-tts`, `models/gemini-3.8-live`, `models/lyria-3.5`,
  `text-embedding-3-large`, `some-chat-model`); new rows disabled at any list size;
  the correction of item 4, with the settings exception.
- `crates/kagent-providers/src/store.rs`: add, change, remove (refused while a role
  names the model), reorder; `usable`; `inherited` and `effective`.
- `crates/kagentd/src/api.rs`: every 400 and the 409 of section 4, `apiType`, the
  `custom` template without a name or without a base URL; `chatModelValid` with the
  provider off; the role check of `PUT /api/settings`.
- `crates/kagent-core/tests/agent_loop.rs` (the scripted model): a capacity error
  switches to the fallback as today; with `fallback_model_ref` empty the turn fails
  with the scripted error text; the three notices.
- `scripts/import-zcode-providers.py`: run it again over a copy taken before:
  `config/providers.builtin.json` does not change (the `custom` and `nvidia` extras and
  the two `nvidia` rules are in that file; all were added on 2026-11-27). When ZCode's
  source file has another revision than the 33 the file was generated from, keep the
  committed templates and say so in the decisions log of `IMPLEMENTATION.md`.

Scenarios: `scenarios.md` S40.

## 10. Documents of the same stage

`api.md` section 3 (the provider rows, `/api/models`, `chatModelValid`), `config.md`
section 2 (`fallbackModel`, `visionModel`, and the note on `options.maxOutputTokens`)
and section 4 (the entry fields), `ui-settings.md` section 3 (the `models` and `tools`
rows).
