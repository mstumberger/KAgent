# TODO

Everything planned for KAgent that is not part of the current milestone
(`docs/milestones/v0.0.2.md`). One list; the order inside a section is the planned
order, the sections are not. Nothing here is started. The owner decides what goes
into the next milestone.

## The agent loop

Small changes to how the agent itself works, from the sessions of 2026-11-27. The
first three are done first.

- [x] Enforce, do not instruct: every "never" and "always" about an action in the
  system prompt and the voice instructions becomes a check in the daemon; the prompt
  only explains what the check does.
- [x] Tool results as messages to the model: each tool's results say what happened,
  what did not, and what to do next; an error is never worded as an empty result, and
  a result never reads as success before the effect exists.
- [x] Notes from the daemon into a running turn: the owner changed a setting or the
  Brain, a newer owner message replaces the current task, a file changed since it was
  read, the todo list has open items.
- [ ] Fewer tools in front of the model: a small core set always declared, the rest
  listed by name with one line each and loaded on request; enforced at execution.
- [ ] Long work lives on disk: a long turn keeps its task state in a file it re-reads
  after compaction or a restart.
- [ ] A fixed template for a worker's brief: goal, what done means, constraints, what
  to return.
- [ ] Recorded sessions as tests: a real transcript is replayed against a scripted
  model as a regression test.
- [ ] A list of tickets per project (key, state, priority, notes), beside the
  conversation's working todo list; "add to the todo" goes there; the board shows it
  as the project's backlog.
  Built: the store, `/api/tickets`, the tool `tickets` and the Tickets view (list, board, Mine now, detail pane) (v0.0.3, `specs/tickets.md`).
- [ ] Footprint: a hidden window is closed, scans follow package changes, helper
  programs become file reads, idle timers stretch, old data is pruned by settings;
  workers run in their own systemd scopes.
- [ ] A Disk page (a usage treemap and the largest files) with an optional, itemised
  cleanup: KAgent's own files, system and developer caches, crash dumps, Docker,
  unused models and virtual machines, old large files.

## Features

- [ ] Workers and workflows, with KAgent's own subagents in the three roles (planner,
  implementer, reviewer): the planner's tasks are implemented and reviewed in rounds
  (three, then the owner decides); workers and the orchestrator can ask and answer;
  a run parks on a usage limit and continues after the reset; a "Work" view (the
  board) shows projects, runs, tasks and the running and ended workers; a run that
  ends or needs the owner is announced on Telegram, the desktop, by voice and in
  the morning report.
- [ ] Telegram as a remote: only the owner (sender checked on every message and button,
  private chat only); a command menu (runs, sessions, model, status, alerts); one
  live message per run with buttons; voice notes in through any speech-to-text
  provider, and the answer as text and as a voice note.
- [ ] Voice providers: live voice (one speech-to-speech model, as today) or pipeline
  voice (speech-to-text, the Brain, text-to-speech), switchable; a Settings page
  with the providers by kind, free and local ones first, with links; local speech
  engines downloaded by KAgent, found on the machine, or reached as a local server.
- [ ] Coding agents (S35 to S39, `specs/coders.md`): Claude Code and Codex as agents of
  KAgent, then as workers of a workflow, with worktrees and stacked pull requests
  pushed by a run after the owner approved that step.
- [ ] GitHub Copilot as a provider, with the API format chosen per model (not decided).
- [ ] Agent integration: expose KAgent via CLI/MCP + publish thin SKILL.md wrapper
  (Agensi / other skill marketplaces). KAgent stays a local daemon; an agent can ask,
  only the owner approves, in KAgent's own surfaces.
  - A0 (a safety fix, first): a call from an outside agent is told apart from the
    owner's chat: every change asks in every permission mode, no remembered rule
    applies, and the session is not taken from the request.
  - A1 (MVP): agent tokens with their own routes; a `kagent` CLI with JSON output
    (`status`, `diagnose`, `tools`, `call`, `changes`); results marked as untrusted
    data; the skill, read-only.
  - A2: `propose`, `apply <id>`, `undo`: the approval card names the agent and shows
    the exact stored action; what an agent may request is a protected setting.
  - A3: the MCP server on the same routes, with a small purpose-built tool set.
  - A4: packaging: release binaries, an install script without sudo, a minimal
    install, then .deb and AUR; an `apiVersion` the skill checks.
  - A5: publish the skill.
- [ ] Usage and limits of the model in use: a limit is a bar with its percentage and its
  reset time (requests per minute, tokens a day, a plan's window, premium requests).
  Shown in chat (a ring on the model picker, a card with every limit of that
  provider, a line when a limit is nearly out) and in the providers list (a bar per
  provider, a Usage block on its card). From what each provider reports on its
  answers and its quota endpoint where it has one, plus KAgent's own count; a limit
  that is out skips to the fallback, and unattended work respects it.
  Built: counters per provider and model (`usage.sqlite`); limits from rate-limit headers (Anthropic, OpenAI style) and 429 bodies (Gemini, OpenAI, Anthropic) and the owner's known limits; the ring on the picker and per provider in the chooser, the card, a bar per provider row, the Usage block, the 90 % line, the tool `usage`. Not built: quota endpoints, skipping to the fallback, the reserve for unattended work.
- [ ] Import from other agents: a guide that scans Claude Code, ZCode and Copilot for
  conversations (grouped by folder, which become projects), skills, commands,
  instruction files and MCP servers; imported conversations can be continued; a
  second run only adds what is new. KAgent as an MCP client, with every tool of a
  server behind the permission engine.
- [ ] Providers and quota: one chooser for models everywhere, provider first, with
  search and filters (free, local, vision); routes (an ordered list of provider and
  model per job that moves on when a limit is out); several named credentials per
  provider; a Usage page with reports; OpenCode Zen with fetched models and private
  data held back from free models; Antigravity as a third coding agent through its
  official CLI.
  Built: the model chooser (providers first, search, chips, Recent, keyboard) in the composer's menu, the widget sheet, the Defaults card and the subagent profile form (the task form has no model field). Not built: routes and rotation, named credentials, the Usage page, OpenCode Zen, Antigravity.
- [ ] The shell: a router with back and forward (buttons, Alt+arrows, the mouse's side
  buttons); every page checked and fixed at five sizes up to 2560 px, with one set of
  layout rules and parts; conversations can be renamed, moved to a project, archived
  and deleted with undo.
- [ ] Tickets: a project's list and board in the sidebar (one list with the tickets a
  run creates); side-by-side diffs per ticket with comments that go back to the
  worker; seeing, correcting and interrupting a running worker; a change to a ticket
  proposes starting a worker or telling the one already on it.
  Built: tickets per project, the side-by-side diff view (journal changes and git ranges) and steering of today's subagents (see, tell, interrupt) (v0.0.3, `specs/tickets.md`, `diff.md`, `steering.md`); runs as tickets, comments, related work, align on change and the Jira mirror are not built.
- [ ] Models of every kind: each model declares what it takes and makes (text, image,
  audio, video, embedding); image, speech, listening, music and video are jobs with
  routes; four tools make media; every result is an artifact shown in chat; speech
  and images first.
- [ ] Time tracker (optional): KAgent records what the owner works on (the focused
  window, project and branch, browser tab, calls, agent and Copilot sessions), keeps
  the current ticket and asks when unsure, reminds him to stretch, rest his eyes and
  have lunch, and at 16:00 opens the day's worklog table for review. He logs to Jira
  by a click or a voice command; nothing else writes a worklog. Evidence also from
  git, GitHub, Jira and Confluence, calendars and Slack chats (read from Slack's
  local store, summarised by a local model only). A "Time" view with the day
  editor and the month report. Raw activity stays on the machine.
- [ ] Market watch (optional): a watchlist of stocks and coins with prices and news
  from feeds; each news item is clustered, matched to the watchlist, classified and
  given an urgency by editable rules (tell the owner now, soon, or in a digest), with
  its sources kept as evidence; the owner's own rules ("tell me when X is 20 % under
  its 90-day high") fire as alerts with measured history of that asset. Alerts on
  Telegram, the desktop and by voice; a Markets view; a block in the morning report.
  No orders are placed, no trading keys are held, and nothing is predicted.
- [ ] Local OCR: text read from images on this machine, on the processor, with
  positions (a small OCR model downloaded from the Local models page; the model, its
  licence and format are checked first, and it must read better than Tesseract in a
  measured comparison on screen text and Slovenian). A `read_text` tool for files, images and the
  screen; a turn's images go to the Brain as text first, with a setting to never
  upload pictures; screen watch checks text goals with OCR instead of the vision
  model; the click tool can find a label on the screen; scanned documents become text
  with their tables.
- [ ] OpenCode as a coding agent, in the same pattern as Claude Code, Codex and
  Antigravity: KAgent runs the owner's installed `opencode` command without a terminal,
  reads its progress, and uses it as a worker of a workflow. Its free models are
  usable this way, because the requests come from OpenCode's own client.
- [ ] Every completions API, and OpenCode as one provider: a Gemini `generateContent`
  adapter next to chat completions, responses and messages; an embeddings client for
  any provider; one "OpenCode" provider with one key whose models are fetched and each
  routed by the API its list names (replacing the six split entries); the cost each
  answer reports counted on the Usage card. Its free models answer only inside
  OpenCode's own client and are shown as such, not worked around.
  OpenRouter (already in the catalog) gets the same treatment: models, prices and
  free models from its own list, and its key's credit and limit on the Usage card.
