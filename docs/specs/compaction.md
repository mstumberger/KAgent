# Spec: context compaction and session digest (S22)

Today a long conversation loses its oldest messages: `context::fit_to_budget` drops
them at user-message boundaries and tells the model only how many were omitted. The
model then answers without what was said earlier. Compaction replaces the dropped part
with a summary that is kept with the session.

## 1. When

- Where `fit_to_budget` runs today: before each model request in `turn_loop`.
- Soft line: when the request's estimate passes `compaction.thresholdPct` percent of
  the window (default 80), the oldest whole user turns are summarized back to
  `thresholdPct - 20` percent (60 by default), so the next compaction does not fall
  due on the following turn.
- Hard line: when `fit_to_budget` would still drop messages after that, everything up
  to its cut point is summarized too.
- Summarize when the model's window is 32k tokens or more, or `fastModel` is set and
  resolves. Otherwise (a small local model, no fast model) there is no soft line, and
  the hard line keeps today's behaviour, trim with the note: a 7B model summarizing
  inside a 16k window is slower and worse.
- `compaction.enabled=false`: always trim.
- Capacity error from the provider mid-turn: compact once and retry the same model
  once; only then switch to `fallbackModel` (today the switch comes at once).

## 2. What is summarized

- Everything before the cut point that `fit_to_budget` chooses, plus the previous
  summary when there is one.
- The cut never separates an assistant tool call from its results (existing rule).

## 3. Summary request (`kagent-core/src/compact.rs`)

- Model: `fastModel` when set and resolvable, else the chat model. No tools.
  `max_output_tokens` 1500.
- System text of the request:
  "Summarize the conversation so far for an assistant who continues it without the
  original messages. Keep, in third person and in the language of the conversation:
  facts established (names, paths, numbers, commands and their results), decisions and
  their reasons, what the user asked for and corrected, what was tried and what
  happened, open tasks and todo items, approvals given or refused. Leave out greetings
  and repetition. At most 1200 words. Output the summary only."
- Messages: the summary so far as a first user message ("Summary of the earlier
  conversation:"), then the messages to compact, each tool result cut to 1500 chars.
- Failure (error, empty text, cancel): trim instead, log a warning. A failed summary
  never fails the turn.

## 4. Storage and use

- `SessionMeta.summary` and `summaryUpto` (config.md section 11), written with
  `save_meta` before the request that uses them. `summaryUpto` is the number of stored
  messages the summary covers.
- Every later request is: system, one synthetic user message "Conversation so far
  (summary of the first N messages):" plus the summary, then the stored messages from
  index N on, trimmed by `fit_to_budget` when still too long. The synthetic message is
  never written to the session file.
- The summary counts toward the budget (chars/3). For a window under 64k tokens a
  summary is cut at 4000 chars, at a sentence boundary.
- Manual: `POST /api/sessions/:id/compact` and `/compact` in chat summarize everything
  before the last user message and return `{summary, keptMessages}`. Fewer than 4
  stored messages -> 400. `/compact` is a built-in command, not a skill.
- `fastModel` gets its first consumer here. The picker for it comes with S19 (Models,
  "Roles").

## 5. Digest for other features

- `SessionStore::digest(id, max_chars)`: the summary when there is one, else the last
  user and assistant messages, each cut to 600 chars, oldest first. Users: voice
  (`voice.md` section 4) and subagent hand-offs (`agents.md` section 2).
- Plan mode re-injects the approved plan after compaction (`plan-todo.md` section 2).

## 6. Token estimate

- The chars/3 estimate stays. One calibration is added: after each model response the
  loop records the response's prompt token count (`ModelResponse.usage`) as the
  session's `last_input_tokens`. The trigger compares `max(estimate, last_input_tokens)`
  with the window. A provider that reports no usage leaves the estimate alone.

## 7. Tests (no root, no network)

- `compact.rs`: `should_compact` table (79 percent of the window -> nothing; 81
  percent -> compact back to 60 percent; a small window without a fast model -> trim; a
  small window with a fast model -> summarize; disabled -> trim; a cut never separates
  a tool call from its results); the request builder puts the old summary first and
  cuts tool results; a long summary is cut at a sentence.
- `session.rs`: `summary` and `summaryUpto` round trip; old meta loads.
- `context.rs`: with a summary the request is [summary message] + messages from N;
  without one it is unchanged.
- `tests/agent_loop.rs` (scripted model): a history that overflows a 32k window causes
  one summarize call and a request that starts with the summary message; a failing
  summarize call falls back to the note; a capacity error compacts before the fallback
  model is used.
- API: manual compact returns the summary; under 4 messages -> 400.
