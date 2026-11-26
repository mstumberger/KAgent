# Spec: coding agents (S35-S39)

KAgent's chat model is often a small local one; Claude Code and Codex are built for
programming. A **coder run** hands one job, in one project folder, to one of those tools:
KAgent starts it as a child process in the tool's non-interactive mode, follows what it
does, and brings its report back to the agent that started it.

The design is taken from Omnius, a terminal application for the same tools built on a
tmux server. Ported is what it knows: how each tool is started, resumed and run (section
1, from its `src/sessions/tool.rs`), its launch profiles (section 3), its rules for a
launch (the job cap, the rate cap, the report of a run that exits at once), its session
index (section 7), linked worktrees (section 8) and automation (section 9). Not ported
is its tmux machinery (session tags, hooks for the activity state, the paste bridge,
screen captures): the target machine has no tmux, and a non-interactive run returns a
structured result where a pane returns a screen. Section 10 has the rest. No code is
copied from Omnius (it has no license); the code here is written fresh.

## 0. Terms

- **Tool**: the program KAgent starts, `claude` (Claude Code) or `codex` (Codex CLI).
  Section 1 holds what differs between them.
- **Profile**: what a run may do and with which options (section 3). The built-ins are
  `<tool>-plan`, `<tool>-edit` and `<tool>-full`; files of the owner come with S36.
- **Run**: one started process. It is a child of the S31 registry (`agents.md`): a
  session with origin `subagent`, the same queue, limit, states, `subagentUpdated`
  events and Agents card, the same `agent_wait`, `agent_stop` and `agent_list`, and the
  same message to the parent when it ends. Its id is the id of its session.
- **Job**: the text the tool gets. **Report**: what the parent is told when the run ends.

## 1. Tools (`kagentd/src/coders/claude.rs`, `coders/codex.rs`)

| | `claude` | `codex` |
|---|---|---|
| Binary | `coders.claudeBinary`, else `claude` on `PATH` | `coders.codexBinary`, else `codex` on `PATH` |
| Version | `claude --version` | `codex --version` |
| A run | `claude -p --output-format stream-json --verbose --session-id <run id> --settings <json>`, then the flags of the profile; the job on stdin | `codex exec --json -`, then the flags of the profile; the job on stdin |
| Resume (S37) | the line of a run with `--resume <run id>` in place of `--session-id` | `codex exec --json resume <thread id>`, then the job |
| Its own session id | the run id KAgent passes | `thread.started.thread_id` |
| Lines it prints | JSON: `system`, `assistant`, `user`, `result` | JSON: `thread.started`, `turn.*`, `item.*`, `error` |

- The `claude` column was read from `claude --help` of 2.1.332 and from the headless-mode
  documentation on 2026-11-27; no run was made. S35 checks each row with the real
  program (section 11) and corrects this table where a check differs. The `codex` column
  follows the non-interactive documentation of 2026-11-27 (`codex exec` runs in a
  read-only sandbox unless `--sandbox` says otherwise); S36 records real output first
  and pins it with fixtures before it writes the adapter.
- The argv is a vector and runs without a shell. The job goes on stdin, never on the
  command line (the process list is readable by every program).
- The environment of a run is the daemon's, without the variables that tell Claude Code
  that it runs inside another Claude Code session (`CLAUDECODE`,
  `CLAUDE_CODE_ENTRYPOINT`), plus the `env` of the profile (S36). With
  `coders.loginShell` the binary runs through a login shell instead,
  `$SHELL -l -c 'exec "$0" "$@"' <binary> <args...>`, which is the environment of the
  owner's terminal.
- A binary that is not there: `coder_run` answers an error with an install hint and
  starts nothing; the page says "not installed". KAgent never installs a tool.

## 2. Runs (`kagent-core/src/agent/children/external.rs`, `kagentd/src/coders/runner.rs`)

- **Start** (`Agent::start_coder`), as `start_child` of `agents.md` section 2: the
  limits of the description and the prompt, a parent that exists and is not a subagent
  (the depth limit: a subagent cannot start a coder either), the digest of the parent
  before the job, and a child session with origin `subagent` and the description as its
  title. The run takes a slot of the same queue and limit (`subagents.maxConcurrent`):
  it starts at once with one, else it waits as `queued`.
- **The launcher.** The core never starts a process. The daemon registers a
  `CoderLauncher` (`start`, `stop`) with the agent, as the profiles reach the tools
  through `AgentAccess`. A start that fails at once ends the child as `failed` and tells
  the parent, as a subagent whose turn cannot start is ended. The core also lists the
  coder runs that wait or run (`GET /api/coders`).
- **The process**: its own process group; stdout read line by line; stderr kept for the
  report of a failure (its last 2000 characters). A run record
  `<data>/coders/<run id>.json` (`pid`, `startedAt`, the pid of the daemon, parent,
  title, profile, project) is written before the process starts and removed when the run
  has ended and both sessions are told.
- **The transcript** of the child session, through `Agent::append_external`: the job as
  a user message with `Via::Agent`; for each text the tool writes, an assistant message;
  for each call it starts, an assistant message that holds the call (a call keeps the
  tool's own name, `Bash`, `Edit`, `command_execution`, and its input); for each result,
  a tool message (its text, cut at 8000 characters, `is_error` as the tool says), and
  one that says the run ended for a call left without a result; a call the tool refused
  is a tool message with `is_error`. Reasoning, retries and unknown lines are counted
  and not stored. The daemon's redactor (token-guard.md section 3) runs over every text
  before it is stored or reported: a coder that quotes the API token cannot bring it to
  the model.
- **The end** is the adapter's verdict, given to `Agent::coder_ended`, which does what
  `child_ended` does (the registry, the next of the queue, the parent, the event):
  - `done`: the tool reported success.
  - `failed`: the tool reported an error, or exited with a code other than 0 without a
    result, or printed no result, or was cut by the timeout.
  - `cancelled`: stopped by `agent_stop`, Stop on the card, the kill switch or the
    delete of its parent. It reports nothing.
- **The report** has the shape of `agents.md` section 3, so the cards and `agent_wait`
  read it unchanged:
  ```
  [Agent "<title>" done in 42 s]
  agent <id>, profile claude-edit
  claude sonnet in KAgent, 5 turns, $0.12
  <the coder's final message>
  ```
  The third line names the tool, its model and the project folder, then the turns and
  the cost when the tool reports them. A run that was refused something ends the line
  with `not done, it needed permission: <what was denied>` (section 3). The final message
  is cut at 4000 characters, as a subagent's answer (`agent_wait` gives 6000).
- **A run that exits at once** (within 15 seconds, without one line of the tool) is
  `failed`, and its error holds the stderr tail: a missing login, an unknown flag.
- **The timeout** is `coders.timeoutMin` from the start: TERM to the group, KILL after
  2 seconds (`kill_group`), the status `failed` with `timed out after N min`; the
  transcript up to then stays.
- **Stop** (`agent_stop`, Stop on the card) ends the child as `cancelled` at once (the
  slot is free, the event goes) and tells the launcher to kill the group. Output that
  arrives after a stop is dropped.
- **The kill switch** (`stop_all`) also ends every running coder run as `cancelled` and
  kills it (`cancel_running_coders`, beside `cancel_queued_children`).
- **Deleting the parent** kills the coder runs among its children, then deletes the
  sessions (`agents.md` section 2).
- **Daemon exit**: every running run gets TERM, then KILL after 2 seconds
  (`Coders::shutdown`, as `Background::shutdown` is), before the process ends.
- **Daemon start**: a run record still in `<data>/coders/` belongs to a daemon that is
  gone. Its group is killed when a member still lives (`group_alive`); the child session
  gets a last assistant message `(interrupted: KAgent was stopped while this run went
  on)`; the parent gets a stored message (no turn) whose first two lines are those of a
  failed report and whose third line is the same sentence in brackets; `N` is the time
  from the start of the run to the start of this daemon, at most the timeout. Then the
  record is deleted. A run never goes on unseen.
- **A coder child runs no KAgent turn.** Its session is read-only (a message into it is
  409, as into any subagent's), it has no KAgent tools, and the hooks of S32 do not fire
  for its steps: they fire for the `coder_run` call, as for any tool call.

## 3. What a coder may do (profiles)

A profile has a `tool` and a `mode`; the built-in of a tool and a mode is named
`<tool>-<mode>`.

| Mode | The coder may | Claude Code | Codex (`exec`) | The card |
|---|---|---|---|---|
| `plan` | read the project and answer; change nothing | `--permission-mode plan` | the default read-only sandbox | none (as `agent_run`) |
| `edit` | edit files in the project folder; run the commands of the allowlist | `--permission-mode acceptEdits`, and `--allowedTools` with the allowlist | `--sandbox workspace-write` | in `ask`; none in `trusted` |
| `full` | anything the owner can | `--dangerously-skip-permissions` | the sandbox of full access and the flag that bypasses approval (S36 names both) | in every mode but full control; never rememberable |

- `plan` and `edit` also pass `--permission-prompts none` (as `claude --help` spells it
  on the machine): what would prompt is denied instead of waited for, a question to the
  owner is impossible, and the denials are in the report.
- The **allowlist of `edit`** (Claude Code tool rules; Codex has its sandbox and takes
  no list): `Bash(cargo build:*)`, `Bash(cargo check:*)`, `Bash(cargo clippy:*)`,
  `Bash(cargo test:*)`, `Bash(go build:*)`, `Bash(go vet:*)`, `Bash(go test:*)`,
  `Bash(npm run build:*)`, `Bash(npm run lint:*)`, `Bash(npm test:*)`, `Bash(pnpm build:*)`,
  `Bash(pnpm lint:*)`, `Bash(pnpm test:*)`, `Bash(pytest:*)`, `Bash(git diff:*)`,
  `Bash(git log:*)`, `Bash(git show:*)`, `Bash(git status)`. A profile of the owner
  (S36) changes it.
- **KAgent's own folders stay closed.** Every profile passes `<config>` and `<data>`
  (`protect::dirs`) as denied paths to the file tools of the coder (`--settings` with
  `permissions.deny`). The configuration protection of hooks.md section 5 must not be
  bypassed by a second agent, and the token stays where it is. What a tool cannot deny
  (Codex reads where its sandbox lets it) the redactor of section 2 catches on the way
  back.
- **To verify with the real program** (S35 for Claude Code, S36 for Codex): that
  `acceptEdits` writes only inside the project folder; that the deny paths hold in
  `full` too; what Codex asks for in a folder it has not seen, and in a folder that is
  not a git repository. A row that does not hold is changed here, in the same session.
- **The project** is a name from the `projects` list (`xdg::find_project` over the
  roots of `settings.desktop.projectRoots`), never a path: the coder works in a folder
  the owner declared. An unknown name is an error with the known names. A folder that
  is `<config>`, `<data>` or inside them is refused with the reason above.
- **The job** is the digest of the parent (2000 characters), then `Your job:` and the
  prompt (as `job_text` of `agents.md` section 2), then one fixed last paragraph: the
  coder was started by KAgent, the owner's agent; it never runs `git commit`, `git push`
  or a merge, it leaves its changes in the working tree, and it ends with a report of
  what it did and what is left.
- **Limits**: the shared concurrency limit; `coders.timeoutMin`; and
  `coders.launchesPerMinute`, starts per minute in the daemon: a further start is an
  error and no card, so a model that loops cannot burn the subscription.
- **Profile of the owner** (S36): a file `<config>/coders/<name>.md` with the front
  matter of the skills parser, as a subagent profile (`agents.md` section 1): `name` =
  the file name; `description` (required, at most 500 characters); `tool` (required);
  `mode` (default `plan`); `model` (passed through, at most 100 characters; empty = the
  tool's default); `effort` (Claude: `low`, `medium`, `high`, `xhigh`, `max`; Codex: one
  short lowercase word; empty = none); `allowedTools` (at most 40 Claude Code rules; on
  a Codex profile the key makes the file invalid); `args` (at most 16 extra words);
  `env` (at most 16 names); `prompt` (at most 8000 characters: Claude Code gets it with
  `--append-system-prompt`, Codex at the top of the job). A key that is not one of
  these, or a value out of range, makes the file invalid: it is skipped and listed with
  its reason, as an agent profile is. Refused in `args`: every flag this spec names for
  the tool (the run, resume, permission, prompt, output, settings and worktree flags),
  and for Codex `-c`; in `env`: `PATH`, `HOME`, `TMUX`, `CODEX_HOME`,
  `CLAUDE_CONFIG_DIR`, every `KAGENT*` name, and every name that holds `KEY`, `TOKEN`,
  `SECRET` or `PASSWORD` (a secret never goes into a profile file: the tool logs in its
  own way, or `loginShell` is on). The folder is protected like `<config>/agents`
  (`protect.rs`): the agent changes a profile only with a card.

## 4. The tool

`coder_run` (tools.md section 25), in the group `coders` (`group_of`,
`DEFAULT_TOOL_GROUPS`, `GROUP_PRIORITY`, and the test that every tool has a group).
`agent_wait`, `agent_stop` and `agent_list` work on coder runs unchanged (tools.md
section 23). `coder_run` is among the tools a subagent does not have (`ORCHESTRATION`).

- Input: `{description, prompt, project, profile?, background?}` (S37 adds `resume`,
  S38 `worktree`). `prompt` is one text or its lines (the S20 trap: the schema says
  `anyOf`), at most 16 000 characters.
- Assessment by the profile's mode: `plan`: MUT, risk low, no approval, parallel-safe (as
  `agent_run`: the call changes nothing by itself). `edit`: MUT, risk medium, rule key
  `coder_run <profile> <project>`. `full`: `always_ask`, risk high. A profile that is
  not known assesses as `full`, so a wrong name never lowers the bar; the call then is
  an error. The job text goes through the destructive gate (`safety::looks_destructive`
  on the prompt): a job that reads like a destructive command asks in every mode, and by
  voice needs the explicit phrase. Read-only mode refuses every profile, and plan mode
  refuses the call (it is not research; the open question of `agents.md` section 2
  stands for coders too).
- Result: `{agentId, status, title, profile}`. With `background: false` the call waits
  up to 120 seconds and then answers the report, or the same object with a `note` (as
  `agent_run` answers).
- Errors the model can fix (no child is made): the description or the prompt is missing
  or too long; `project` is unknown; `profile` is unknown; the binary is not installed;
  the rate cap; the folder is KAgent's own; the launcher is missing ("coding agents are
  not available in this process"). Read-only mode is the engine's denial.
- On the MCP server the tool answers what `agent_run` answers there (S35 reads
  `kagentd/src/mcp.rs` and follows it).
- The description of the tool is tuned with raw replays of the local model, as S31
  tuned `agent_run` (12 of 12 calls there; the bar is the same here). The words that
  make `coders` a hinted group start as: `claude`, `codex`, `coding agent`, `implement`,
  `refactor`, `write a function`, `write the code`, `add a test`, `unit test`,
  `code review`, `fix the bug`, `napiši`, `refaktor`, `popravi`. A sentence in the
  system prompt, or a pointer in the description of `agent_run`, is added only if the
  replays show that the description alone is not enough, and only if `agent_run` keeps
  its 12 of 12.

## 5. Settings and the page

A typed section (config.md section 11): `defaultProfile` (default `claude-edit`),
`timeoutMin` (1..1440, default 60), `launchesPerMinute` (1..30, default 6),
`loginShell` (default false), `claudeBinary` (default empty; else an absolute path).
S36 adds `codexBinary`; S38 the three notification keys.

Settings -> **Coding agents** (`coders`, group "Agent capabilities", after Subagents):

- S35: one row per tool: found with its path and version, or "not installed"; **Test**
  (section 6); then the rows of the section (default profile, timeout, starts per
  minute, login shell, the binary).
- S36: the profiles (the built-ins marked) and their editor (name, description, tool,
  mode, model, effort, allowlist, extra words, environment, prompt), Delete; an invalid
  file with its reason.
- S37: "Open in a terminal" on an ended card and in the header of its session.
- S38: the review block of an ended run (section 8).

## 6. API (api.md section 6)

`GET /api/coders` and `POST /api/coders/test` (S35), `PUT` and `DELETE
/api/coders/:name` (S36), `POST /api/coders/terminal` (S37), `GET
/api/coders/:id/review` (S38). A run is stopped through `POST
/api/sessions/:id/subagents/:child/stop`, and its session is read through `GET
/api/sessions/:id`, whose meta carries `coder` (config.md section 11).

**Test** is the owner's word for a real run. `{tool}` alone makes the version probe.
`{tool, run: true}` then starts one short job with the `plan` profile in a new
temporary folder ("Reply with the single word OK."), 60 seconds at most, with the
environment section 1 describes. It uses the owner's subscription, so it is never a
test and never automatic. The answer says which step failed and why (not installed,
exited at once with the stderr tail, no answer). A Test that fails with `loginShell`
off and passes with it on tells that the tool needs the environment of the terminal.

## 7. History, resume and the terminal (S37)

- **Session index** (`coders/index.rs`), read-only, from Omnius's session index: Claude
  Code keeps a conversation in `<claude home>/projects/<encoded folder>/<uuid>.jsonl`
  (`$CLAUDE_CONFIG_DIR`, else `~/.claude`; the folder name is the project path with
  every character that is not a letter or a digit replaced by `-`, per UTF-16 code
  unit); Codex in `<codex home>/sessions/YYYY/MM/DD/rollout-*.jsonl` (`$CODEX_HOME`,
  else `~/.codex`), its id and folder in the first line (`session_meta`). A record is
  attributed to a project by encoding the known projects and matching the folder name;
  where two encode alike, the first `cwd` of the file decides, and without one the
  record is listed for no project. Title: Claude Code's `ai-title` or `custom-title`
  line (the latest, a custom one wins), else the first real prompt (slash commands,
  caveats and the notes of KAgent are skipped); Codex: its `thread_name` in
  `session_index.jsonl`, else the first prompt. The rollouts of sub-agents and forks
  are left out. Reads are bounded (the head and the tail of a file) and keyed by the
  modification time; a failed read is never a deletion; nothing is written.
- **`coder_sessions`** (read-only, group `coders`; tools.md section 25): `list` gives
  one line per conversation of a project (id, tool, time, title, tokens when known, and
  `KAgent` for one that a run of KAgent made); `read` gives the last messages of one as
  Markdown, at most 12 000 characters, labelled untrusted.
- **Resume**: `coder_run {resume: <id>}` continues a conversation (the whole id, or a
  unique prefix of 8 characters) with a profile of the same tool; without `project` the
  folder of the earlier run is used. It is a new child with a new card, and its meta
  says what it resumes. A conversation that a run of KAgent uses now is refused
  (`already running`). From S37 on, the third line of a report ends with
  `resume <id>`.
- **The terminal bridge**: `POST /api/coders/terminal {tool, sessionId, project?}` opens
  Konsole in the folder with the interactive form of the same line
  (`claude --resume <id>`, `codex resume <id>`); 409 while a run of KAgent uses the
  conversation. The owner takes a job over from where the run stopped.
- **Handover** between tools needs no code: the bundled skill `handover` has the agent
  resume the first tool with a request for a handover report (Task, State, Next steps,
  Files, Branch and running servers, Notes) and start the second with that report at
  the top of its job.

## 8. Worktree, review and notifications (S38)

- **`worktree: true`** on `coder_run`: the run works in a linked checkout
  `<project>/.worktrees/kagent-<8 hex>` on a new branch `kagent/<8 hex>`
  (`git worktree add`, argv, no shell, 120 seconds); the folder must be a git
  repository, else an error and no child. `.worktrees/` goes into the `info/exclude` of
  the repository when it is not ignored yet; `.gitignore` is never edited. The report
  names the folder and the branch. KAgent never merges, never deletes a branch or a
  worktree: that stays with the owner. `coder_sessions` gains `worktrees`, the list of
  a project with branch, clean or dirty, and last activity.
- **Review**: when a run ends, the runner records on `coder` (config.md section 11) the
  files the tool edited (from its edit calls), the branch, the status and the diff
  stat of the project (`git status --short`, `git diff --stat`, at most 4000
  characters). `GET /api/coders/:id/review` returns it; the header of the ended session
  shows it as the review block (files, branch, worktree, the stat, and the buttons of
  S37).
- **Notifications** (`notify.rs`): `coders.notifyDesktop` (default true) and
  `coders.notifyTelegram` (default false), after `coders.notifyAfterMin` minutes
  (0..120, default 2) of a run that ended `done`, and always for one that failed or
  timed out: "Claude Code finished: <title>" and the first 200 characters of its report.
  The parent is told as before; a run nobody watches is still noticed.

## 9. Automation (S39)

- A task can be a coder run: `kind: "coder"` with `coder: {profile, project,
  worktree?}`; `what` is the job (automations.md section 1). The runner makes a session
  with origin `task` ("Task: <title>"), asks the permission engine with the assessment
  of the tool, starts the run as the session's child, waits `timeoutMin`, records the
  first 500 characters of the report as the result of the run, and delivers as
  `deliver` says. No model sits between the schedule and the run.
- What the engine says decides whether the run starts: allowed, and it starts; a card,
  and under `deny` (the default) the run is not started and is recorded `failed` ("it
  needs approval on the desktop"), under `wait` the card is shown and awaited as any
  card of the session. `plan` needs no card.
- `tasks add` takes `kind: "coder"` with `project` and `profile`; `tasks list` shows
  the tool; the Tasks tab has the project and the profile in its form and the report of
  the last run in the row detail.
- Continuing a run that stopped on a usage limit by itself (Omnius's wake) is not
  built: section 10.

## 10. Not ported, and open questions

Not ported from Omnius: accounts and the switching of credentials, the usage board
(both read the subscriptions' tokens), remote mode (off in Omnius itself), sync between
machines, the restore points, and what belongs to a terminal interface (abbreviations,
the prompt log, the header animation, the clock, the search palette, the statistics
popup, the ETA catcher, opencode and Grok). tmux is not used.

Open for the owner (the defaults stand until they decide):

- Relaying the permission prompts of a coder as cards of KAgent, answerable by voice
  (Claude Code's `--permission-prompt-tool`): it would give `edit` a way to ask. Not
  planned.
- `project` by name only; a page with buttons that start a run for a project, as
  Omnius's picker does; a separate concurrency limit for coders.
- Whether a task may continue a run that stopped on a usage limit once the window
  resets.

Known limits: a `full` run is as powerful as the owner; the permission engine, the
hooks and the token guard of KAgent do not reach inside it (the deny paths and the
redactor are what section 2 and section 3 give). A run cannot be joined while it works;
it is continued afterwards in a terminal (S37). The owner cannot be asked mid-run: a
question is refused and the report says so.

## 11. Tests (no root; no network; never a real tool)

The real `claude` and `codex` are started only by the owner (Test, and the scenarios
marked for the owner) or by a stage agent the owner allowed real runs for. Fixtures
recorded from real runs replace the samples written from the documentation as soon as
one exists; a test says which kind its fixture is.

- S35, core (`children/external.rs`, `tests/subagents/coder.rs` with the scripted model
  and a fake launcher): a run is a child (queued under the limit, running with a slot);
  `subagentUpdated` on each change; the running runs are listed; a finished run gives an
  idle parent a turn with the report and a busy one steering; a failed start reports at
  once; Stop and the kill switch end it as cancelled, report nothing and tell the
  launcher to kill; deleting the parent kills it and deletes the session; a subagent
  cannot start a coder; `agent_wait` and `agent_list` see it; `coder_ended` for an
  unknown or ended child does nothing.
- S35, adapter (`coders/claude.rs`, recorded lines under `crates/kagentd/src/coders/fixtures/`,
  loaded with `include_str!` as `web/search` loads its pages):
  the transcript of a run with text, tool calls and results; a refused call; each
  result subtype gives its status; a stream that ends without a result; an unknown line
  is skipped; the third line of the report, with denials and without.
- S35, runner (`coders/runner.rs`, a fake `claude` script): its own process group; the
  job on stdin and not in argv; a timeout kills and reports `timed out`; stop kills and
  late output is dropped; the run record appears and goes; a leftover record at start
  gets its group killed and both sessions their line; `loginShell` runs through the
  shell; the redactor runs over the report and the transcript.
- S35, tool and settings (`tools/coders.rs`, `coders/settings.rs`): the assessment of
  each mode in each permission mode; an unknown profile assesses as `full`; an unknown
  project, a folder of KAgent, a missing binary and the rate cap are errors and start
  nothing; the job carries the digest and the last paragraph; the section's defaults,
  ranges, typo suggestion and refusal on load.
- S36: the Codex adapter on recorded lines (each item type, `turn.failed`, `error`);
  the profile files (every field, each refusal of `args` and `env`, an invalid file
  listed); the routes' status codes; `<config>/coders` is protected.
- S37: the encoding of project folders and the attribution of a record (two that encode
  alike); titles; the skip of sub-agent rollouts; `resume` (prefix, wrong tool, already
  running); a failed read changes nothing.
- S38: a worktree is created, named in the report, and never removed; the `worktrees`
  list; a folder that is not a repository is an error; the review's files and stat; a
  notification by each rule.
- S39: a `coder` task starts a run and records its report; the approval policy `deny`
  and `wait` under `edit`; delivery; the timeout.
