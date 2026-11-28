# Spec: acceptance scenarios

Given / When / Then per stage. A stage is done when its scenarios pass (automated
where marked **[auto]**, otherwise run manually once and note the result in
IMPLEMENTATION.md section E). Test model: Ollama `qwen2.5:7b-instruct`. All daemon
scenarios run with `KAGENT_HOME=$(mktemp -d)` unless stated.

## S0-S1 core and read-only tools
1. **[auto]** Given the workspace, when `cargo test --workspace`, then all tests pass
   and `cargo clippy --workspace -- -D warnings` is clean.
2. **[auto]** Given `examples/tools_smoke.rs`, when run, then every RO tool returns
   `ok` (sections may say `unavailable`) within 5 s each.
3. **[auto]** `shell` assessment table (tools.md section 5) classifies all cases.
4. **[auto]** `journal` argv builder rejects `since: "x; rm -rf /"`.
5. **[auto]** Agent loop with a scripted fake model: model asks for 2 RO calls + 1 MUT
   call in one response -> the 2 RO run in parallel, the MUT asks the broker; deny ->
   tool result says "not executed: the user denied this action"; next model request
   contains 3 tool results in call order.
6. **[auto]** Cancel during a pending approval -> turn status `cancelled`, every tool
   call has a result, no dangling call in the stored session.
7. **[auto]** Session file with an assistant tool call but no result (crash) ->
   `sanitize_history` inserts a synthetic error result.

## S2 providers
1. **[auto]** Import script produces a file with >= 20 templates incl. `anthropic`,
   `openai`, `ollama`, `gemini`; no template with `zhipu-account`.
2. **[auto]** Rule precedence: model rule says contextWindow 100k, api rule 150k,
   site rule 200k -> resolved 200k; personal config 50k -> 50k.
3. **[auto]** Add from template twice -> ids `anthropic`, `anthropic-2`.
4. **[auto]** Masking: `sk-ant-abcdefghijkl1234` -> `sk-...1234`; short key -> `****`.
5. **[auto]** File written with mode 0600; reload equals saved.

## S3 adapters
1. **[auto]** Each SSE fixture (text + one tool call, arguments split across 3 chunks)
   -> ModelResponse with exact text, one ToolCall with parsed JSON input, usage set.
2. **[auto]** SSE parser handles `\r\n`, comments (`:` lines), event split across
   chunk boundaries.
3. **[auto]** Invalid tool-argument JSON -> input `{"_raw": "..."}`, no panic.
4. **[auto]** 429 then 200 -> one retry, success; cancel before first byte -> error
   returned within 200 ms.
5. **[gated auto]** `KAGENT_IT=1`: Ollama tool round trip: ask "call system_info" with
   a single tool spec -> model emits a tool call.

## S4 daemon
1. **[auto]** Wrong Host header -> 421; missing token -> 401; correct -> 200.
2. **[auto script]** `scripts/smoke-api.sh`: add Ollama provider, set chatModel, create
   session, send "What kernel version am I running? Use your tools.", turn finishes
   `completed` within 120 s, session has a tool message from `system_info` or
   `kernel_info`, final answer contains the string from `uname -r`.
3. Send a second message while the first turn runs -> response `queued`; it appears
   as a user message before the next model request (steering).
4. DELETE a running session -> turn cancelled, files removed.

## S5 web
1. Open the printed URL -> token stored, hash cleared, chat loads.
2. Ask "list the 5 processes using the most memory" -> streaming text, a
   `process_list` tool card with input/output/duration.
3. Start `sleep 600` in a terminal; ask "kill the sleep 600 process" -> approval card
   (risk high) -> Deny -> model reports it did not kill; repeat -> Allow once -> killed.
4. Providers page: add Anthropic template with a fake key -> test shows an auth error
   (not a crash); key shown masked; reload page -> masked again.
5. Permission mode read-only -> mutating calls are denied without a card.

## S6 audit + kill switch
1. **[auto]** 3 tool calls -> 3+ audit records; `GET /api/audit/verify` ok; edit one
   byte in the file -> verify reports that seq.
2. **[auto]** Tool running `sleep 60` (shell) -> `POST /api/stop-all` -> process group
   gone within 3 s, turn `cancelled`.
3. **[auto]** Input field `password` appears redacted in the audit record.

## S7 diagnostics + skills
1. Ask "my machine feels slow, check it" -> model calls `load_skill slow-machine`,
   then at least `system_info`, `process_list`; answer names the top consumer.
2. Ask "why is boot slow?" -> `boot_analysis` used; answer cites the top blame entries.
3. Ask "is my nvidia driver ok after the last kernel update?" -> `drivers` +
   `kernel_info` used; answer states DKMS status for the running kernel.
4. **[auto]** Skills loader reads frontmatter `name`, `description`; missing
   frontmatter -> skill skipped with a warning.

## S8 change layer
1. **[auto]** priv validation table (priv-ops.md section 5) passes.
2. Helper not installed -> a PRIV call returns a clear error with the install hint;
   `GET /api/state` shows `privHelper: "missing"`.
3. "Set swappiness to 10" -> approval card shows `sysctl vm.swappiness=10 (persist)`
   -> allow -> `/proc/sys/vm/swappiness` is 10, `/etc/sysctl.d/99-kagent.conf` has the
   line, change listed -> Undo -> value back to the old one, file restored.
4. "Install htop" -> card shows the apt simulation -> deny -> nothing installed.
5. "Restart bluetooth" -> PRIV card -> allow -> unit restarted, change recorded
   (no undo available, card says so).
6. `edit_file` on a home file with `old` occurring twice -> error with count 2.
7. Ask to stop `dbus.service` -> refused by validation with an explanation.

## S9 desktop
1. "Switch audio output to <headphones>" -> `audio_control set_default`; `wpctl status`
   shows the new default.
2. "Connect <bt device>" -> connected; "dark mode" -> colorscheme applied.
3. Provider key migrated to Secret Service: `providers.json` has `apiKey: ""` +
   `apiKeyRef`; daemon restart still works.

## S10 voice
1. **[auto]** Setup message builder output matches a golden JSON (model, voice, tools
   converted to Gemini schema with uppercase types, resumption, compression).
2. **[auto]** Fake socket: `toolCall` with 2 functionCalls -> 2 `execute_call`s ->
   one `toolResponse` with both ids in order.
3. **[auto]** `goAway` -> reconnect with the stored handle; browser socket stays open
   (fake upstream).
4. Manual: say "what is my CPU temperature?" -> spoken answer with the value; interrupt
   mid-answer -> playback stops within 300 ms.
5. Manual: say "restart bluetooth" -> approval card appears in the UI, model says it is
   waiting; click allow -> it confirms by voice.
6. After the session, the text session contains the voice transcript (via voice).

## S11 Tauri
1. Hotkey shows the overlay with the orb; speaking animates it; Escape hides it.
2. Tray "Stop all" works like `POST /api/stop-all`.
3. Closing the main window keeps the daemon and tray running.

## S12 MCP
1. MCP inspector lists only tools from `mcpToolGroups`; `system_info` returns data.
2. A MUT tool via MCP raises an approval card in the KAgent UI; deny -> MCP result says
   denied; audit has session id `mcp`.

## S13 watchers
1. **[auto]** Each matcher fixture (alerts.md section 6).
2. `logger -t sshd "Failed password for invalid user test from 203.0.113.9 port 1 ssh2"`
   x12 within a minute -> one `ssh-bruteforce` alert, count 12, one notification.
3. Fill a test loop-mounted ext4 image to 95% -> `disk-space` review alert.
4. "Investigate" on an alert -> new session with the evidence, model proposes a fix.

## S14 intrusion detection
1. Enable honeytokens -> 4 files created, listed in `honeytokens.json`.
2. `cat ~/.aws/credentials.legacy` from a terminal -> `honeytoken-access` high alert
   within 2 s (with pid/comm when the bpftrace stream is enabled).
3. Add a line to `~/.ssh/authorized_keys` -> `persist-authorized-keys` high alert with
   the key fingerprint within 10 min (or immediately via inotify).
4. Create `~/.config/systemd/user/x.service` with `ExecStart=/tmp/x` -> high.
5. A change applied through KAgent (S8 sysctl persist) does not raise a baseline alert.
6. Stop the daemon -> within 5 min a critical desktop notification "KAgent stopped".

## S15 MikroTik
1. **[auto]** REST fixtures parse into devices; IPFIX fixture (template + data set)
   decodes to the expected flow rows.
2. With the owner's router: `lan_devices` lists devices with first-seen; a phone joining
   Wi-Fi raises `lan-new-device`.
3. `nmap -p 1-1000 <router>` from a LAN laptop -> the laptop appears in `scanners`;
   `router-scanner` review alert.
4. `router_block_ip 203.0.113.9` -> approval (high, not rememberable) -> entry exists
   in `kagent-block` with timeout; undo removes it.
5. Router powered off -> `router-unreachable` info after 10 min, no crash.

## S16 eBPF
1. "What is writing to my disk right now?" -> `trace io-by-process` (PRIV card) ->
   answer lists top processes by bytes.
2. **[auto]** Unknown script id or seconds 60 -> rejected by the helper.
3. Each script runs once without bpftrace errors on this machine (log fixes in E).

## Voice levels + KDE widget (added in review, 2026-11-24)
1. **[auto]** `voice_levels` tests: envelope of silence/tone, speaking -> idle state
   transitions, seq-based polling without repeats, ring + poll cap.
2. Daemon running, widget installed: `POST /api/voice/demo` -> widget shows blue bars
   labelled "Speaking", then green bars labelled "Listening". (Verified 2026-11-24 with
   `plasmawindowed org.kde.kagent.voice`.)
3. A chat turn running -> widget shows the scanner sweep with "Thinking..."; an approval
   card pending -> red sweep, "Waiting for your approval", plasmoid needs-attention.
4. Daemon stopped -> "KAgent is not running", polling backs off to 2 s; daemon restarted
   with a new token -> widget recovers within 5 s (401 -> token re-read).
5. S10: during a live session the widget follows real mic and model audio with < 200 ms
   visible lag.

Phase 4 (S17-S33, planned 2026-11-24). Same rules: daemon scenarios with
`KAGENT_HOME=$(mktemp -d)` unless stated. Each stage also runs the unit tests its spec
lists under "Tests".

## S17 agent foundations
1. **[auto]** A turn cancelled after two text deltas: `wait` returns `Cancelled` and
   `text` holds both deltas.
2. **[auto]** A tool future that panics: the turn ends `Failed` with "internal error",
   `TurnFinished` is emitted, and the same session runs a next turn.
3. **[auto]** "This session" allowed in session A for `shell ls`: the same call in
   session B still shows a card.
4. **[auto]** `PUT /api/settings {"startup":{"greeting":{"enabled":false}}}` keeps
   `startup.autostart` and the greeting text.
5. **[auto]** `PUT /api/settings {"voice":{"bran":"routed"}}` -> 400 with
   `did you mean "brain"`.
6. **[auto]** A group missing from `GROUP_PRIORITY` is advertised when the budget allows.
7. **[auto]** The guard table: with `KAGENT_HOME` set, or from a `target/` binary while
   the unit points to a release binary, the systemd unit is not rewritten.
8. **[auto script]** `scripts/smoke-api.sh` passes; `systemctl --user cat kagentd.service`
   shows the same `ExecStart` before and after it.

## S18 skills library core
1. **[auto]** The 12 real frontmatter fixtures parse to the expected name, description
   and body; the three the old loader dropped (`discernment-nudge`, `claude-api`,
   `academy-guide`) parse.
2. **[auto]** With `<config>/skills/alpha` present the index lists `alpha` and every
   bundled skill; a `beta` directory added between two turns is in the next prompt
   without a restart.
3. **[auto]** A user skill named like a bundled one wins; `skills.disabled` with a
   bundled name hides it from the index and `load_skill` refuses it.
4. **[auto]** With `indexBudgetChars` 500 the index holds full lines up to the cap and
   then one names-only line.
5. **[auto]** `load_skill` returns the body with `${CLAUDE_SKILL_DIR}` replaced by the
   skill directory and counts a use.
6. With Ollama, "my machine feels slow, check it" still calls `load_skill
   slow-machine` (regression of S7 scenario 1).

## S19 settings screen
1. `#view=settings` opens the last section; `#view=settings&section=tools` opens Tools
   and permissions; `#view=providers` opens Models.
2. Every section of the registry opens from the menu without a console error, dark and
   light, at 1200x820 and at 700x500 (headless Chrome, screenshots checked).
3. Type a persona name, then toggle "Greet me": the typed name is still there; one PUT
   per change; no second `GET /api/state` for `settingsChanged`.
4. `ui show {view: "settings", section: "browser"}` opens the Browser section.
5. `pnpm -C web dev` against the running daemon: a chat message streams its answer.
6. Tools and permissions shows `toolOverrides` per tool and the `tasks` group chip.

## S20 skills UI and self-authoring
1. Settings -> Skills lists bundled and user skills with source badges; switching a
   bundled skill off removes it from the next prompt.
2. "New skill" with an invalid name shows a field error; a valid one appears in the
   list and in the next prompt.
3. In chat, "remember how you fixed the bluetooth problem as a skill": `skill_save`, a
   card with the text, allow, the file exists under `<config>/skills` and `load_skill`
   returns it.
4. `skill_delete` on a bundled skill -> error naming the source; on a user skill ->
   card -> allow -> the directory is gone.
5. `/` in the composer lists the skills; `/slow-machine it lags after login` runs the
   turn with the skill body in every model request of that turn, and the stored user
   message is only the text.
6. `skill-creator` is listed and loads.

## S21 skill import and sources
1. **[auto]** Import plan from two source folders that share a real path: one entry; an
   existing name is skipped with `name exists`.
2. Import three skills from `~/.agents/skills` in copy mode: they appear switched off;
   enable one: it is in the next prompt.
3. Import one in symlink mode, then delete it: the link is gone and the target folder
   still has its `SKILL.md`.
4. Add `https://github.com/obra/superpowers` with subdir `skills`: a clone under
   `<data>/skill-sources/`, its skills listed disabled, the commit in
   `skill-sources.json`.
5. Change a file in a local-path source and sync: the skill is `updated` and disabled
   again; a removed skill is reported `removed`.
6. **[auto]** A source URL that is neither https nor an absolute path -> 400 before any
   command runs.

## S22 context compaction
1. **[auto]** Scripted model, 32k window, a 60-message history: one summarize call; the
   next request starts with the summary message; the session file is unchanged; the
   meta holds `summary` and `summaryUpto`.
2. **[auto]** 16k window without `fastModel`: the old note ("N earlier messages were
   omitted") and no summarize call.
3. **[auto]** A failing summarize call: the turn continues with the note.
4. **[auto]** A capacity error from the model: one compaction and a retry before
   `fallbackModel` is used.
5. `/compact` in a long chat returns; the next answer still uses a fact from the
   summarized part.
6. **[gated auto]** With Ollama (16k) the S4 smoke question still finishes with one
   tool call.

## S23 todo lists
1. **[auto]** `todo_write` with two items `in_progress`: the second becomes pending;
   the result lists all items; `todoUpdated` is emitted.
2. **[auto]** After a daemon restart the list is still in the meta.
3. Ask for a five-step diagnosis: the panel appears above the composer, items turn done
   as the work proceeds, a click toggles one item, Clear empties the list. Run it with a
   cloud chat model (Gemini): the local `qwen2.5:7b-instruct` often skips the status
   updates, which is accepted (decision of 2026-11-27, section E).
4. The widget shows "2/5".

## S24 plan mode
1. **[auto]** In plan mode with `full` control and an allow rule, `write_file` is
   refused with the plan message; `read_file` and `todo_write` run.
2. **[auto]** `plan_exit` with a plan raises a `kind: "plan"` card; approve ->
   `planning` false, the plan file written, `approvedPlan` set, the next request has
   the plan block.
3. **[auto]** Deny with a reason: still planning; the tool result carries the reason.
4. `/plan free 20 GB of disk`: the agent researches read-only and shows the plan card;
   Approve: the steps run under the normal permission mode. Run it with a cloud chat
   model (Gemini). With the local `qwen2.5:7b-instruct` the card came in 4 of 15 runs;
   that is accepted, the loop does not nudge (decision of 2026-11-27, section E).
5. Voice: "yes" to a plan card approves it (an ordinary card).

## S25 memory
1. **[auto]** Scorer: "which router do I have" picks `router` over `printer`; "stop the
   music" matches nothing.
2. **[auto]** Secret guard: an `sk-...` key is refused; "the printer is in the
   hallway" is accepted.
3. **[auto]** `replaces` removes the old memory from recall; `memory_get` still returns
   it.
4. "Remember that my printer is in the hallway": card, allow, the file exists; in a new
   session a message about the printer has the memory in its prompt.
5. Settings -> Memory lists it as "agent, not reviewed"; opening it clears the badge;
   an edit survives a restart.
6. "What did we decide about the router last week?" -> `session_search` returns the
   older session.

## S26 voice brain
1. **[auto script]** `scripts/test-live.py`: the checks of `scripts/test-live.py` (34
   in six groups).
2. **[auto]** `speakable` turns an answer with a table and a code block into plain
   sentences.
3. `routed`: "what is my CPU temperature" -> the chat model's answer is spoken word for
   word; the session holds the turn with via voice.
4. A question that needs a tool, then a follow-up about the answer: the follow-up is
   answered with the earlier context.
5. "stop" while it thinks: silence, the turn is cancelled; "stop the music" pauses the
   player.
6. `live`: a session opened after a long chat knows what was discussed; with the
   vocabulary on, a project name is transcribed correctly.

## S27 browser attach
1. **[auto]** `parse_devtools_active_port` table; tab rules table.
2. **[auto]** `BrowserSession` against the fake CDP server: two targets on one socket;
   reopened after a drop.
3. Remote debugging off in Brave: `browser status` says so and prints the instruction
   line; Settings -> Browser shows the same.
4. Tick the checkbox, Connect, click Allow: status `on` with the tab count; "read the
   page I have open" returns the active tab's text.
5. A click in the owner's tab shows a card in `full` mode; closing that tab is refused;
   a tab the agent opened can be closed.

## S28 automations
1. **[auto]** Schedule table: "every 2 weeks on monday 09:00", "mon,fri 18:00",
   "*/20 9-17 * * 1-5" and "in 20m" parse; "sometimes" -> 400.
2. **[auto]** `next_run` with a fixed clock: three runs do not drift; a run 6 minutes
   late is `missed`.
3. **[auto]** An old `tasks.json` with `Every` and `Daily` records loads and runs as
   before.
4. A task created with `delayMinutes: 1` fires once, notifies, and is `done`.
5. A weekdays task whose prompt needs an approval, in `ask` mode: the call is refused,
   the result lists what was skipped, the history shows `ok`.
6. The template "Morning health check" enabled: the next due run writes a run with an
   empty result and no notification when the score is 90 or more (or any agent task
   whose prompt has nothing to report).

## S29 Telegram bot
1. **[auto]** Pairing table; splitting; Markdown to HTML; forwarding rules.
2. Paste a token: the bot username appears; "Show pairing code"; `/start <code>` from
   the phone: status `polling`.
3. A text from the phone is answered in Telegram and appears in the session with a
   `telegram` pill; a message from a second chat gets no answer.
4. With forwarding on: a pending card arrives with buttons; Allow once resolves it; a
   reboot card is never forwarded.
5. A `review` alert inside quiet hours is not pushed; a `high` alert is, with an Ack
   button.

## S30 web tools
1. **[auto]** Guard table; a redirect to a blocked address stops.
2. **[auto]** HTML fixtures -> the expected text and links; the search page fixture ->
   results.
3. "What is the latest stable kernel on kernel.org?": `web_fetch` answers without a
   browser window.
4. "Search the web for ...": results are listed; with the engine Off the tool says
   search is off.
5. `web_fetch http://127.0.0.1:9333/json/list` is refused.

## S31 subagents and background tasks
1. **[auto]** The `children.rs` scenarios of `agents.md` section 7.
2. **[auto]** Tool filter; `explore` refuses a mutating call.
3. "Research in parallel what uses the disk and what uses the memory, then summarize":
   two Agents cards; the summary arrives without a new message. (With the local
   qwen2.5:7b-instruct: two `agent_run` calls in 6 of 6 runs, the summary without a
   new message in 6 of 6; two Agents cards only after the first pair was refused for
   `model: "general"`, in 4 of 6 runs.)
4. A mutating call by `general` shows a card named after the subagent.
5. `shell` with `background: true` returns at once; `task_output` shows the output;
   Stop all kills it.

## S32 hooks
1. **[auto]** Runner table: exit 0, 2, other; timeout; async.
2. **[auto]** Allow cannot relax a destructive call; ask forces a card; deny ends the
   call.
3. **[auto]** Protection table: `edit_file` on `settings.json` and
   `shell ... > .../settings.json` are `always_ask`.
4. A `PreToolUse` hook that denies `shell` commands containing `curl`: the call ends
   with "not executed"; the audit has a `Hook` record.
5. Settings -> Hooks: Test runs the hook with the sample and shows the decision.

## S33 inline widgets
1. **[auto]** Size cap; unknown id is 404; export replaces the markers.
2. "Show my doctor score as a gauge": the answer holds the marker and the panel renders
   in chat.
3. Headless Chrome: a fragment's `fetch`, `top.location` and `localStorage` stay
   blocked; the height follows the content.
4. Voice: the same question is answered in words, without the marker.

## S34 token guard
1. **[auto]** `read_file` of a credential file is refused without a card, and its content
   does not leak, in every mode.
2. **[auto]** A shell command that names a credential file, runs `journalctl` on the daemon
   or contains `7878` asks in every mode, including `full`; reading `settings.json` stays
   read-only.
3. **[auto]** A tool output that holds the API token reaches the model and the session
   without it.
4. Started with stdout that is not a terminal, `kagentd` prints no token (the journal of
   its first start under systemd shows the new line).

## S35 coding agents: runs and Claude Code
1. **[auto]** `tests/subagents/coder.rs` and `children/external.rs`: the scenarios of
   `coders.md` section 11 (a run is a child; the report reaches an idle and a busy
   parent; stop, the kill switch and the delete of the parent; the depth limit).
2. **[auto]** The Claude Code adapter on recorded lines: the transcript, each result
   subtype, a refused call, a stream without a result.
3. **[auto]** The runner with a fake `claude`: process group, stdin, the timeout, stop,
   the run record, the recovery at start, the redactor.
4. **[auto]** The assessment of each profile in each permission mode; an unknown
   profile, project or binary, the folder of KAgent and the rate cap start nothing.
5. The owner: Settings -> Coding agents finds `claude` with its version; Test answers
   OK (it uses a little of the subscription). With the daemon's environment and, if
   that fails, with the login shell on.
6. The owner, or the stage agent with the owner's leave: one job per mode in a
   temporary repository: `plan` changes nothing; `edit` edits a file, runs a test of
   the allowlist and cannot write outside the folder; a job outside the allowlist is
   refused and the report says so; `full` shows its card in `trusted` mode.
7. "Use Claude Code to add a unit test for X in <project>": one card `claude-edit`; the
   child session shows its steps; the report arrives without a new message. (With the
   local qwen2.5:7b-instruct: to measure; a cloud model otherwise.)
8. Headless Chrome: the card (queued, running, done), the read-only header of the coder
   session, the Settings page at 1200x820 and 700x500.

## S36 Codex and profiles
1. **[auto]** The Codex adapter on recorded lines: each item type, `turn.failed`,
   `error`, the report's third line without a cost.
2. **[auto]** Profile files: every field; each refusal of `args` and `env`; an invalid
   file is listed with its reason; a built-in name cannot be a file; `<config>/coders`
   is protected.
3. **[auto]** `PUT` and `DELETE /api/coders/:name`: 201, 200, 400, 409.
4. The owner: Codex installed and logged in; Test answers OK; one job per mode; the
   rows marked "verify" in `coders.md` section 3 are settled.
5. "Use Codex to ...": the card says `codex-edit`; the report arrives.
6. Headless Chrome: the profile editor; an invalid file with its reason.

## S37 history, resume and the terminal
1. **[auto]** The index on temporary stores: the folder encoding, the attribution of a
   record (two projects that encode alike), titles, sub-agent rollouts left out, a
   failed read changes nothing.
2. **[auto]** `resume`: a unique prefix; a profile of another tool; a conversation in
   use.
3. **[auto]** `coder_sessions`: `list`, and `read` (cut, labelled untrusted).
4. The owner: `coder_sessions list` shows the Claude Code conversations of a project,
   also those started in a terminal; "Open in a terminal" continues one in Konsole.
5. "Ask Claude Code to review the pending changes, then have Codex continue from its
   report": two runs; the job of the second starts with the handover report.

## S38 worktree, review, notifications
1. **[auto]** A run with `worktree: true` in a temporary repository: the checkout and
   the branch exist during the run, the report names them, nothing is removed after.
2. **[auto]** The review: the files of the run, the branch, the status and the stat.
3. **[auto]** Notifications: after `notifyAfterMin` for `done`, always for a failure;
   the two channels by their keys.
4. The owner: a job with `worktree: true` leaves its branch for a look; the review
   block is in the header of its session.

## S39 automation
1. **[auto]** A `coder` task starts a run at its time and records the report; under
   `onApproval: deny` an `edit` profile is not started; the timeout; delivery.
2. The owner: a paused `coder` task, run now from the Tasks tab, delivers its report.

## S40 models and providers
1. **[auto]** The resolver: a smart entry takes the rules under its patch; a manual
   entry ignores the rules for the six editable fields and keeps their request patch.
2. **[auto]** A fetch on a list that holds `gemini-3.8-flash`: `models/gemini-3.8-flash`
   is not added; `models/gemini-3.8-flash-tts` arrives disabled with no role;
   `models/gemini-4-neon` arrives disabled with role `chat`.
3. **[auto]** A file whose rows were marked `chat` by the old fetch: at load the
   text-to-speech, image and music rows lose the role and the live rows get `live`; a
   row that is the `chatModel` is left alone; a second load changes nothing.
4. **[auto]** `PUT` with `smart: false` and no `maxOutputTokens`: 400 naming
   `maxOutputTokens`, the file unchanged. `removeModels` of the `fallbackModel`: 409
   naming the role.
5. **[auto]** Scripted model, 429 with "quota": with the fallback's provider off the
   turn fails with the 429 text; with it on the notice says "out of quota".
6. Headless Chrome (dark and light, 1200x820 and 700x500): the pencil opens the dialog
   with the inherited values as placeholders; a typed context window saves as an
   override (accent border, the badge of the row changes); clearing it removes the
   override.
7. Headless: Smart off fills the fields; Save with an empty Max output tokens is
   refused and that field has the focus; a valid save sends the six fields.
8. Headless: a dragged model row and a dragged provider keep their order after a
   reload of the page.
9. Headless: the Defaults card shows "provider switched off" under a fallback whose
   provider is off; leaving the name field of a provider saves it with one `PUT`. A
   dropdown of the Defaults card opens in the colours of the theme (dark and light),
   picks with the mouse and with the keyboard (open, ArrowDown, Enter), closes with
   Escape and gives the focus back to its trigger, and closes on a click outside.
10. The owner, on the real Gemini provider after a restart (and after the copy of
    `providers.json`, section G): Fetch models adds no `models/` twin; the
    text-to-speech, image and music rows have no role badge; a chat turn while the key
    is over quota ends with the quota message in the header notice.

## S41 model picker in the composer
1. **[auto]** `model_api.rs`: each 400 of `picker.md` section 3 with its message, the 409,
   the 404; a key left out keeps its value; a level is cleared when the model changes to
   one that does not offer it; `POST /api/sessions` with both; a refused create makes no
   session.
2. **[auto]** Scripted model: a session's choice replaces the model, the window and the
   output limit of the request and sends the level; a choice that cannot be used makes
   the default answer and a notice says why; a level the model does not offer is not
   sent; a subagent keeps the model of its job.
3. **[auto]** The fields round-trip; a meta from before S41 loads; `unusable` names the
   reason; an old `options.reasoningLevel` survives a load and a save.
4. Headless Chrome (dark and light, 1200x820 and 700x500): the trigger shows the model
   and the effort; Ctrl+M opens the menu; a pick sends one PUT and the trigger changes;
   the check sits on the current one; the image icon shows where the model reads images;
   the slider is hidden for a model without levels and sends one PUT per change; "Manage
   models" opens Settings at Models; "Default" clears the choice.
5. Headless: a new conversation starts with the last choice; a remembered model that is
   gone is not sent.
6. Headless, in the widget (340x460): the chip opens the sheet; a pick applies and the
   sheet stays open; Escape closes it; Space and M do nothing to the voice while it is
   open; Ctrl+M opens it and does not mute.
7. Headless: the header has no model dot; with no usable model the trigger reads "No
   model" and Send is disabled; with no default but a usable choice, Send is enabled.
8. The owner, live: pick another model in one conversation and ask something; the answer
   comes from that model, the other conversations keep the default; a task and a Telegram
   chat keep the default too.

Items 4 to 7 are `scripts/test-picker-web.py` (103 checks, with the keyboard and mouse rules
of `picker.md` section 5.3). Item 8 is the owner's.

## S42 left sidebar and projects
Spec: `sidebar.md` section 7 and its amendment of 2026-11-27. Items 1 to 4 are `[auto]`.
1. **[auto]** `project_api.rs`: each refusal of section 3 with its message (the shape of the
   body, a relative path, a folder that is not there, KAgent's own folder, both filters, the
   404, the two 409); `~` is expanded and the canonical path (a link is resolved) is stored;
   detach; pin and unpin; the event when something changed and none when nothing did;
   `GET /api/projects` (the markers, `agentsMd`, the roots); the list of a folder.
2. **[auto]** `agent/project.rs` and `tests/project.rs`: the block with and without
   `AGENTS.md`; the cut at 4000 characters; a link to a credential file gives none; a missing
   file and a folder named `AGENTS.md` give none; a session without a folder has no block.
3. **[auto]** Scripted model: every request of a session with a folder carries the block and
   one without does not; a folder attached or detached between turns is in the next turn only;
   a child of a session with a folder has the folder in its meta and the block after its job's
   block. (`coder_run` and `start_coder` are phase 5, not in the tree: `coders.md` says it.)
4. **[auto]** The fields round-trip under `project` and `pinned`; a meta from before S42 loads
   with `None` and `false`.
5. Headless Chrome (dark and light, 1200x820): no tab row; the sidebar lists the seven views
   and a click opens one; Projects | Recent; pinned on top, a group per folder, This machine
   last; the rail; the state, the view and the closed folders survive a reload.
6. Headless at 700x500: the sidebar is a drawer; the toggle, Escape and a click on the scrim
   close it; the focus comes back to the toggle; a row and a view close it.
7. Headless: Attach folder lists the projects and attaches one with one PUT; a typed path that
   is not a folder shows the daemon's message and stays; Detach folder and `/project` alone
   detach; `/project <path>` attaches; New session in this project sends one POST with
   `project`; pin and unpin move the row; the search finds a title and a folder; the keyboard.
8. Headless, in the widget (340x460): the sessions sheet opens; a session opens from it and the
   sheet closes; a view expands the window; the row menu works in the sheet.
9. Headless, the amendment: the header is 36 px and holds exactly the toggle, KAgent, Stop all,
   the voice chip and the window controls; the permission mode changes from the foot of the
   sidebar in the full, the collapsed and the drawer state; the theme button goes through
   light, dark and auto; the keyboard order is the toggle, Stop all, the voice chip (the window
   controls in the app window), then the sidebar.
10. The owner, live: attach a folder that has an `AGENTS.md` and ask "what are the rules of
    this project": the answer quotes them.

Items 5 to 9 are `scripts/test-sidebar-web.py` (162 checks).

## S43 work windows: agents, plan, progress, and notices as toasts
Specs: `work-windows.md` section 7 and `notices.md` section 3. Items 1 to 3 and 9 to 11 are `[auto]`.
1. **[auto]** `tools/background`: `list` gives the running tasks first and the ended ones with
   the one that ended last first, with the command and `startedAt`, only those of the session
   asked, at most 50; the sink sees `running` at the start, `killed` on a stop (and nothing
   more at the end of the group) and the final status at the end, before the notification; a
   registry without a sink behaves as before (the tests of S31 unchanged).
2. **[auto]** `background_api.rs`: the 404s, the 400 of a `tail` out of range, the 409 of a task
   that ended; `output` gives the tail and nothing of a token planted in the log.
3. **[auto]** Scripted model (`tests/background.rs`): `shell {background: true}` gives
   `backgroundTask running` and the list holds the command; at the end the final status comes
   before the notification of S31; `stop_all` gives `killed` and tells no session.
4. Headless Chrome (dark and light, 1200x820): with two subagents and a todo list the capsule
   shows `2` and `3/7`; a click opens the panel; Agents lists both with state, time and Stop
   (the route of S31); Background lists the commands and their state; Plan says off, planning
   with Cancel plan, then the approved plan; Progress ticks and clears as before; a folded
   section stays folded after a reload.
5. Headless: a click on an Agents card opens the Subagent tab and the conversation stays; more
   tabs make a tab bar; a tab closes with its x or with Escape; the Plan tab shows the plan
   (the approved one, or the one that waits with the note); the Output tab follows a running
   command, stops following when scrolled up, resumes with the button and stops reading after
   the end event; tabs belong to their session.
6. Headless at 700x500: the panel leaves the composer its room; the pane is an overlay that
   closes with its button or the scrim.
7. Headless, in the widget (340x460): the capsule above the bar opens the sheet Work with the
   four sections; Stop works there; no line opens a pane.
8. The owner, live: "Research in parallel what uses the disk and what uses the memory, then
   summarize", with a background command: the capsule counts, the pane follows a subagent and
   the output, Stop ends one.
9. **[auto]** The notice event: the class of a provider error (`quota`, `key`, `busy`, `down`,
   `other`) and its key `provider:<id>:<class>`; the event carries `key`, `sessionId` and
   `kind` only when it has them; a recovery has the key of the error, comes once, and never
   without a previous error (`agent/notice.rs`, `tests/notices.rs`).
10. Headless (dark and light, 1200x820 and 340x460): two errors of one key are one toast
    "x2"; a recovery replaces the error and goes after 6 s; an error stays; hover holds the
    timer; close; Dismiss all with two or more; the fifth folds into "1 more"; Escape closes
    the focused one; the tab order; a long text is cut at three lines with "more"; "Open
    conversation" only for another session; reduced motion drops the slide; the header has no
    notice line; a live voice session says an error once per key in its status line.
11. The owner, live: with the chat model out of quota, one turn shows the quota toast with the
    fallback; the next turn that the chat model answers shows "answers again" for 6 s.

Items 4 to 7 and 10 are `scripts/test-work-web.py` (the daemon is stood in for when the
owner's own holds port 7878).

## S44 settings by voice and the voice rules
Spec: `voice-control.md` section 7. Items 1 to 7 are `[auto]`; the last two are the owner's.
1. **[auto]** `settings get` masks the secrets; `models` lists the refs and the jobs
   they hold and no key; `set` of two keys applies both or none (a bad second value
   leaves the first unchanged, in the file too); an unknown key names the three nearest;
   `chatModel` with a live model, or one without the chat role, is refused with the
   route's message; the card summary reads `change setting <key> from <old> to <new>`.
2. **[auto]** Every protected key (`PROTECTED_KEYS`) and every key below one is refused
   with "changed on the screen only"; a walk over the keys of the defaults and of each
   section fails when a key is in neither the protected nor the free list.
3. **[auto]** `always_ask` from voice (a live call, or a call inside a `Via::Voice` turn)
   raises a card in `full` mode; from chat in `full` it does not.
4. **[auto]** `voice.volume` is 0..100 (101 and -5 are refused), default 100; `voice.brain`
   defaults to `live`, also for a section without the key; the median of the last 5
   brain turns is in `GET /api/state` (`voice.thinkMedianMs`).
5. **[auto]** The prompt carries the Brain and `settings` lines while the tool is offered
   and the name line while `persona.address` is empty; `live` and `auto` carry them once;
   the greeting without a name ends with "What should I call you?".
6. **[auto]** `scripts/test-live.py`: after `PUT liveVoice`, the next `turnComplete` rolls
   the connection and the second setup has the new voice and the resumption handle;
   `settings` is declared in `live` and `auto`; `thinkMedianMs` is a number after a
   `routed` turn.
7. **[auto]** A voice change whose resumption the server refuses connects fresh (the
   decision is `voice_roll::refused`; the fake Gemini of the script does not refuse).
8. Headless Chrome: the Volume slider in Settings -> Voice moves the gain at once and
   saves on release; under `routed` and `auto` the line "Every request waits for the Brain
   (<model id>)." shows, and turns into the warning over 4 s.
9. The owner, by voice: "switch your voice to Puck" (said after its sentence, then the new
   voice), "a bit quieter", "set the Brain to Nemotron", "change the permission mode to
   read-only" (refused, said why); every change is asked out loud first.

## S45 Google Calendar and the morning report
Spec: `calendar.md` section 9. Items 1 to 8 are `[auto]` (the fake Google, no network); the last is the owner's.
1. **[auto]** The sign-in: the URL holds the two scopes, the S256 challenge and the
   loopback redirect and no secret; a callback with an unknown, used or older-than-10-minutes
   `state` is refused before Google is asked; the exchange stores the token (a 0600 file under
   `KAGENT_HOME`, one JSON text, no access token); a refresh a minute before the end and on a
   401; `invalid_grant` gives `reconnect`, one notice, nothing deleted; Disconnect revokes at
   Google and keeps the client. The first connection switches the module on and adds the group
   to `toolGroups` and `voice.toolGroups`; a second one leaves the owner's choices alone.
2. **[auto]** Only the switched-on calendars exist: a calendar that is off is not asked (no
   request reaches the fake for it), refused by name or id, and the client itself refuses a
   calendar it was not given; paging reads every page; a timed event is shown in the machine's
   time zone with the weekday, an all-day event as such, overlaps between timed events, one
   instance of a series; Google failing is an error, never an empty list.
3. **[auto]** `calendar_change`: the insert body (location, the Maps link as the first line of
   the description, reminder, the Meet request, guests with `sendUpdates=all`, `none`
   without guests); each refusal sends nothing; the three sentences are built from the stored
   event (the model's wrong title does not reach the card); `add` and `update` ask from voice
   in `full` and not from chat, `delete` asks everywhere and is destructive; the journal
   record and the undo of an add, an update and a delete.
4. **[auto]** Places: the field mask, the key in a header only, the bias to `near` or to
   `home` (resolved once); without a key, and after a failed call, the web gives the address
   (`source: "web"`, "I could not check this on the map"); the key is in no error; the drive
   asks for the no-traffic time, then traffic at the departure.
5. **[auto]** The report: the text for a day with an overlap, an all-day event and no events,
   in Slovenian and English; leave-by arithmetic (traffic plus margin); the timer's days and
   times, once a day, the late start (delivered before noon, skipped after); the spoken report
   is given once, and only on its day; Telegram is left out when no chat is paired; one
   notice when the calendar is not connected, not one a day.
6. **[auto]** No token, no key and no client secret is in a tool result, a view, an error or a
   log line of a whole session against the fake (`calendar/tests/secrets.rs`); the files
   `calendar.json` and `maps.secret` are credential files of the token guard.
7. **[auto]** `scripts/test-live.py`: a waiting morning report reaches the first live session
   as a `[SAY]` text turn after the opening, once; the `SAY` sentence is in the `live`
   instruction.
8. **[auto]** `scripts/test-calendar-web.py`: the Calendar page in headless Chrome, dark and
   light at 1200x820 and 700x500, in the seven connection states; Connect opens Google's
   address; the read-only calendar cannot be the default; Telegram is disabled until a chat
   is paired; Send now shows the text.
9. The owner, with a real Google account: connect; "what's on today?"; "let's add an event" by
   voice through the interview with a place; move it; delete it with "confirm"; "Send now";
   the report spoken the next morning (and on Telegram at its time).

## S46 Images in chat and "look at my screen"
Spec: `vision.md`. Items 1 to 8 are `[auto]` (scripted models, fakes on loopback, headless Chrome); the last is the owner's.
1. **[auto]** The store: an image round-trips through the session file and a restart (the line has
   the id, never the pixels); a screenshot stays PNG and a photograph becomes JPEG 85; the longest
   side is cut to 1568 (a live frame to 1024); EXIF and text chunks do not survive; GIF and PDF are
   refused as a type, a byte over 8 MB as too large, a cut file as unreadable; five images in a
   message, another session's image and an id that is a path are refused; the folder goes with the
   session.
2. **[auto]** The routes (`api/images/tests.rs`, over loopback): an upload answers the `ImageRef`; 415,
   413 (the body limit of the route), 400 without the field `file`, 404 for an unknown session; a
   message names its images by id, at most four.
3. **[auto]** The readers (`kagent-core/tests/vision.rs`): a Brain with `supportsImage` gets the stored
   file as pixels and no vision call is made; one without gets the description of the vision model
   once (`[image 1, read by <model>: ...]`), stored in the message, and the next turn asks nothing; with
   no vision model the turn is refused before it starts (the notice of the spec, nothing stored, no
   model asked); a failing vision model fails the turn with what to do; only the last 6 messages
   carry pixels, older ones go as a line; compaction summarizes descriptions, never pixels.
4. **[auto]** The adapters pin the request body of one image in each of the four formats; an image
   with no pixels is not sent, and no empty text part stands beside an image.
5. **[auto]** Telegram: a photo with a caption is an image of the message with the caption as its text, the
   biggest size that a bot can download, also as steering while a turn runs; a document still gets the
   old answer.
6. **[auto]** Live voice (`screen/live/tests.rs`, `confirm.rs`, `instruction.rs`): `screen` is declared in
   `live` and `auto` whatever `voice.toolGroups` holds, with `target` only, and not in `routed`; a live
   call sends one JPEG frame of at most 1024 px and answers ok, and the screenshot file is deleted; a
   locked screen sends nothing; the first look is asked with the sentence of the spec and the yes covers
   the later ones in that session, a no covers nothing; the instruction line is in `live` and `auto`.
7. **[auto]** `scripts/test-vision-web.py`: paste, drop, the attach button, remove, the fifth image and a GIF
   refused, Send with the ids, an images-only message, the thumbnail (56 px), the image of a message (at
   most 320 px), the full view (Escape and the backdrop close it), the line "Images are sent to <provider>."
   once per session and not for a local provider, a refused message comes back; dark and light at 1200x820
   and 340x460; screenshots in `/tmp/kagent-s46-shots/`.
8. **[auto, written, not run with the owner's daemon up]** `scripts/test-live.py` scenario `look`: the first
   `screen` call is asked and sends no frame; after a spoken yes one JPEG video frame precedes the answer
   `{"status": "ok", "output": "the screenshot is in your view now"}`; the next look is not asked; a locked
   screen answers "the screen is locked" with no frame.
9. The owner: paste a screenshot of an error into a chat and ask what it says; by voice, "look at my
   screen, what is this dialog?"; say yes once and look again without being asked.

## S48 Screen watch
1. **[auto]** The loop (`screen_watch/tests/hits.rs`, a fake capture and a scripted vision model, paused time): a hit on the third look
   ends a `once` watch with one message; `repeat` reports and not more often than every 30 s and ends with the time; the end says the
   last note and the minutes; stop says nothing; low confidence needs a second look 2 s later, never skipped, and the second look's
   note is the report; `cloudConfirm` sends exactly one frame per hit (none for a look that missed, none when off), a "no" from the
   cloud model is not a hit, a failed cloud answer is not asked again.
2. **[auto]** The guards (`tests/guards.rs`): a second start is refused with the running goal (also two at once); a closed window fails
   the watch ("the window was closed"); five unreadable answers in a row fail it and a good answer resets the count; a model that does
   not answer counts the same; an idle screen costs a model look in seven; a locked screen pauses it, counts no looks and the clock
   runs on; a card on the screen stops a monitor watch from looking; slow looks double the interval and say so; a turn thinking on the
   model makes it wait; limits of minutes, interval and goal; the 2 hour cap (120 looks at 60 s, then expiry); the defaults come from
   the settings; refused switched off, without a local model, or on KAgent itself.
3. **[auto]** `look` (`look/tests.rs`): the 1.5 % edge, the limit of 6, a forced look, the JPEG of at most 1024; the prompt; the JSON
   reply with a fence, words around it, strings and percents; every malformed reply is unreadable; a missing confidence needs the
   second look; a note is 12 words.
4. **[auto]** The model and "local" (`tests/models.rs`, `vision/tests.rs`, `local_models/vision.rs`): the refusal names Settings -> Local
   models and the recommendation; an installed vision model of the list is used; the `visionModel` setting wins when its provider is
   local and is never used when it is a cloud one; Ollama at a public address is not local; the address table of `is_local_url`.
5. **[auto]** Telling (`tell/tests.rs`, `tests/telling.rs`, `say.rs`): a hit is a session message `via: "watch"` and a desktop notification;
   spoken as `[SAY]` (without the goal) when a live session is open and then no notification; Telegram with or without the voice; the
   restart note is a session message only, told once; the stop phrases in English and Slovenian with their negations, in pieces; the kill
   switch (and the tray and KRunner) ends the watch; frames never reach disk: the home is listed before and after a run with the real
   capture function, only the marker exists meanwhile and it holds the goal.
6. **[auto]** Tool and settings (`tests/tool.rs`, `screen_watch/settings.rs`, `settings_tool/tests.rs`): the summary, `always_ask` and what
   from voice asks in every mode, status and stop ask nothing, the calls through the tool, refusal and bad input; the defaults, the
   ranges, a typo; `screenWatch.cloudConfirm` and `.telegram` are refused to the `settings` tool.
7. **[auto, gated]** `KAGENT_IT=1 cargo test the_prompt_against_the_real_local_model`: the look's prompt against the real `gemma3:4b` in
   the local Ollama on two generated pictures (a green circle that meets the goal, a blue square that does not): the reply parses as the
   JSON shape; the raw replies and the time of a look are printed.
8. **[auto]** `scripts/test-screen-watch-web.py` (62 checks): the chip from the daemon and from the event, its minutes, title and
   popover, the countdown, paused, every end state, Stop, every view, the widget, the card and each save, the refusal and its link; dark
   and light at 1200x820 and 340x460; screenshots in `/tmp/kagent-s48-shots/`.
9. The owner: with `gemma3:4b` set for vision, say "tell me when this build finishes" and run `cargo build`; the chip shows, the minutes
   count down, and when the build ends the widget says so; "stop watching" ends one. Not run by the agent: the real capture of a window
   (`xwd`) and of a monitor (`spectacle`), the real notification, the Telegram message, the plasmoid's eye, and `scripts/test-live.py`
   (it needs port 7878): a live check that a hit is spoken as `[SAY]` and that `screen_watch start` is asked out loud belongs there.

## S47 Recommended local models
1. **[auto]** The probe (`kagent-core platform::gpu`): `nvidia-smi` output of one card (recorded on the owner's RTX 3060), two
   cards (the larger wins), a card that says `N/A`, a name with a comma, garbage; the amdgpu sysfs files (bytes, the largest AMD
   card, other vendors skipped, connector entries skipped); `MemTotal`; no card is `None`.
2. **[auto]** The list and the rules (`local_models/fit.rs`): `config/local-models.json` parses with `deny_unknown_fields`, ids are
   unique, only speech models are not downloadable; the fit at its edges (85 %, 100 %, no card: half of RAM); the table for 12 GB,
   8 GB, 4 GB and no card; the owner's language wins among the models that fit; a chat model without tools is never the
   recommended Brain.
3. **[auto]** The machine answer against a fake Ollama (`local_models/tests.rs`): reachable with its version, the card, RAM, the
   installed models with their jobs, `:latest` read as the listed id, the fit and the "in use by other programs" line, no card,
   Ollama not reachable as a state, the base address from the Ollama provider, the tool's text (and that it pulls nothing).
4. **[auto]** Downloads (`local_models/tests_pull.rs`, the fake streams Ollama's lines): an id outside the list and a speech model are
   400 and Ollama is not asked; a pull adds the Ollama provider from its template, the row is on with the roles of its jobs
   (`chat` with `supportsImage` for vision, none for an embedding) and nothing is made the Brain, fallback or vision model; the
   role check accepts it as the vision model; progress is the sum of the layers and events come at most twice a second; the
   disk check (one MB short is 409 with both numbers, exactly enough passes, unknown passes); one pull at a time with the
   others queued, in order; cancel of a running pull closes the stream, of a queued one leaves the queue; a failed stream and a
   stream that ends early keep their error on the row, Retry replaces it, Dismiss removes it; Ollama down is 409 with what to
   do and adds no provider; a model Ollama already has only gets its row; remove is 409 while a setting names the model and
   deletes it from Ollama and the providers otherwise.
5. **[auto]** `scripts/test-local-models-web.py` (58 checks): each fit chip, Recommended first, a running download with
   progress from the event, Cancel, a waiting and a failed row with Retry, "Use for vision" and "Use as fallback" (the saved
   keys), Remove with its confirmation, the 4 GB, no-card and no-Ollama states; dark and light at 1200x820 and 700x500;
   screenshots in `/tmp/kagent-s47-shots/`.
6. The owner: open Settings -> Local models; with a card that has room, press Download on Gemma 3 4B and watch the bar; when it
   is done press "Use for vision"; ask for a look at a screenshot in a chat.
