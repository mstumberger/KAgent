# Spec: voice brain and live context (S26)

Where the thinking happens in voice today: only the Gemini live model. It receives the
chat system prompt, every tool at once, and the last 8 text messages cut to 300
characters each (`live/mod.rs`, `instruction`); its tool calls run through
`execute_call`. The chat model never takes part in a voice turn. That is why voice is
weaker than chat and loses the thread of a conversation.

Wire protocol: api.md section 5. This spec adds three brain modes, history seeding, a
context message, a custom vocabulary, a voice tool set, cancellation and speakable
replies.

## 1. Modes (`settings.voice.brain`)

| Mode | Who thinks | What the live model gets | Latency |
|---|---|---|---|
| `routed` (default) | The chat model, as a normal text turn in the session: history, compaction, memory, skills, all tools, approvals | One function, `think` (tools.md section 24) | The chat model's time: one to a few seconds |
| `live` | The live model itself (today's behaviour, improved below) | The tools of `voice.toolGroups` | Lowest |
| `auto` | The live model decides per utterance: small talk and direct commands itself, everything else through `think` | `think` plus the tools of `voice.toolGroups` | Mixed |

- `routed` needs a chat model. Without one, or when the chat model is a live model,
  the session runs in `live` and the status detail says "no chat model: the live model
  thinks".
- The mode is read when the live session connects. A change in Settings applies to the
  next session.

## 2. Routed mode (`kagentd/src/live/brain.rs`)

1. The live model gets a short dedicated instruction instead of the chat system
   prompt: persona and language; "You are the voice interface. For everything the user
   says that asks for anything, call `think` with the user's exact words in their
   language and say nothing else. When the result arrives, read `reply` aloud word for
   word in its language, adding nothing. If `status` is `handled` or `cancelled`, say
   nothing. If `failed`, say you could not reach your brain and why." You may say one
   short phrase before `think` ("Let me check"). The approval-card and destructive
   sentences of today's VOICE MODE text stay word for word: they are a second line of
   defence next to `live/approve.rs`.
2. On a `think` call the daemon does, in this order:
   - The text answers a pending approval card of this session (`approve::classify_for`
     on the call's text): resolve the card exactly as `voice_approvals` does and return
     `{"reply": "", "status": "handled"}`. The brain turn waiting for that card is
     still running; its reply arrives through its own `think` call.
   - The text is only a stop phrase (`approve::is_stop`: at most two words out of
     stop, cancel, ustavi, prekliči, "stop that", "cancel that", "never mind",
     pozabi): cancel the running brain turn and return `{"reply": "", "status":
     "cancelled"}`. "Stop the music" is three words and goes to the brain.
   - Otherwise send the text as a voice turn: `Agent::send(session, text, Via::Voice)`.
     While a brain turn runs the agent queues the text, so a second utterance steers
     the first. Then `wait_idle(session)`.
   - Result: `{"reply": speakable(result.text), "status": "ok" | "cancelled" |
     "failed"}`. On `failed` the reply is "Sorry, I could not think: <error>". A brain
     turn longer than 5 minutes is cancelled and reported `failed` ("that took too
     long").
3. The reply goes back as the `toolResponse` of the `think` call, which the model then
   speaks, as for every tool today.
4. Transcripts: in `routed` mode the text turn writes the session. The live transcript
   buffers feed the subtitles only; `flush_transcripts` is skipped.

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
- Group names are validated like `toolGroups` (400 on an unknown name).
- `live` keeps today's instruction (chat system prompt + VOICE MODE rules), without
  the "Recent conversation" block.

## 4. History and context (`kagentd/src/live/context.rs`)

- First connection of a session (no resumption handle): the setup adds
  `"historyConfig": {"initialHistoryInClientContent": true}`. After `setupComplete`
  the daemon sends the last 12 user and assistant messages of the session as
  `clientContent` turns with roles `user` and `model`, each message cut to 600 chars
  and 6000 chars in total, the last with `turnComplete: true`. The server does not
  answer these; the live model gets real conversation turns instead of 8 cut lines.
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
  only).
- `routed` sends neither: the text turn already has the context.

## 5. Vocabulary

- With `voice.vocabulary` on, the setup's `inputAudioTranscription` becomes
  `{"customVocabulary": [...]}` with at most 100 terms: the persona address, the
  project folders under `settings.desktop.projectRoots`, memory names, capitalized
  words of 3 or more chars from the memory core files, skill names.
- Terms are de-duplicated, 2 to 40 chars.

## 6. Cancellation of live tool calls (all modes)

- `ToolCallCancellation(ids)`: each named call that still runs gets its token
  cancelled, and its pending approval card is resolved as Deny "cancelled: the user
  interrupted". Today these ids are ignored and the tool runs on.

## 7. Speakable replies (`kagent_core::util::speakable(text, max_chars)`)

- Removes directive lines (`::name{...}`), code blocks (replaced by "The code is on
  screen."), link syntax (the text stays), heading and list markers, emphasis markers;
  joins table cells with commas; collapses whitespace.
- Cuts at `max_chars` (default 800) on a sentence boundary and appends "More is on
  screen." when something was cut.

## 8. Model catalog

- `scripts/import-zcode-providers.py`: the `gemini` template lists two live models,
  `gemini-3.8-live` and `gemini-3.8-live-extended-thinking`. The Voice section shows
  "slower, more thorough" next to the second.

## 9. UI

- Settings -> Voice: "Thinking" radio with `Chat model (smarter, a little slower)`,
  `Live model (fastest)`, `Live model decides`; "Tools for the live model" chips (the
  groups of `/api/tools`); toggles "Conversation context" and "Custom vocabulary".
- Voice tab: the status line shows "thinking" while a brain turn runs (from the `think`
  tool event); the transcript shows the spoken reply.

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
  OpenAI-compatible `/v1/chat/completions` with a fixed answer.
  1. `routed`: audio in, the fake answers with a `think` tool call -> a `toolResponse`
     with `status: ok` and the fixed answer as `reply`; the session holds the turn.
  2. A stop phrase -> `status: cancelled`.
  3. A pending card answered by voice -> `status: handled`, the card is resolved.
  4. `live`: the first setup has `historyConfig`; the fake receives the history turns
     and the context frame.
  5. `toolCallCancellation` for a call waiting on a card -> the card is denied.
  6. The transcript is not stored twice in `routed` mode.

## 11. Owner check (manual, once)

Talk to the widget in `routed` mode: a question that needs a tool ("what is my CPU
temperature"), a follow-up that depends on the first answer, and "stop". The reply is
spoken word for word; barge-in keeps the thinking running; the follow-up is answered
with the earlier context.
