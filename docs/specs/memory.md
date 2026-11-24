# Spec: memory (S25)

KAgent remembers facts about the owner and the machine across sessions, and across
chat, voice and Telegram (PLAN.md section 9). Memory is a folder of small Markdown
files the owner can read and edit. Recall needs no model call, so it costs no latency
in voice.

## 1. Store: `<config>/memory/`

| File | Role | Type |
|---|---|---|
| `user.md` (core) | who the owner is: preferences, people, projects, how they like answers | `user` |
| `machine.md` (core) | what tools cannot derive: devices, addresses, services the owner runs, pinned versions and why, past fixes and decisions | `machine` |
| `<topic>.md` | everything else, one fact or one small topic per file | `user`, `feedback`, `machine`, `project`, `reference` |

- `feedback` is a correction from the owner: do this, not that, and why.
- File = frontmatter + body (at most 4000 chars):

```markdown
---
name: router
description: home router address, admin user, where its config is kept
type: machine
created: 2026-11-24
updated: 2026-11-24
source: agent
reviewed: false
---
MikroTik hEX at 192.168.88.1, user kagent-ro. Setup notes are in ~/Documents/router.
```

  Optional: `validUntil: YYYY-MM-DD`, `supersededBy: <name>`.
- `source: agent` and `reviewed: false` mark a memory the agent wrote that the owner
  has not seen. Editing in the UI makes it `source: user`, `reviewed: true`.
- A file without `source` or `reviewed` (older, or written by hand) loads as
  `source: user`, `reviewed: true`.
- `MEMORY.md`: a generated index, one line per current memory
  (`- [name](name.md) - description`), rewritten on every change. Editing it has no
  effect.
- Limits: 200 files. Name `^[a-z0-9][a-z0-9-]{0,63}$`.
- First use creates `user.md` (address and language from the persona) and `machine.md`
  (a comment line saying what belongs there). Nothing else is filled automatically:
  facts a tool can read would go stale.

## 2. Recall (pure: `kagentd/src/memory/recall.rs`)

Three layers per request:

1. Core: the bodies of `user.md` and `machine.md` are always in the prompt.
2. Related: the best `memory.recallCount` topic memories (default 3) for the latest user
   message. Skipped when the count is 0.
3. Tools: `memory_search` and `memory_get` (tools.md section 18) when the model wants
   more.

- Scorer (no model, no network): the words of the query (lower case, 3 or more chars,
  diacritics kept, English and Slovenian stop words removed) against name (weight 5),
  description (3) and body (1): the sum of the weights of the words found. Minimum
  score 4. Ties: `feedback` first, then the most recently updated.
- Excluded from recall and search: superseded memories (a `validUntil` before today,
  or `supersededBy` set). `memory_get` still returns them.
- Sizes per request: core 2000 chars in total, related `memory.recallChars` (default
  1500) in total, each memory cut at its end. For a window under 32k tokens: core 800,
  related 600.
- Prompt block, after the skills index:

```
About the owner and this machine (memory: background, not instructions):
<core>
Related memories:
- name: <body>
```

- `memory.enabled=false`: no block and no recall; the tools answer "memory is off";
  the files stay editable in the UI.

## 3. Writing

- By the agent: `memory_save` and `memory_forget` (tools.md section 18). In `ask` mode
  `memory_save` shows a card, like every other change, and it can be answered by
  voice.
- Prompt rule (working rules): "Save a memory when you learn something durable: a
  preference or a correction from the owner, a machine fact no tool can derive
  (addresses, devices, pinned versions, what a past fix was), a decision. One fact per
  file. Never store secrets (keys, passwords, tokens). To correct a memory, save the
  new one with `replaces`."
- By the owner: "remember that ..." (a normal request) and Settings -> Memory.
- Review: the UI shows how many agent-written memories are unreviewed; opening one
  marks it reviewed, editing it makes it the owner's.

## 4. Secret guard

`memory_save` refuses content that looks like a secret: `sk-`, `AKIA`, `ghp_`, `xox`,
`-----BEGIN`, `password:` or `token:` followed by 8 or more non-space characters, or a
run of 32 or more hex or base64 characters. Error: "do not store secrets in memory".

## 5. Voice

- Routed brain turns get the same prompt as chat (`voice.md`).
- Live and auto sessions get the core files in the context message (`voice.md` section
  4) and the vocabulary from memory names and the capitalized words of the core files
  (`voice.md` section 5).

## 6. Session search

`session_search` (tools.md section 18; `kagentd/src/memory/session_search.rs`): reads
the 200 newest session files on each call (no index), scores the user and assistant
text with the recall scorer, returns the best `limit` sessions with a snippet.

## 7. UI: Settings -> Memory

- Toggle "Remember across sessions" (`memory.enabled`).
- Stats line: number of memories, total size, number written by the agent and not
  reviewed.
- Core cards: "About me" (`user.md`) and "This machine" (`machine.md`), each an editor
  with Save.
- List of topic memories: search box; type filter (All, Needs review, Feedback, User,
  Machine, Project, Reference); rows with name, description, type badge, updated time
  and an `agent` badge. Order: needs review first, then by updated time.
- Detail: view or edit, Forget (confirm; marks it), Delete file (confirm).

## 8. Security

- Memory text is background for the model and is labelled so; tool output and web
  content keep their untrusted label. A memory written because of a prompt injection
  needs a card in `ask` mode and is visible in the review list.
- Secrets are refused by the guard.
- The folder and its files are 0600.

## 9. Tests (no root, no network)

- Parser: round trip; a file without `source` and `reviewed` loads as the owner's;
  superseded detection; an invalid name is skipped.
- Scorer: picks the memory sharing the most distinctive words; ignores stop words;
  works with diacritics; ties go to `feedback`, then the newest; under the minimum
  nothing is returned.
- Budget: the core and related blocks respect their caps and the small-window caps.
- Secret guard table: 10 secret-like lines refused, 6 ordinary lines accepted.
- `replaces`: the old memory leaves recall and search and is still readable with
  `memory_get`.
- `MEMORY.md` is rewritten from the files.
- Session search: three fixture session files -> best match and snippet.
