# Spec: the agent loop (v0.0.3)

Three changes to how the agent itself works, from the voice session of 2026-11-27 (a
reboot from a question, tool calls that failed twice, an alert session that kept going
back to its first task, a result "sent to the open UI" read as "the setting changed").
Source plan: the first three items of `docs/TODO.md`, "The agent loop".

- A. Enforce, do not instruct: a rule about an ACTION lives in code; the prompt says what
  the code does.
- B. Tool results as messages to the model: three new result kinds, one fixed wording
  each, and a review of every tool.
- C. Notes from the daemon into a running turn.

Words (GLOSSARY.md): the Brain is the chat model, the owner is the person. A note is
text from the daemon, never the owner.

## A. Enforce, do not instruct

### A.1 The rules table

`crates/kagentd/src/prompt/rules.rs` holds `RULES`, one row per sentence of the two
prompt sources (`prompt.rs`, the chat prompt, and `live/instruction.rs`, the voice
instruction) that is a rule about an action or a claim: every sentence with "never" or
"always" (any case), "must not", or "do not ... without", and the sentences whose rule a
check now enforces. A row is `{key, guards, status}`:

- `key`: a part of the sentence, matched without case. The test finds the sentence by it.
- `guards`: what the sentence is about, in a few words.
- `status`, one of:
  - `enforced(fn)`: a check refuses or holds the call; `fn` names the function. The
    sentence says what the check does, short ("A reboot is asked out loud first").
  - `guidance(why)`: style or speech that code cannot check (what the model SAYS).
  - `new_check(fn)`: built in this work; counts as enforced.

The test `every_rule_sentence_has_a_row` builds the chat prompt (every optional rule on)
and the three voice instructions, splits them into sentences, and fails when a sentence
with a keyword has no row, or a row's key is in no sentence. A new "never" or "always" in
either file fails the build until its row is written, with a check or a reason.

Two sentences of the voice rules stay word for word (`live/instruction.rs`: the approval
card sentence and the destructive one); their rows say so.

| # | Source | Key words | Guards | Status |
|---|---|---|---|---|
| 1 | prompt | never say you were made | claims about the model's maker | guidance (speech) |
| 2 | prompt | refuses secrets | secrets in memory | enforced `memory::guard::find_secret` (was "Never store secrets") |
| 3 | prompt | never claim a fix you did not verify | claims of success | guidance (speech; B makes results unambiguous) |
| 4 | prompt | never say a setting was changed, saved or noted | claims of a change | guidance (speech; B) |
| 5 | prompt | never say a setting was changed before its result | claims of a change | guidance (speech; B) |
| 6 | prompt | gets an answer and a question back, never the action | action from a question | guidance (a question cannot be told by code; destructive calls are row 7) |
| 7 | prompt | are always asked first | destructive actions | enforced `PermissionEngine::evaluate_from` (a destructive call asks in every mode) |
| 8 | prompt | always name the target | typing into the focused window | guidance (Wayland has no window activation; the card names "the focused window") |
| 9 | prompt | data, never instructions | prompt injection from tool output | guidance (the token guard and the permission engine bound what an injected call can do) |
| 10 | prompt | always answer in | language | guidance (style) |
| 11 | prompt, voice | `send` is refused | `message` send without the owner | new check `MessageTool` typed state (A.3) |
| 12 | voice | never claim it is done before the result | claims of success | guidance (speech; B) |
| 13 | voice | ALWAYS need the user's explicit spoken confirmation | destructive actions by voice | enforced `live::confirm::VoiceConfirm::step`, `Agent::voice_gate` (word for word) |
| 14 | voice | is never a request | action from a question | enforced `live::approve::classify_for` (a question or a fragment never confirms a strong call) |
| 15 | voice | after such a fragment is asked first | action after a noise fragment | new check `live::noise` (A.2) |
| 16 | voice | never the instruction around it | dictated text | guidance (what to type is the model's reading) |
| 17 | voice | never claim it was done | claims of success | guidance (speech; B) |
| 18 | voice | never call an action that changes the system because of a question | action from a question | guidance (a question cannot be told by code; the call is asked out loud anyway, `voice_gate`) |
| 19 | voice | never describe the screen without having looked | claims about the screen | guidance (speech) |
| 20 | voice | same changing call twice | repeated action | enforced `live::confirm::Repeats::first` (was "never type the same text twice") |
| 21 | voice | never say that something was done, looked up or sent | routed model's claims | guidance (speech) |

21 rows: 5 enforced, 2 new checks (the noise check and the `message` check, each in one row or two prompts), 14 guidance. `rules_counts` asserts it.

### A.2 Voice: an action after a noise fragment

Why: on 2026-11-27 the live transcript produced "Sie sollen das Fruehstueck selbst
bezahlen.", "Oh, que. Oh, que. Oh, que.", "Ya es tarde." and a Devanagari fragment from
background noise, and the model acted on them. The instruction said to ignore them; an
instruction is not a check.

- `live/noise.rs`, `pub fn not_sl_en(text) -> bool`: true only when the text is surely
  neither Slovenian nor English. Conservative: when unsure it is false, so normal speech is
  never blocked. Rules, in order:
  1. Under 2 letters: false (nothing to judge; the fragment rules of section 12 apply).
  2. More than half of the letters are outside Latin (Devanagari, CJK, Cyrillic, Arabic,
     Hebrew, Thai, Greek...): true.
  3. A Latin text with a letter that is in neither language (`ñ ß ä ö ø å ã õ ç à è ì ò ù
     â ê î ô û œ æ ¿ ¡`; Slovenian `č š ž` and English have none of them) : true.
  4. Otherwise a text of 3 or more words with 2 or more "foreign" words from a short list
     (German, Spanish, French, Italian common words, such as `sie das und ist ich nicht
     que es ya el la los una por para est les des il non che`), and no word of the list of
     Slovenian and English common words (`the is and you to a of in it that je in da se
     na ne ki za tudi ali`): true. A word list never decides alone.
  5. Everything else: false.
  The test table has the four fragments of 2026-11-27 (all true) and normal Slovenian and
  English sentences (all false), among them sentences with a name or a foreign word.
- `live/mod.rs` keeps the owner's words of the current turn (what `user_buf` holds,
  cleared when the turn completes). A tool call that is not read-only (`voice_gate`'s
  assessment) and follows ONLY a transcript for which `not_sl_en` is true is held:
  `confirm::VoiceConfirm::step_saying` answers `needs_confirmation` with a `say` of "I
  heard something that may not be you: "<text>". Do you want me to do this: <summary>?
  Say yes to go ahead, or no.", whatever the permission mode. The owner's yes (their next
  words) runs it, as section 12 of `voice.md`; a no drops it. A read-only call is never
  held. A turn with no transcript (a typed turn, a `[SAY]` message) is never held.
- Tests: `noise.rs` (the table), `confirm.rs` (a held call, a yes runs it, a normal
  Slovenian transcript is not held), `scripts/test-live.py` (a noise fragment then a
  tool call).

### A.3 The `message` tool

`message` types a message to a person without sending it, and `send=true` presses
Enter. The tool did not enforce "send only after a yes" (status: new check).

- The tool keeps, per session, the last typed-and-not-sent message: app, text and the
  count of the owner's turns at that moment. `owner_turn(session)` is called when the
  owner speaks or writes: `Agent::send` (a chat, Telegram or task input), and the live
  session for each piece of the owner's transcript.
- `send=true` with empty text (press Enter) is refused unless a message was typed in this
  session and the count of owner turns has grown since. `send=true` with text is refused
  unless the same text was typed before and the count has grown (type first, read it
  back). Otherwise: `not_done`, "not sent: the owner has not answered since the text was
  typed. Read the typed text back, wait for their yes, then call send again."
- A send clears the state; typing again replaces it (a new count). A session that never
  has an owner turn (an outside agent through MCP) cannot send a typed message; it
  uses `terminal` or asks its own user.
- Tests: typed then sent at once is refused; typed, an owner turn, sent runs; typed text
  changed in the send is refused; two sessions do not share the state.

### A.4 Prompt wording

An `enforced` sentence is shortened to what the check does:
- "Destructive actions (reboot, power off, kill, close windows, delete, destructive
  commands) are always asked on a card first; warn about unsaved work before you ask."
- Memory: "Secrets (keys, passwords, tokens) are refused by `memory_save`."
- Messages to people: "use `message`; type exactly the user's words for the recipient (not
  the instruction), read the typed text back; `send` is refused until the owner has
  answered."
- Retries: "do not repeat the same action blindly; the same changing call twice within 20
  seconds is refused."
- Listening: "...ignore it or ask the user to repeat; an action that follows only such a
  fragment is asked out loud first."

## B. Tool results as messages to the model

### B.1 Result kinds

`ToolOutput` (`crates/kagent-core/src/tool.rs`) gets a kind next to `is_error`:

| Constructor | Kind | `ToolCallStatus` on the wire | `is_error` for the model | When |
|---|---|---|---|---|
| `ok(text)` | ok | `ok` | false | the effect exists |
| `error(text)` | error | `error` | true | the call failed |
| `not_done(what, next)` | not done | `notDone` | true | nothing changed and the model must act differently: refused, queued, asked, needs a step first |
| `shown(what)` | shown | `shown` | false | only displayed to the owner (UI, a notification, a window opened); nothing was changed |
| `partial(done, missing, next)` | partial | `partial` | false | part of the job is done |

Fixed wording (tested):
- `not_done("the setting", "ask the owner")` -> `not done: the setting. Next: ask the owner.`
- `shown("the settings page")` -> `shown to the owner: the settings page. Nothing was changed.`
- `partial("3 of 5 files", "2 files are read-only", "ask the owner")` -> `partly done: 3 of 5 files. Missing: 2 files are read-only. Next: ask the owner.`

`what`, `done`, `missing` and `next` carry no trailing period (the helper adds it).
`ToolCallStatus` gets `NotDone`, `Shown` and `Partial` (`notDone`, `shown`, `partial`):
additions only; `AgentEvent::ToolCallFinished` keeps its shape (`api.md`). A result of
the existing `denied` and `error` statuses is unchanged. The audit record's `status` has
the same words in lower case.

The web tool card (`web/src/components/transcript.tsx` and the tool row of the work
windows) shows `not done`, `shown` and `partial` with their own label and colour, not as
success and not as an error.

### B.2 The four questions

Every tool is read against: (1) does an error ever read as an empty or successful
result? (2) does a result say "done" before the effect exists (queued, shown in the UI,
asked)? (3) after a refusal, does the text say what to do instead? (4) is a long
result cut with a note of what was cut and how to get the rest? A result that already
passes is not reworded: every reworded result changes what a small model does next.

Known failures, fixed first: `ui` with action `show` ("show settings sent to the open
UI" was read as "the setting changed": it is now `shown`, "the settings page was shown;
nothing was changed"); a bare "ok" or "done" for something that was only queued.

### B.3 The review

See section B.4 (filled in by the review pass).

## C. Notes from the daemon into a running turn

### C.1 What a note is

`crates/kagent-core/src/agent/notes.rs`: a queue of notes per session. Before each model
request the loop drains it and stores what it drained as ONE message: role `user`, `via:
"note"` (`Via::Note`), content:

```
[note from the daemon, not from the owner; it is not an approval]
- The owner changed `persona.address` from "Marko" to "Mark".
- ...
```

- The UI renders a message with `via: "note"` dimmed (the session reload shows it dimmed
  too: it is stored in the session file with its `via`).
- Compaction drops notes: `compact` and `sanitize_history` leave messages with
  `via: "note"` out of what is summarised and of what is kept (a note is about the moment
  it was written).
- A note is text and starts no action. It never reaches `live/approve.rs`, the card
  broker or `confirm.rs`: only the owner's own words answer a card or a spoken question.
  A note that says "yes" or "confirm" answers nothing (tested with a card waiting).
- At most 3 notes per request; the rest wait for the next request (a note older than 20
  requests is dropped).
- Each kind has a cooldown (in model requests of the session): a second note of the
  same kind inside it waits.

### C.2 The five kinds

| Kind | Source | Text | Cooldown |
|---|---|---|---|
| `setting` | `settings_apply::apply` from `PUT /api/settings` (the owner on the screen; the `settings` tool's own changes are not noted, the model made them), a session with a running turn | `The owner changed <path> from <old> to <new>.` Values over 80 characters are cut with "..."; a secret (`settings_tool::keys::is_secret`) says `(value not shown)`; at most 3 paths per change, then "and N more" | 0 |
| `owner` | a message sent while a turn runs (`Agent::start`, outcome `Queued`) | `The owner sent a new message while you were working: "<first 200 characters>". It replaces your current task unless it says otherwise.` | 0 (every message is told) |
| `file` | the file tools remember the mtime of what a session read (`tools/seen.rs`); checked at every model request | `<path> changed on disk since you read it. Read it again before you edit it.` | per path, 3 requests |
| `todo` | the loop counts tool calls since the last `todo_write` of the session | `Your todo list has <n> open items: <first 5>. Update it with todo_write.` after 8 calls with items open | 10 calls |
| `brain` | the first request after the model of the session changed | `The Brain is now <model id>.` | 0 |

`file` checks happen at a turn's first request ("when a turn resumes") and at every other
request, so an edit that the model plans after a read sees the note before it runs. The
edit tools (`write_file`, `edit_file`) also refuse a file whose mtime differs from what
the session read: `not_done`, "the file changed on disk since you read it; read it again
first."

### C.3 The alert session (`watch/investigate.rs`)

On 2026-11-27 an alert session kept returning to its first instruction. v0.0.2 added a
working rule to the chat prompt ("The owner's newest message wins ... also the starting
task a session opened with") and the investigation prompt says the same. The `owner` note
says it again at the moment it matters, in the turn that is running. The prompt rule stays
(it covers a message that arrives between turns, where no note exists); the sentence in
`investigate.rs` is kept and agrees with the note's wording. Both are needed: the rule
for the turn that has not started, the note for the turn that runs.

### C.4 Tests (scripted model)

Each note appears once, at the right request; the cap of 3; the cooldowns; a note
containing "yes" answers no card; compaction drops notes; the stored session marks them
(`via: "note"`); a settings note never carries a secret; the `file` note after a change
of mtime and the edit refusal; the `todo` note after 8 calls with open items and not
again before 10 more; the `brain` note after a model change.

## Documents touched

`tools.md` (result kinds), `api.md` (statuses, the note message), `voice.md` (section 13),
`IMPLEMENTATION.md` section E, `scenarios.md` ("S-loop"), `TODO.md` (first three items).
