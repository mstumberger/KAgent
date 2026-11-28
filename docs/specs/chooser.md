# The model chooser

Status: v0.0.3. Plan: `plans/providers-and-quota.md` section 3 (local, not committed). One component picks a model
everywhere a model is picked: providers on the left, that provider's models on the right, a search box and filter chips.
The limits ring of each provider comes from `usage.md`.

## 1. Where it is used

| Place | File | What it picks | Rules it keeps |
|---|---|---|---|
| The composer's picker (S41) | `components/ModelMenu.tsx` | the conversation's model, then its effort | a "Default" row on top; a chosen model that is gone shows as a disabled current row with the reason; the effort slider and "Manage models" stay under the chooser |
| The widget's model sheet | `views/Overlay.tsx` (`ModelBody`) | the same | the same body; the ring only, no card |
| Settings -> Models -> Defaults | `settings/sections/models/Defaults.tsx` | `chatModel`, `fallbackModel`, `fastModel`, `visionModel` | models with role `chat` only; for `visionModel` only models whose resolved config has `supportsImage`; a "none" row (the setting's own wording); a value the list does not hold stays as the current row with its reason |
| The subagent profile form | `settings/subagents/ProfileEditor.tsx` | `model` | a "The chat model" row for the empty value; a model the providers no longer list stays selectable so it is seen |
| The task form | `views/tasks/TaskForm.tsx` | - | has no model field (a task uses the chat model); nothing to change |

Nothing the daemon refuses today becomes selectable, and nothing selectable today disappears. The one change: the vision
job offers only models that accept images (the daemon already refused the others on save).

## 2. Data

`GET /api/models` (`chat`, `fast`, `vision`, `live` lists) gives each model `{id, name, ref, providerId, providerName,
config}`. It gains `local` (the provider is local) and `free` (the model id ends in `-free`, or the provider reports a price
of 0: no provider reports one yet, so the id is the rule today). The provider rows of `GET /api/providers` gain `local`.

**Local** is decided by one function, `kagent_providers::is_local_url(base_url)`: a loopback, private or link-local IP
literal, or `localhost`. It moved out of the screen watch (which re-exports it) so the vision refusal, the provider view and
the chooser share the one rule. The page never decides it from an address.

## 3. The two levels

```
 Default                      Gemini 2.5 Flash      (a row above the columns, when the place has one)
 [ search: flash            ]  (Free) (Local) (Vision) (Tools) (Reasoning)
 +------------------------+-------------------------------------------+
 | Recent               5 | gemini-2.5-flash       1M  image  low high |
 | (ring) Gemini  local 12| gemini-2.5-pro         1M  image  free    |
 | NVIDIA        free   8 | ...                                       |
 +------------------------+-------------------------------------------+
```

- **Providers** (left): name, how many usable models match, the tags `local` and `free` (free: every shown model of it is
  free), the limits ring (`usage.md` section 7; none when the provider has no limit). "Recent" is the first entry when it has
  models.
- **Models** (right): the id as the owner reads it (`models/` removed), the context size (`1M`, `128k`), an image icon when
  it reads images, its reasoning levels, a "free" badge, and a check on the current one. A model that cannot be picked here
  is not listed.
- **Search** filters both levels: each space-separated term must appear in the model's name, id, provider name, or one of the
  words `free`, `local`, `image`, `vision`, `tools`, `reasoning` when the model has that. A provider whose name matches shows
  all its models. Providers with no match are hidden; the models column shows the active provider's matches.
- **Chips** (Free, Local, Vision, Tools, Reasoning) are on or off and combine with AND with each other and with the search.
- **Recent**: the last 5 models picked in any place, newest first, kept in this browser (`localStorage` key
  `kagent.chooser.recent`, a list of refs). A ref that is no longer in the list, or that the place does not offer, is left out.
  It is recorded on a pick of a model, not of "Default" or "none".
- The provider that holds the current value is active when the chooser opens (else the first).

## 4. Keyboard

The search box has the focus when the chooser opens and keeps it (`aria-activedescendant` names the cursor row), so typing
searches. Arrow Down and Up move the cursor in the active column; at the top of a column Up goes to the row above the
columns (Default or none) when there is one. Arrow Right in the providers moves to the models; Arrow Left in the models moves
back. Moving in the providers shows that provider's models at once. Enter picks the model under the cursor (in the providers
column, it moves to the models). Escape closes and returns the focus to the trigger (in a page's field, to the field). Home
and End go to the first and last row of the column. A click on a provider selects it, on a model picks it.

## 5. Files (`web/src/components/chooser/`)

`ModelChooser.tsx` (state, layout, keys), `ProviderColumn.tsx`, `ModelColumn.tsx`, `Chips.tsx`, `data.ts` (types, search,
chips, grouping, free, the context label), `recent.ts`, `ChooserField.tsx` (a trigger button that opens the chooser in a
popover, for the forms), `chooser.css`. The composer's trigger and menu stay in `ModelMenu.tsx`.

## 6. Out of scope

Routes and rotation, several credentials per provider, a workflow step's model, price classes.
