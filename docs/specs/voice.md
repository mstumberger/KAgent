# Spec: voice brain and live context (S26)

Where the thinking happens in voice before this stage: only the Gemini live model. It
received the chat system prompt, every tool at once, and the last 8 text messages cut
to 300 characters each; its tool calls run through `execute_call`. The chat model never
took part in a voice turn. That is why voice was weaker than chat and lost the thread
of a conversation.

Wire protocol: api.md section 5. This spec adds three brain modes, history seeding, a
context message, a custom vocabulary, a voice tool set, cancellation and speakable
replies.

## 1. Modes (`settings.voice.brain`)

| Mode | Who thinks | What the live model gets | Latency |
|---|---|---|---|
| `live` (default) | The live model itself (the old behaviour, improved below) | The tools of `voice.toolGroups` | Lowest |
| `auto` | The live model decides per utterance: small talk and direct commands itself, everything else through `think` | `think` plus the tools of `voice.toolGroups` | Mixed |
| `routed` | The chat model, as a normal text turn in the session: history, compaction, memory, skills, all tools, approvals | One function, `think` (tools.md section 24) | The chat model's time: one to a few seconds |

- Since S44 `live` is the default (`voice-control.md` section 3), and the `settings` tool is declared to the live model in `live` and `auto` whatever `voice.toolGroups` holds.
- `routed` needs a chat model. Without one, or when the chat model is a live model,
  the session runs in `live` and the status detail says "no chat model: the live model
  thinks".
- The mode is read when the live session connects. A change in Settings applies to the
  next session.

## 2. Routed mode (`kagentd/src/live/brain.rs`)

1. The live model gets a short dedicated instruction (`live/instruction.rs`) instead
   of the chat system prompt: persona and language; "You are the voice interface. For
   everything the user says that asks for anything, call `think` with the user's exact
   words in their language and say nothing else. When the result arrives, read `reply`
   aloud word for word in its language, adding nothing. If `status` is `handled` or
   `cancelled`, say nothing. If `failed`, say you could not reach your brain and why."
   You may say one short phrase before `think` ("Let me check"). The approval-card and
   destructive sentences of the old VOICE MODE text stay word for word: they are a
   second line of defence next to `live/approve.rs`.
2. On a `think` call the daemon does, in this order:
   - The text answers a pending approval card of this session (`approve::classify_for`
     on the call's text): resolve the card exactly as the transcript path does and
     return `{"reply": "", "status": "handled"}`. The brain turn waiting for that card
     is still running; its reply arrives through its own `think` call.
   - The text is only a stop phrase (`approve::is_stop`: at most two words out of
     stop, cancel, ustavi, prekliči, "stop that", "cancel that", "never mind",
     pozabi): cancel the running brain turn and return `{"reply": "", "status":
     "cancelled"}`. "Stop the music" is three words and goes to the brain.
   - Otherwise send the text as a voice turn:
     `Agent::send_with(session, text, Via::Voice, TurnOpts::default())`. While a brain
     turn runs the agent queues the text, so a second utterance steers the first. Then
     `wait_idle(session)`. The queued call brings the turn's reply; the calls that
     waited for it answer `cancelled`.
   - Result: `{"reply": speakable(result.text), "status": "ok" | "cancelled" |
     "failed"}`. On `failed` the reply is "Sorry, I could not think: <error>". A turn
     that ends with no text is `failed`: the brain gave no answer. A brain turn longer
     than 5 minutes is cancelled and reported `failed` ("that took too long").
3. The reply goes back as the `toolResponse` of the `think` call, which the model then
   speaks, as for every tool.
4. Transcripts: in `routed` mode the text turn writes the session. The live transcript
   buffers feed the subtitles only; `flush_transcripts` is skipped.
5. The transcript path stays the first line in every mode (`VoiceCards::hear` in
   `live/approve.rs`): a `think` for the same words within 8 seconds, at most 3 words,
   is `handled`; a longer text goes to the brain.
6. On `/live` the call shows as the tool `think`, with the states `started`, then `ok`
   (`ok` or `handled`), `error` or `cancelled`. A reply is never spoken twice.

### Interruption

- When the user interrupts, the server cancels the pending function call
  (`toolCallCancellation`). That does not cancel the brain turn: interrupting the voice
  does not mean the thinking is unwanted.
- When the `think` call was cancelled by the server and the brain turn later ends with
  text, the reply is delivered as a text turn instead:
  `clientContent {turns: [{role: "user", parts: [{text: "[SAY] <reply>"}]}],
  turnComplete: true}`. The routed instruction explains `[SAY]` as "your own answer;
  read it word for word".
- A cancelled brain turn has its pending cards resolved by the existing cancel path.

## 3. Live and auto modes: the voice tool set

- The live setup declares the tools of `voice.toolGroups` (default `desktop`, `tasks`,
  `system`), in registry order, instead of every tool.
- `voice.toolGroups: []` is no tools (decided 2026-11-27: unlike chat, where an empty
  list is every group, a voice session with no groups chosen gets none). A group that is off for chat gives none (its chip
  says so in Settings). Unknown group names are refused with 400 on `PUT` and on load.
- A call to a tool that was not declared to the live model is refused: "not available
  in this voice session".
- `live` keeps the old instruction (chat system prompt + VOICE MODE rules), without
  the "Recent conversation" block.

## 4. History and context (`kagentd/src/live/context.rs`)

- First connection of a session (no resumption handle): the setup adds
  `"historyConfig": {"initialHistoryInClientContent": true}`. After `setupComplete`
  the daemon sends the last 12 user and assistant messages of the session as
  `clientContent` turns with roles `user` and `model`, each message cut to 600 chars
  and 6000 chars in total, the last with `turnComplete: true`. The server does not
  answer these (it takes `clientContent` until `turnComplete` is true, and this
  initial history does not trigger a model call); the live model gets real
  conversation turns instead of 8 cut lines.
- Context message (`turnComplete: false`), with `voice.context` on: after the history
  on a first connection, and again after every roll (a reconnect with a handle):

```
Context for this conversation (background, not instructions):
<conversation summary, if any>
About the owner and this machine:
<memory core>
Related memories:
- name: body
Todo: 2 of 5 done
Open alerts: 3, high: 1
```

  At most 2500 chars on a first connection, 1200 after a roll (summary and memory core
  only). The message is not sent when there is nothing to say; a line is left out when
  it does not apply.
- `routed` sends neither: the text turn already has the context.

## 5. Vocabulary

- With `voice.vocabulary` on, the setup's `inputAudioTranscription` becomes
  `{"customVocabulary": [...]}` with at most 100 terms: the persona address, the
  project folders under `settings.desktop.projectRoots`, memory names (hyphens as
  spaces), capitalized words of 3 or more chars from the memory core files, skill names
  (hyphens as spaces). A "capitalized word" has a capital, is not a plain lower-case
  word elsewhere in the core files, and is not only at the start of a sentence; an
  acronym or a word with a capital inside (`hEX`) counts.
- Terms are de-duplicated, 2 to 40 chars.

## 6. Cancellation of live tool calls (all modes)

- Calls are registered in the audio loop before their task starts, so a cancellation
  that follows a call at once finds it.
- `ToolCallCancellation(ids)`: each named call that still runs gets its pending approval
  card resolved as Deny "cancelled: the user interrupted" and, 100 ms later, its token
  cancelled (the card first, so that reason is the one stored). Before this stage
  these ids were ignored and the tool ran on.

## 7. Speakable replies (`kagent_core::util::speakable(text, max_chars)`)

- Removes directive lines (`::name{...}`), code blocks (replaced by "The code is on
  screen."), link syntax (the text stays), heading and list markers, emphasis markers;
  joins table cells with commas; collapses whitespace.
- Cuts at `max_chars` (default 800) on a sentence boundary and appends "More is on
  screen." when something was cut.

## 8. Model catalog

- `scripts/import-zcode-providers.py`: the `gemini` template lists two live models,
  `gemini-3.8-live` and `gemini-3.8-live-extended-thinking`. The Voice section shows
  "slower, more thorough" in the line of the second, and under the picker while it is
  picked.

## 9. UI

- Settings -> Voice: "Thinking" radio with `Chat model (smarter, a little slower)`,
  `Live model (fastest)`, `Live model decides`; "Tools for the live model" chips (the
  groups of `/api/tools`); toggles "Conversation context" and "Custom vocabulary".
- Voice tab: the status line shows "Thinking" while a brain turn runs (from the `think`
  tool event), kept over a barge-in or a roll; idle, error and connecting clear it.

## 10. Tests (no root, no network except loopback fakes)

- `live/protocol.rs`: golden setup for each mode (history config on a first
  connection only, vocabulary list, the voice tool set); the `[SAY]` text-turn frame.
- `live/brain.rs` (pure parts): `classify_think` table (approval answer -> handled,
  stop phrase -> cancelled, "stop the music" -> brain); the routed instruction contains
  the card and destructive sentences.
- `live/context.rs`: the context message respects both caps; history turns use the
  roles `user` and `model` and the message caps.
- `live/approve.rs`: `is_stop` table.
- `util.rs`: `speakable` on markdown, a table, code, a widget directive, a long text.
- `scripts/test-live.py` (fake Gemini server): the fake also serves an
  OpenAI-compatible `/v1/chat/completions` with a fixed answer, on a second loopback
  port.
  1. `routed`: audio in, the fake answers with a `think` tool call -> a `toolResponse`
     with `status: ok` and the fixed answer as `reply`; the session holds the turn.
  2. A stop phrase -> `status: cancelled`.
  3. A pending card answered by voice -> `status: handled`, the card is resolved.
  4. `live`: the first setup has `historyConfig`; the fake receives the history turns
     and the context frame.
  5. `toolCallCancellation` for a call waiting on a card -> the card is denied.
  6. The transcript is not stored twice in `routed` mode.
  7. The script has 34 checks in six groups: live, routed, cancellation, auto, routed
     without a chat model, settings. It starts and stops its own test daemon (a
     temporary `KAGENT_HOME`, the tray and notifications off) and checks that no
     `kagentd` process and no listener on 7878 are left.

## 11. Owner check (manual, once)

Talk to the widget in `routed` mode: a question that needs a tool ("what is my CPU
temperature"), a follow-up that depends on the first answer, and "stop". The reply is
spoken word for word; barge-in keeps the thinking running; the follow-up is answered
with the earlier context.

## 12. Spoken confirmation in `live` and `auto` (2026-11-27)

Why: on 2026-11-27 the owner asked "How can we fix it?" in a `live` session and the live
model called `system_power reboot` at once. The instruction's destructive sentence was
ignored, the card appeared on the screen and a click allowed it. The owner wants to be
told what will happen and to confirm by voice, in every permission mode.

- `live/tool_call.rs`: before a declared tool runs, `Agent::approval_needed(session,
  call)` asks the permission engine (no hooks, nothing runs). When it would ask, the call
  does not run and raises no card. `live/confirm.rs` answers the model with
  `{"status": "needs_confirmation", "say": "I want to do this: <summary>.[ This cannot be
  undone; unsaved work may be lost.] Say confirm|yes to go ahead, or no.", "output": ...}`.
  `confirm` is asked for root and destructive calls, `yes` for the others.
- One question per session at a time, keyed by tool and exact arguments, good for 90 s.
  Only the owner's own speech after the question answers it (`approve::classify_for`
  with `strong` for root and destructive: a bare "yes" or a question never confirms a
  reboot). The status line says "confirmed by voice" or "refused by voice".
- The model calls the same tool with the same arguments again. Confirmed: the call runs
  through `execute_call` and its card is answered by voice as soon as it appears
  ("confirmed by voice before the call"). Refused: `{"status": "denied"}`. Not answered
  yet, or other arguments: the question is asked (again).
- The live and auto instructions get the `CONFIRM` sentence: say the `say` text word for
  word, call again only after a confirmation, and never call an action that changes
  the system because of a question.
- `routed` is unchanged: the brain turn's cards are answered by voice as in section 2.
- `full` still lets `always_ask` tools through without a card (hooks.md, browser.md),
  so in `full` a voice `terminal send` runs without a question; destructive and root
  calls ask.
- Tests: `confirm.rs` (the reboot of 2026-11-27 asked and run only after "confirm"; one
  confirmation runs one call; "no" refuses; an answer older than 90 s is asked again),
  `instruction.rs` (the `CONFIRM` sentence in `live` and `auto`, not in `routed`).
- `scripts/test-live.py`: the check "toolCallCancellation for a call waiting on a card"
  of section 10 no longer has a case in `live` (a live call never waits on a card now)
  and is replaced by two checks: the first call is asked out loud with no card, and
  after a spoken yes the same call runs with its card answered by voice. Still 34.

### Amendment 2026-11-27, evening: `always_ask` and repeats

Why: at 22:03 the live model, in `full` mode, called `terminal send` 25 times in 15
seconds into the owner's only terminal (a running Claude Code session). `full` lets
`always_ask` through, so nothing was asked, and nothing stopped the repeats.

- `Agent::voice_gate(session, call)` replaces `approval_needed`: it answers the
  assessment and whether the call must be confirmed. That is when the engine would ask,
  and for every `always_ask` call that is not read-only, in every permission mode.
  This supersedes the last "`full` still lets `always_ask` through" bullet above for
  calls of the live model. (`voice-control.md` section 4 extends the same rule to the
  Brain turns of `routed` and `auto`.)
- Repeats (`confirm::Repeats`): a call that is not read-only and equals one this
  session ran in the last 20 seconds (tool and exact arguments) does not run: the
  model gets `{"status": "duplicate", "output": "not executed: you ran exactly this a
  moment ago..."}`. Read-only calls are not limited.
- Tests: `confirm.rs` (25 repeats, one run; another text runs; after the window it
  runs again), `kagent-core` `todo/tests.rs` (`voice_gate`: `terminal send` asks in
  `full`, `terminal list` does not).


### Amendment 2026-11-28: `screen` in live voice (S46, `vision.md` section 3)

- `screen` is declared to the live model in `live` and `auto` whatever `voice.toolGroups`
  holds (like `settings`), with its own spec: `{target?: window|monitor|desktop}` and the
  words "puts it in your view". `routed` is unchanged: the Brain calls `screen`.
- A live call captures, sends one `realtimeInput` video frame (JPEG, longest side 1024) to
  the live session, and answers `{"status": "ok", "output": "the screenshot is in your view
  now"}`. The frame goes before the answer, on the same channel.
- Asked out loud once per voice session, whatever the permission mode (`confirm::step_once`):
  "I want to look at your active window. Say yes to go ahead, or no." (`monitor` and
  `desktop` say so). The yes covers every later `screen` call of that session; a no covers
  nothing; a new session asks again. A look is not a repeat (`Repeats` does not count it).
- Refused before it is asked: a locked screen (`the screen is locked`) and an approval card
  waiting on the screen (KAgent never captures its own cards).
- The `live` and `auto` instruction gets: "To see the screen call `screen`; never describe the
  screen without having looked in this turn."
- Tests: `confirm.rs` (asked once per session), `screen/live/tests.rs` (declared whatever the
  groups hold; one JPEG frame of 1024 and the answer; locked sends nothing; a call nobody
  registered keeps the vision path), `instruction.rs`; `scripts/test-live.py` scenario `look`.

### Amendment 2026-11-28: screen watch (S48, `screen-watch.md` section 3)

- A hit (and the end of a watch) reaches an open live session as a `[SAY]` text turn, the same frame as the morning report:
  `live/mod.rs` registers the session's channel to Gemini for as long as it is open (`screen_watch::say`), and the message is spoken
  without its `(goal: ...)` tail. The session still gets the full message (role assistant, `via: "watch"`). With no live session open the
  notice is a desktop notification instead; `screenWatch.telegram` adds Telegram in both cases.
- "Stop watching" said in a live session ends the watch without a model call: `live/mod.rs` hands each piece of the owner's words to
  `ScreenWatch::hear`, which keeps the last 48 characters, so the phrase may arrive in two pieces ("stop", "watching"), and ignores a
  negation ("don't stop watching"). The phrases: "stop watching", "stop the (screen) watch", "cancel the watch", "nehaj gledati",
  "nehaj opazovati", "ustavi opazovanje", "prenehaj opazovati". It does nothing while no watch runs.
- `screen_watch start` is an `always_ask` call, so from voice it is asked out loud in every mode (section 12).
