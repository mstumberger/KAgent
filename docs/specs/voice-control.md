# Spec: settings by voice and the voice rules of v0.0.2 (S44)

Milestone: `docs/milestones/v0.0.2.md` section 0, item 2. Terms: `GLOSSARY.md`.
Depends on: `voice.md` (sections 1 and 12), `config.md` section 2, `tools.md`.

Why: on 2026-11-27 the owner asked by voice to "select the NVIDIA provider for the
Brain", to change KAgent's speaking voice and to turn it down. The agent had no tool for
any of it, could not see the providers, and said "noted" without doing anything.

## 1. The `settings` tool (group `agent`)

One tool, declared to chat and to the live model in every voice mode's tool set
(`live` and `auto` get it whatever `voice.toolGroups` holds; `routed` reaches it through
the Brain).

| Action | Assess | Input | What |
|---|---|---|---|
| `get` | RO | `{action: "get", key?: string}` | Without `key`: the settings as JSON, protected values masked (`"<set>"` or `"<empty>"`). With `key` (a dotted path, `voice.brain`): that value. |
| `models` | RO | `{action: "models"}` | Per provider (in order): id, name, enabled, and its enabled models as `ref`, roles, and which jobs they hold (`chatModel`, `fastModel`, `fallbackModel`, `visionModel`, `liveModel`). Also `voices`: the voice names of the live model's template. No keys. |
| `set` | MUT, `always_ask`, never rememberable | `{action: "set", changes: {<dotted key>: <value>, ...}}` (1 to 8 keys) | Applies all the changes as one patch, or none. Returns each key with its old and new value. |

- Summary of a `set` (the card, and the sentence of `voice.md` section 12):
  `change setting <key> from <old> to <new>`; several keys are joined with "; ".
  A model reference is said by its model id without `models/`.
- `set` goes through the same function as `PUT /api/settings`
  (`settings_apply::apply(st, patch, via)`, moved out of `api.rs::put_settings`):
  the same validation (roles of models, sections, ranges), the same side effects, the
  same `settingsChanged` event. A refused patch changes nothing and names the field.
- An unknown key is an error that lists the three nearest keys. `get` and `set` take
  the keys the API takes (`config.md`), camelCase.

### Protected keys

`set` refuses these, whoever asks, with "this setting is changed on the screen only":
`permissionMode`, `rules`, `toolOverrides`, `toolGroups`, `mcpToolGroups`, `privilege`,
`approvalTimeoutSec`, `hooks`, `telegram` (the whole section), `calendar.clientId`
and every key whose value lives in the keyring (provider keys, the Telegram token, the
search key, the calendar token). `get` masks the secrets and shows the rest.

The list is one constant (`PROTECTED_KEYS`, prefixes) with a test that walks every key
of `Settings::defaults()` and of each section: a new key must be put in the list or in
the "free" list of the test, so a new safety setting is never open by accident.

### In `full` mode

`set` is `always_ask`, which `full` lets through in chat (hooks.md). From voice it is
always asked (section 4). So: chat in `full` changes a setting without a card, voice
always asks out loud. The audit record of the call holds the old and new values.

## 2. KAgent's own voice and volume

- **Speaking voice**: `liveVoice` is a normal key (`settings set {"liveVoice": "Puck"}`).
  The live session watches `settingsChanged`: when `liveVoice` differs from the voice of
  its connection, it rolls the connection at the next `turnComplete` (the same path as
  `goAway`, `voice.md` section 4) with the new voice in the setup. If the server refuses
  the resumption handle with another voice, the session connects fresh and sends the
  history frames again. The model is told in the tool result: "the voice changes after
  this sentence".
- A voice name is checked against the template's `voices` (case-insensitive; the stored
  value is the template's spelling). Unknown: an error listing the voices.
- **Volume**: new key `voice.volume` (0 to 100, default 100). The page applies it live to
  the gain of the playback node (`web/src/voice/`), from `settingsChanged`, without a
  reconnect. It is KAgent's speech only, not the system volume (that stays `audio`).
  "Quieter" and "louder" are steps of 20 the model computes from `get`.
- **Voice mode**: `voice.brain` is a normal key; it applies to the next session, and the
  tool result says so.

## 3. Defaults and words

- `voice.brain` defaults to `live` (was `routed`). A settings file without the key reads
  as `live`. `voice.md` section 1 is updated: the table order is `live`, `auto`, `routed`.
- Settings -> Voice shows under `routed` and `auto`: "Every request waits for the Brain
  (<model id>)." The daemon keeps the last 5 durations of `think` (memory only); when
  their median is over 4 s the line turns into a warning: "The Brain took <n> s on
  average: `live` answers faster." (`GET /api/state` gets `voice.thinkMedianMs`.)
- The system prompt (`prompt.rs`) and the live instructions get one line: "The owner
  calls the chat model 'the Brain'. 'Set the Brain to X' means the setting `chatModel`."
- Both also get: "To see or change a setting, a model or your voice, use `settings`;
  never say a setting was changed before its result arrives."

## 4. Calls that come from voice

A call comes from voice when it is a live tool call (`turn_id` "live") or runs inside a
turn sent with `Via::Voice` (the Brain turn of `routed` and `auto`).

- For such a call `always_ask` asks in every permission mode, `full` included
  (`PermissionEngine::evaluate` takes the origin; `Agent` passes it from the turn).
  The origin has a third value since v0.0.3, `agent` (an outside coding agent through MCP):
  every non-read-only call asks, in every mode (`tools.md` section 0).
  Chat in `full` is unchanged.
- In `live` and `auto` the question is spoken (`voice.md` section 12). In a Brain turn
  the card appears and is answered by voice (`voice.md` section 2).
- This covers `terminal send` (the reason: on 2026-11-27 it typed toward the owner's
  only terminal without a question), `settings set`, and the calendar changes
  (`calendar.md`).

## 5. The owner's name

- `persona.address` empty means KAgent does not know the owner's name.
- While it is empty: the start greeting ends with "What should I call you? Tell me, or
  set it in Settings -> General."; the system prompt and the live instructions hold
  "You do not know the owner's name yet. Ask once, at the start of the conversation,
  and save the answer with `settings set {"persona.address": ...}`."
- `persona.address` is not protected; from voice the change is confirmed like any other
  ("change setting persona.address from empty to Marko").
- The stale task `t56cca8ba` of the owner's `tasks.json` is deleted by the owner (or by
  the coordinator at go-live, through `DELETE /api/tasks/:id`), not by code.

## 6. Files

```
crates/kagentd/src/settings_apply.rs     apply(st, patch, via): the body of put_settings
crates/kagentd/src/settings_tool.rs      the tool: get, models, set; PROTECTED_KEYS
crates/kagentd/src/settings_tool/tests.rs
crates/kagentd/src/api.rs                put_settings calls settings_apply
crates/kagentd/src/live/mod.rs           roll on a liveVoice change
crates/kagentd/src/live/brain.rs         think durations; tools(): settings always declared
crates/kagentd/src/sections.rs           VoiceSection: volume; brain default live
crates/kagent-core/src/permission.rs     evaluate(.., origin)
web/src/voice/                           gain from voice.volume
web/src/settings/sections/Voice.tsx      the Brain line, the volume slider
```

## 7. Tests (no network)

- `settings_tool`: `get` masks every secret; `models` lists refs and jobs and no key;
  `set` of two keys applies both or none; each protected key is refused; the key walk
  of section 1; an unknown key suggests; a model without the chat role is refused for
  `chatModel` with the route's message; the summary text.
- `permission`: `always_ask` from voice asks in `full`; from chat it does not.
- `live`: a `liveVoice` change rolls at `turnComplete` (fake Gemini: the second setup
  has the new voice); a refused handle falls back to a fresh connection with history.
- `scripts/test-live.py`: one new check, the roll with the new voice.
- Web, headless: the slider changes the gain; the warning line shows over 4 s.
- Scenarios (`scenarios.md` S44), with the owner by voice: "switch your voice to Puck",
  "a bit quieter", "set the Brain to Nemotron", "change the permission mode to
  read-only" (refused, said why).
