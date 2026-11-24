# KAgent - project plan

Status: plan only, nothing implemented yet. Stages to build it: `docs/IMPLEMENTATION.md`
(its crate layout and root-helper decision supersede section 3 here).
Last update: 2026-11-23.

## 0. Vision

One personal assistant for the whole Linux desktop: Siri's "just say it", ChatGPT/Claude's
depth, Gemini Live's natural, interruptible voice, and a sci-fi presence (JARVIS, KITT,
HAL), with real low-level access to the machine. You talk; it inspects the system, fixes
things, organizes your day, writes your emails in your words, runs coding agents, asks a
council of models for hard decisions and tracks your work time. Anything risky or
outward-facing still waits for your "yes".

It is built on the open standards the agent ecosystem already uses (section 4), so it
works with Claude Code, Codex, ZCode, MCP servers, other agents and voice devices
instead of being another closed silo.

## 0.1 Focus (read this first)

KAgent is worth building only where it beats ChatGPT, Claude or Gemini in a browser:
**it sits on the machine, sees the kernel and the services, and can change them safely.**
Everything else is parked (section 18) until this core is excellent.

What the core does, in order of value:

1. **Diagnose the machine from real data.** "Why is it slow / no sound / wifi drops /
   boot takes 40 s / battery drains / fan screams / disk full / the app crashed?"
   Correlate `/proc`, `/sys`, journal and kernel log, `systemd-analyze`, sensors,
   SMART, PipeWire (`pw-dump`), NetworkManager, UPower, BlueZ, GPU (`nvidia-smi`),
   coredumps (`coredumpctl`). A cloud chatbot can only guess; KAgent measures.
2. **Change it safely, with undo.** Typed actions for sysctl, kernel modules and their
   parameters, services, packages, drivers (`ubuntu-drivers`, DKMS), config files,
   power profiles, swap/zram, CPU governor. Before every change: dry run where
   possible, record the undo, optional btrfs/snapper snapshot. After: measure again
   ("boot went from 41 s to 23 s").
3. **Control the desktop and hardware by voice or text.** Over D-Bus, not screen
   scraping: audio output and volume (PipeWire), Bluetooth (BlueZ), Wi-Fi/VPN
   (NetworkManager), brightness and power (UPower, power-profiles-daemon), windows,
   virtual desktops, dark mode, notifications (KWin, Plasma). "Connect my headphones
   and switch output to them" is one sentence instead of four menus.
4. **Watch and warn.** Daemon watchers on kernel and journal events: OOM kills, kernel
   warnings and taint, failed units, DKMS build failures after a kernel update,
   SMART errors, thermal throttling, disk filling up, AppArmor denials, repeated SSH
   login failures, pending security updates. It tells you once, with the cause and a
   proposed fix.
5. **Security posture and intrusion detection.** Honeytokens, persistence and behaviour
   watch on the PC, traffic monitoring and traps on the MikroTik router (section 7.4).
   Also on demand: Listening ports and the process behind each,
   firewall state, sudoers, users, failed logins, kernel lockdown/Secure Boot, what is
   using the mic, camera or network right now.
6. **Lend its system tools to your coding agents.** MCP server mode: Claude Code / ZCode
   / Codex use KAgent's typed, audited system tools instead of raw `sudo` in a shell.
   Cheap to build, immediately useful.

Voice (Gemini Live via the daemon, ReelGen basis) is the interface for 1-5, with a
simple audio-reactive orb. It is a means, not a separate product.

## 1. Principles

1. **Daemon owns all side effects.** Every surface (overlay, chat, phone, voice) is a
   client of one daemon that holds the agent, tools, permissions, memory and audit.
2. **One owner per state, one write path** (ZCode rule).
3. **Autonomy by action type, not by global switch** (section 7.2).
4. **Local first.** Audio, webcam frames, activity data and memory stay on the machine.
   Only what a provider call needs leaves it.
5. **Everything is auditable and, where possible, undoable.**
6. **Standards over invention** (section 4).
7. **Low-level value first.** Build only what needs to live on the machine (section
   0.1); park the rest.

## 2. Platform

- **Core: Linux in general.** Daemon, tools (`/proc`, `/sys`, sysctl, systemd, journal,
  polkit, `ip`, `ss`) work on any systemd distro, with or without a desktop.
- **First and only tested target: Ubuntu 26.04 + KDE Plasma 6** (the dev machine, X11
  session today, Wayland expected later).
- **Distro modules** for the few distro-specific parts: package manager (apt + snap on
  Ubuntu), firewall (ufw), packaging (.deb). Fedora (dnf, firewalld) and Arch (pacman)
  are one module each later.
- **Desktop adapter** over D-Bus, KDE first with freedesktop fallbacks:

| Feature | KDE | Generic fallback |
|---|---|---|
| Global hotkey / push to talk | KGlobalAccel | GUI button |
| Tray (status, stop-all) | StatusNotifierItem | same spec |
| Notifications | KNotification | `org.freedesktop.Notifications` |
| Launcher ("ask KAgent...") | KRunner D-Bus runner | none |
| Root password prompt | polkit-kde-agent | any polkit agent |
| Secrets (API keys, OAuth tokens) | KWallet via Secret Service | 0600 file |
| Window / desktop control, active window for time tracking | KWin scripting | `xdotool` (X11 only) |
| Overlay placement | KWin window rules | always-on-top hint |
| Plasma widget | plasmoid: status + mic | none |
| PIM | Akonadi (KMail, KOrganizer) | IMAP/SMTP, CalDAV |

KWin is chosen over X11 tools so it keeps working on Wayland.

## 3. Architecture

```
 Surfaces (clients)                         kagentd (Rust, systemd user service)
 ┌───────────────────────────┐   AG-UI     ┌──────────────────────────────────────────────┐
 │ Tauri app (React):        │  events     │ gateway: HTTP + WS (AG-UI), auth, Host check │
 │  - presence overlay       │◀──────────▶ │ agent loop · scheduler · permission broker   │
 │  - chat window            │             │ providers (registry, adapters, live)         │
 │  - control center         │             │ tools (system, kernel, fs, desktop, PIM...)  │
 │ Browser (same React app)  │             │ memory · skills · cron/heartbeat             │
 │ Phone: Telegram / Signal  │◀─ channels ─│ council · coding conductor (ACP)             │
 │ Voice satellites (Wyoming)│◀─ audio ───▶│ voice: live proxy, wake word, local pipeline │
 │ KRunner / plasmoid        │◀─ D-Bus ───▶│ desktop adapter · webcam presence            │
 │ Claude Code / ZCode (MCP) │◀─ MCP ─────▶│ audit log · change journal (undo)            │
 └───────────────────────────┘             └───────┬───────────────┬──────────────────────┘
                                                   ▼               ▼
                                AI providers (any)     sudo -n / pkexec (per call)
```

Crates
- `kagent-core`: protocol, tool contracts, scheduler, permission engine + broker, agent
  loop, sessions, audit, change journal.
- `kagent-providers`: provider registry, model rules, adapters, live protocols, catalog.
- `kagent-tools`: system, kernel, fs, network, package, desktop, PIM, webcam tools.
- `kagent-voice`: live proxy (Gemini Live, OpenAI Realtime), wake word, VAD/STT/TTS
  pipeline, Wyoming server.
- `kagent-agents`: memory, skills, cron/heartbeat, council, coding conductor (ACP),
  time tracking, channels.
- `kagentd`: binary that wires it all and serves the API.
- `app/`: Tauri shell + React UI (also served to browsers).

Single owners

| State | Owner | Persisted at |
|---|---|---|
| Provider config | `ProviderStore` | `~/.config/kagent/providers.json` (keys in Secret Service) |
| Settings, persona, autonomy | `Settings` | `~/.config/kagent/settings.json` |
| Sessions | `Session` | `~/.local/share/kagent/sessions/<id>.jsonl` |
| Audit chain | `AuditLog` | `~/.local/share/kagent/audit.jsonl` |
| Change journal (undo) | `ChangeJournal` | `~/.local/share/kagent/changes/` |
| Memory | `Memory` | `~/.local/share/kagent/memory/` (Markdown files + index) |
| Skills | `SkillStore` | `~/.config/kagent/skills/<name>/SKILL.md` |
| Activity timeline | `ActivityTracker` | `~/.local/share/kagent/activity.sqlite` |
| Pending approvals, live sockets | broker / voice | memory only |

## 4. Standards KAgent adopts

| Standard | Used for | Why |
|---|---|---|
| **MCP** (client + server) | Client: add any MCP server as a tool source. Server: expose KAgent's system tools to Claude Code, ZCode, Codex, Goose | Universal tool protocol; KAgent's permissions and audit apply in both directions |
| **ACP** (Agent Client Protocol, Zed) | Drive Claude Code (via `claude-code-acp`), Gemini CLI, Goose and other coding agents with structured progress | JSON-RPC over stdio, editor-agnostic; headless CLI (`claude -p --output-format stream-json`) as fallback |
| **A2A** (Agent2Agent, Linux Foundation) | KAgent publishes an Agent Card; council members and remote agents can be A2A agents | Vendor-neutral agent-to-agent tasks with streaming |
| **AG-UI** | Event stream between daemon and every UI surface (lifecycle, text, tool call, state events) | Typed event protocol; other AG-UI frontends can attach |
| **SKILL.md skills** | Skills folder format compatible with Claude Code / OpenClaw / Hermes skills | Reuse existing skills (e.g. your `logging-hours`, `marko-voice`) |
| **AGENTS.md** | Per-project instructions when KAgent or a delegated agent works in a repo | Same file every coding agent reads |
| **Wyoming** | Voice satellites (phone, Raspberry Pi, second room) and local STT/TTS/wake-word services | Simple JSONL + PCM protocol from the Home Assistant voice stack |
| **OpenAI-compatible API** | Optional local `/v1/chat/completions` over the configured providers | Other apps can use KAgent's providers |
| **Provider API types** | `anthropic-messages`, `openai-chat-completions`, `openai-responses` (same as ZCode) | Covers practically every provider |
| **models.dev catalog** | Context limits, output limits, prices, reasoning flags | Same source opencode and ZCode use |
| **SIP / RTP via Asterisk ARI + AudioSocket** | Phone line, parallel ringing, call audio to the agent | Mature telephony stack; KAgent stays out of SIP/NAT/codec details |
| **MPRIS** | Pause/resume media when a call or voice session starts | Standard D-Bus media control |
| **freedesktop** | Notifications, StatusNotifierItem, Secret Service, polkit, D-Bus | Works beyond KDE |

## 5. Providers (reusing ZCode's provider logic)

What ZCode does (`packages/provider`, `config/provider/zcode-builtin.json`):
- A provider has access (API key), an API (`type`, `baseUrl`, `headers`) and model ids.
- 21 templates act as the "add provider" picker. Personal rules overlay the built-in ones.
- Model config is layered in a fixed order: `modelRules` (regex) -> `modelApiRules`
  (+ API type) -> `providerSiteRules` (+ baseUrl) -> `templateModelRules` ->
  `providerModelRules` (personal).
- `optionSpecs` (reasoning level, max output) map options to request fields with a
  CEL-like expression.

KAgent keeps: the same API types, the template/provider/model split, the layering order
and the capability fields. `scripts/import-zcode-providers.py` converts ZCode's built-in
file so templates stay in sync.

KAgent improves:
- **No expression language**: option mapping is a declarative JSON patch per value,
  with per-API defaults. Editable in the GUI, nothing to inject.
- **Flat personal file** like ReelGen: `{order, providers: {id: {name, templateId, api,
  models[], enabled}}}`. Keys live in Secret Service, not in the file.
- **Model discovery**: "Fetch models" from `/models`, `/v1/models`, Ollama `/api/tags`.
- **Model reference** `"<providerId>::<modelId>"` everywhere.
- **Capabilities per model**: chat, tools, vision, `live` (realtime voice, protocol
  `gemini-live` or `openai-realtime`), `embedding`, `stt`, `tts`.
- **Roles for models**: chat, fast (quick answers, routing), live, vision, embedding,
  council members, coding reviewer. Each picked from the same registry.
- **Extra templates**: Google Gemini (+ live models and voices), Ollama, LM Studio,
  Groq, Mistral, VoiceStudio (local TTS/STT).
- **models.dev catalog** (cached, never on the request path), usage and cost per
  session, and a **fallback model** (e.g. local Ollama) when the main provider fails.

## 6. Agent loop and tools (reusing ZCode's tool contracts)

Tool contract (port of ZCode `contracts/src/tools/contract.ts`): name, description,
input schema, side effect (`none | workspace | system | kernel | network | outward |
user_interaction`), read-only, destructive, concurrency-safe, needs approval, always
ask, privileged, risk, timeout, output budget, **undoable**, **dry-run support**.
Each call is also **assessed per input** (e.g. `systemd status` is read-only,
`systemd restart` is not).

Loop (port of ZCode `docs/design/v2/loop/tool-calling-loop.md`):
1. Append input. Steering input sent during a turn is drained only at a model-request
   boundary.
2. Stream the model: text and reasoning deltas, tool calls.
3. Schedule: consecutive concurrency-safe calls run in parallel; anything with side
   effects runs alone, in order.
4. Validate input (normalize common local-model mistakes first); errors go back to the
   model as tool results.
5. Permission check -> allow / deny / ask (section 7).
6. Execute with timeout and cancellation; truncate to budget (head + tail).
7. Every tool call gets exactly one result, synthetic on deny/skip/cancel.
8. Loop until a plain answer. No hard tool-call cap; cancellation, permissions,
   timeouts and a configurable iteration safety valve bound it.
9. **Tool budget** (Goose): only enabled tool groups go to the model; small local models
   get a core set.

Tool families

| Family | Tools (RO = no approval) | Phase |
|---|---|---|
| System | `system_info` RO, `process_list` RO, `process_signal`, `disk_info` RO, `network_info` RO, `hardware` RO (lspci, lsusb, sensors, SMART) | 1 |
| Kernel | `kernel_info` RO (version, cmdline, taint, modules, kernel log), `kernel_module` (info RO / load / unload, privileged), `sysctl` (get RO / set privileged) | 1 |
| Services | `systemd` (status/list RO, start/stop/restart/enable), `journal` RO, `timers` RO | 1 |
| Files | `read_file` RO, `list_dir` RO, `search_files` RO (grep/glob), `write_file`, `edit_file` / `apply_patch` (ZCode) | 1 |
| Shell | `shell` (argv or `bash -c`; read-only command detection auto-allows `ls`, `cat`, `ps`... without metacharacters; `sudo=true` always asks) | 1 |
| Packages, firewall | `package` (search/info RO, install/remove), `firewall` (list RO, change) | 2 |
| Desktop | `desktop` (windows, virtual desktops, open app/URL, clipboard, screenshot, Plasma settings via D-Bus), `dbus` (introspect RO, call), `udev_devices` RO, `watch_path` (inotify) | 2 |
| Containers | `containers` (docker/podman ps, logs RO; start/stop) | 2 |
| PIM | `mail` (search/read RO, draft, send = outward), `calendar`, `tasks`, `reminders`, `contacts` RO | 2 |
| Web | `web_search` RO, `web_fetch` RO | 2 |
| Delegation | `coding_task` (section 12), `council` (section 13), `subagent` | 2-3 |
| Memory | `memory_search` RO, `memory_save`, `skill_save` | 2 |
| Kernel tracing | `trace` RO: allowlisted bpftrace probes (syscall counts, open files, run-queue latency, TCP retransmits) for a few seconds | 4 |
| Undo | `undo_change` | 2 |

Diagnostic **playbooks** ship as skills: "why is the machine slow", "wifi is down",
"disk almost full", "failed boot units", "battery drains fast".

## 7. Safety, autonomy and audit

### 7.1 Permission engine
- Modes: `read-only`, `ask` (default), `trusted` (auto-allow unprivileged system changes).
  Switchable mid-session (Goose `/mode`).
- Approval scopes: **once / this session / always** (rule keyed by tool + prefix of
  the call's rule key), as in Codex.
- Per-tool overrides on top of the mode: always allow, always ask, never.
- Privileged or `always_ask` calls ask in every mode and are never satisfied by a rule.

### 7.2 Autonomy levels by action type

| Level | Actions | How you confirm |
|---|---|---|
| Automatic | Reading, inspecting, summarizing, drafting, sorting, reminders, calendar changes that only affect you | none |
| Spoken yes | Outward actions under your name: send email, reply, accept invite, message someone; unprivileged system changes in `ask` mode | "Send it?" -> "yes" (voice or click) |
| Click only | Root actions, kernel modules, sysctl set, package installs, anything destructive and not undoable | approval card |
| Never automatic | Commit, push, merge, release, Jira worklog writes | KAgent hands you the commands |

Optional per-recipient rules later (e.g. auto-send status replies to your team).

### 7.3 Mechanisms
- **Typed privileged actions** (SysKnife): privileged tools build their own argv from
  validated fields; only `shell` takes free text and it always asks with `sudo=true`.
- **Privilege**: daemon runs as you; per-call `sudo -n` (sudoers allowlist) or
  `pkexec` (polkit prompt). Never root.
- **Sandbox** (Codex): unprivileged `shell` runs under Landlock (read-only FS, writes
  only to /tmp and the work dir) + seccomp (network off unless asked). Sandbox and
  approval are separate settings.
- **Dry run** on approval cards where possible (`apt -s`, `nft -c`, old vs new sysctl).
- **Script check** (Open Interpreter safe mode): `shellcheck` and a plain-language
  explanation for multi-line scripts.
- **Change journal with undo** (SysKnife rollback): before a mutating call record what
  reverses it (old sysctl value, unit state, previous file, installed package list).
  "Undo" in the GUI and `undo_change`. Optional snapper/btrfs snapshot for big changes.
- **Hash-chained audit log** (SysKnife): every call, approval and result; each line
  holds the SHA-256 of the previous one; `kagentd audit verify`. Ed25519 later.
- **Kill switch**: GUI button, global hotkey, saying "stop" or an open-palm gesture
  cancels every turn and kills child processes.
- **Gateway security**: 127.0.0.1 only, `Host` check (DNS rebinding), bearer token
  (0600 file); the URL `http://127.0.0.1:7878/#token=...` is printed once. Channel
  bots (Telegram/Signal) accept only paired accounts (OpenClaw `doctor`-style check
  for risky settings).
- **Skill quarantine** (Hermes): skills from outside are inactive until reviewed.

### 7.4 Intrusion detection and deception (defensive only)

Goal: notice an intruder early, on this PC and on the LAN, with alerts you can trust.
Detect, contain, preserve evidence, notify. **No hacking back** (illegal, and it tips
the intruder off).

**On the PC: tripwires, no new attack surface**
- **Honeytokens** that nothing legitimate ever touches: decoy `~/.aws/credentials.legacy`,
  a fake SSH key `~/.ssh/id_backup`, a fake `passwords.kdbx`, a fake `.env` with API keys,
  a decoy browser profile folder. Watched with fanotify/inotify (later eBPF), so any read
  is a high-confidence alert with the process, user and parent chain.
- **Canary credentials**: decoy keys/tokens that are valid only at a trap (a fake
  account on the MikroTik, a Canarytokens AWS key). Using them reveals the intruder even
  from another machine.
- **Persistence watch**: new `authorized_keys` entries, new systemd units or timers,
  crontab changes, new SUID binaries, `LD_PRELOAD` / `ld.so.preload`, new kernel
  modules, changes to sudoers and PAM. File-integrity baseline (AIDE-style) of `/etc`,
  `/usr/bin`, `/boot`.
- **Behaviour watch** (eBPF in phase 3): a shell spawned by a service or browser, a
  process whose stdin/stdout is a network socket (reverse shell), binaries running
  from `/tmp` or `/dev/shm`, new listening ports, outbound connections to new countries
  or at odd hours.
- **Logins**: SSH failures and successes, new sudo users, logins while you are away.
- **Low-interaction honeypot** (optional): OpenCanary with fake SSH/SMB/HTTP, in a
  container on a separate VLAN or a Raspberry Pi, **never on this workstation and never
  exposed to the internet**. Any connection to it from inside the LAN is suspicious.
- **Protect the agent itself**: KAgent is a juicy target (it holds root actions). Its
  token, a heartbeat to the router/phone ("KAgent went silent" is an alert), audit log
  verification, and prompt-injection rules: text read from files, logs or the network is
  data, never instructions.

**On the LAN: MikroTik (RouterOS v7) adapter, traffic monitoring first**
- **Traffic Flow (NetFlow/IPFIX) export** from the router to a small collector in the
  daemon: per-device traffic, top talkers, new destinations, unusual ports, spikes,
  a device "phoning home" at night. Stored in SQLite with short retention.
- **REST API (HTTPS, LAN only)**: DHCP leases and ARP (alert on a new device), connection
  tracking, interface counters, DNS cache, firewall counters, router log.
- **Router-side traps**: port-scan detection (`psd` matcher -> address list "scanners"),
  hits on unused ports or the honeypot -> address list, failed router logins, use of the
  fake router account. KAgent reads these lists and explains them.
- **Containment with approval**: add an IP to a block list with a timeout, or move a
  device to a quarantine VLAN. Click-only, logged, undoable (remove from list).
- **Credentials**: two RouterOS users. `kagent-ro` (read, api, rest-api) from phase 1;
  `kagent-rw` (adds write) only when containment is enabled. Stored in Secret Service.
- Limits: `psd` catches automated scanners, not a careful attacker; flow data shows who
  talks to whom, not content (encrypted traffic stays opaque, which is fine).

**Alert quality rules**: every alert says what happened, how sure KAgent is, the
evidence (process tree, flow records, file, snapshot of state) and the proposed
response. Honeytoken and canary hits are "high"; behaviour and traffic anomalies start
as "review" until they repeat. Evidence is copied into the audit log so it survives
cleanup by the intruder.

## 8. Voice and presence

### 8.1 Live voice (reusing ReelGen's Gemini Live)
ReelGen (`frontend/src/lib/live.ts`, `reelgen/kitt/service.py`): the browser opens the
Gemini `BidiGenerateContent` socket, mic -> AudioWorklet -> 16 kHz PCM16, 24 kHz PCM16
playback queue, `interrupted` = barge-in, `goAway` = new session, tools bridged over HTTP,
approval-gated tools declined.

KAgent: **the daemon is the proxy** (browser/app <-> `kagentd` <-> Gemini).
- API key never leaves the daemon; works for AIza and express keys.
- `toolCall` runs through the same scheduler, permissions and audit; approvals appear
  in the overlay and the model says "waiting for your approval" instead of declining.
- **Session resumption**: store `sessionResumptionUpdate.newHandle`, reconnect with it on
  `goAway` (handles valid 2 h), so context survives rolls.
- **Context window compression** (sliding window) for sessions over 15 minutes (native
  audio is about 25 tokens/s).
- Transcripts go into the active session; voice and text share one conversation.
- Keeps ReelGen's audio code (worklet, sign-extended PCM decode, playback queue, flush
  on interrupt).
- **Screen awareness** (phase 2): "look at my screen" sends frames of the active window
  to the live model, only while you hold the hotkey or say so; a visible indicator.

### 8.2 Other voice paths
- **OpenAI Realtime** as second live protocol (phase 3).
- **Local pipeline** (phase 3): VAD -> local STT (whisper.cpp / VoiceStudio) -> any chat
  model -> local TTS (Kokoro / Piper / VoiceStudio). Works offline and with models
  that have no live mode.
- **Wake word in the daemon** (phase 3): rustpotter or oww-rs (openWakeWord models),
  16 kHz mono, 80 ms frames; audio never leaves the machine before the wake word.
- **Wyoming server** (phase 4): a phone or Raspberry Pi in another room becomes a voice
  satellite.
- **Spoken approvals**: "yes" approves "spoken yes" level actions only (7.2).

### 8.3 Presence overlay (the sci-fi part)
- Transparent always-on-top Tauri window, appears on wake word, hotkey or look-to-talk.
- **Audio-reactive visual** driven by real FFT of both voices (WebAudio AnalyserNode).
  Themes: orb (default), KITT scanner bar, HAL eye, waveform/spectrum ring.
- Subtitles of both sides; HUD cards for tool activity ("scanning processes... 3 found");
  approval cards in the overlay.
- **Persona**: name, personality (default calm and quietly witty, KITT-style), how it
  addresses you, language (Slovenian / English).
- **Voice**: pick from live-model voices; later a custom/cloned voice via VoiceStudio.
- **Voice effect** (optional, off by default): subtle synthetic tone, room reverb.
- **Sound design**: wake chime, thinking tone, boot sequence, alert tone.
- **Proactive speech** from heartbeat and watchers (section 9), held while you are away
  or in a meeting.

## 9. Memory, skills, scheduling

- **File memory** (Hermes): `MEMORY.md` (what it learned: machine profile, services you
  run, past fixes, decisions) and `USER.md` (you: preferences, people, projects, writing
  style). Small, human-readable, editable in the control center.
- **Memory as tools** (OpenClaw): `memory_search` / `memory_get` instead of loading
  everything into every prompt; only a short profile is always loaded.
- **Facts with time** (Graphiti idea, without Neo4j): facts carry `valid_from` /
  `valid_to`, so "Ana is my manager" can be superseded without losing history. SQLite
  + embeddings from the configured embedding model.
- **Session search** over past conversations.
- **Skills** (SKILL.md, compatible format): bundled playbooks; the agent writes a skill
  after solving something hard (Hermes); a **curator** job tracks usage, flags stale
  skills, archives unused ones. External skills are quarantined until reviewed.
- **Recipes / roles** (Goose, AIChat): instructions + tool groups + model + autonomy for a
  kind of work ("sysadmin", "writer", "organizer").
- **Heartbeat** (OpenClaw, default every 30 min, own session): checks watchers,
  follow-ups, unread priority mail, system health; speaks or notifies only when
  something matters.
- **Cron in natural language** (Hermes): "every Monday at 9 summarize last week's
  worklogs"; results delivered to voice, notification or phone channel.
- **Context compaction** (ZCode compact) near the context limit.
- **RAG over local docs** (AIChat, phase 4): man pages, `/usr/share/doc`, your notes.

## 10. Organizer and email

- **Calendar, tasks, reminders, notes**: "move my 3 pm to tomorrow", "remind me to call
  the accountant when I get home" (presence-aware), "what did I promise Luka last week?"
- **Daily briefing** (spoken in the morning): meetings, deadlines, mail that needs you,
  yesterday's unfinished work (from time tracking), system health.
- **Follow-ups**: "if Ana doesn't reply by Friday, remind me" (reply tracking, Inbox Zero).
- **Email in your words**: you say the gist; it drafts in your style learned from sent
  mail and your `marko-voice` skill, in Slovenian or English per recipient; reads it back
  or shows it; "shorter", "more formal", "send".
- **Inbox rules in plain English** (Inbox Zero): "newsletters -> archive and summarize
  weekly", "anything from the bank -> flag and tell me".
- Bulk unsubscribe, summaries of long threads, extraction of dates into the calendar.
- **Connectors**: Google (Gmail, Calendar) via OAuth; IMAP/SMTP + CalDAV/CardDAV; KDE
  Akonadi over D-Bus. Tokens in Secret Service.
- **Channels** (OpenClaw / Hermes gateway): talk to KAgent from your phone over Telegram
  or Signal; approvals can be answered there, except privileged ones.

## 11. GUI surfaces (Tauri + React; same UI in a browser)

- **Presence overlay**: section 8.3.
- **Chat window** (ChatGPT/Claude-like): sessions, markdown, code, attachments (files,
  screenshots), tool cards, approval cards, cancel, "second opinion" (council) button.
- **Terminal pane** (ishell, fewshell): live PTY output of commands it runs; "take over"
  to type yourself.
- **Control center**: providers and model roles, persona and voice, overlay theme,
  autonomy and permissions, tools and groups, skills and recipes, memory editor,
  connectors (mail, calendar, channels), webcam settings, audit log with undo, usage
  and cost.
- **Tray**: status, mic toggle, stop-all. **KRunner** plugin and **plasmoid**.

## 12. Coding conductor

- **Built-in light coding**: read/search/edit/apply_patch tools (ZCode) for configs,
  scripts, units and small fixes.
- **`coding_task` delegates real projects** to Claude Code, Codex, ZCode CLI or Gemini
  CLI, through **ACP** where available, headless CLI otherwise. Progress streams into
  chat and the overlay HUD; you get a spoken summary when it ends.
- Several tasks in the background, notification when each finishes.
- The delegated agent keeps its own permission rules; starting the task is a KAgent
  approval.
- After a task, other models can **review the diff** (council, section 13).
- KAgent adds system context: a build failing on a missing package or full disk is
  fixed on the system side.
- Commits, pushes, merges and releases are never automatic: KAgent hands you commands.
- Default worker: ask each time (configurable per project).

## 13. Council

Members: any configured model (z.ai GLM, Anthropic, OpenAI, Gemini, local Ollama) and
agent CLIs (Claude Code, Codex, ZCode, read-only in a council), or A2A agents.

Default protocol (Karpathy's `llm-council` stages, extended):
1. **Brief**: the question plus context (files, logs, system state).
2. **Independent answers**: members answer without seeing each other.
3. **Anonymous peer review**: answers labelled A/B/C; each member ranks and critiques.
4. **Weighing**: votes weighted by confidence, role and (optionally) track record.
5. **Synthesis**: the chair (rotating by default) writes a decision record: options,
   positions, vote, confidence and **dissent kept, not averaged away**.
6. **You decide**; the council never acts.

Options: roles (skeptic, security reviewer, pragmatist, cost watcher), protocols (quick
vote, debate rounds, devil's advocate), budget limit (tokens/money/time), member picker.
Uses: "second opinion" on high-risk approval cards, diff review after coding tasks,
architecture decisions, unclear time allocation, by voice ("Council, should I upgrade
the kernel now?" -> "3 to 1 wait, Codex disagrees because...").
Decision log is searchable.

## 14. Time tracking

- **Signals, all local**: active window and project (KWin), git commits and branches,
  Claude Code / ZCode / KAgent session logs, calendar, idle time, webcam presence.
- **Timeline** per day, mapped to Jira tickets with the logic of your `logging-hours`
  skill; unclear blocks go to you or the council.
- **Output**: worklog drafts plus the exact commands. **You run them; KAgent never writes
  to Jira itself** (default; changing it is an explicit setting).
- Weekly summaries, focus time, break reminders.

## 15. Webcam presence (off by default, local only, visible indicator)

- **Presence**: at desk or away; holds non-urgent speech while away; "welcome back,
  while you were away..."; feeds time tracking.
- **Look-to-talk**: look at the orb and speak, no wake word.
- **Gaze context**: "what's this?" means the window/region you look at. Limit: webcam
  gaze is accurate to a few centimetres, so window or region, not word.
- **Gestures**: open palm = stop talking / kill switch; thumbs-up = confirm "spoken yes"
  level actions.
- **Wellbeing**: break reminders after long focus.
- **Tech**: MediaPipe Face Landmarker (iris landmarks) via ONNX Runtime in the daemon,
  a few frames per second; a small gaze-direction model on top. Research found no
  existing open "look to talk" desktop tool, so this part is new work.

## 16. Phone calls (call agent)

Goal: your Telemach line rings on the IP phone **and** on the PC. You can answer on the
desk phone, answer on the PC (headset), or let KAgent answer, screen the call and hand
it to you. Your work is interrupted only when the call deserves it.

### 16.1 Getting the line into KAgent (the deciding question)

Telemach's line is SIP under the hood, but community reports say Telemach historically
did **not** give subscribers SIP credentials, and blocked inbound UDP 5060 (old reports,
2010 and 2015, may be outdated). So there are three routes, in order of preference:

| Route | How | Trade-off |
|---|---|---|
| A. SIP credentials from Telemach | Ask support for SIP username/password/registrar for your number (business "IP telefonija" packages are the likeliest to have them). A local PBX registers the line and rings both the IP phone and KAgent | Cleanest; depends on Telemach |
| B. Gateway at the modem | If the line only comes out of the Telemach modem's phone port: an FXO gateway (e.g. Grandstream HT813) turns that port into SIP for the local PBX | Works without credentials; one small box, narrowband audio |
| C. Forwarding | Telemach forwards unanswered/busy calls to a cheap SIP number from a VoIP provider that the PBX registers | No hardware; KAgent only gets calls you didn't pick up; extra cost per minute |

Only official routes: no extracting credentials from provisioned devices.

### 16.2 Architecture

```
 Telemach line ──(A: SIP / B: FXO gateway / C: forwarded SIP number)──▶ Asterisk (local PBX, Docker)
                                                                         │ rings in parallel
                               ┌─────────────────────────────────────────┼──────────────┐
                               ▼                                         ▼              ▼
                         IP desk phone                     KAgent softphone (PC)   KAgent call agent
                                                           (you answer on PC)     (AudioSocket/ARI:
                                                                                   call audio <-> live model)
```

- **Asterisk as the call router** (proven: AVA, the Asterisk AudioSocket voicebots).
  It handles SIP registration, NAT, codecs and parallel ringing. KAgent talks to it
  through **ARI** (call control: answer, transfer, hold, hang up) and **AudioSocket**
  (raw call audio to and from KAgent).
- Pure-Rust SIP (xphone, rsiprtp, wavekat-sip) is still beta/pre-1.0; reconsider later
  for a PBX-free setup.
- **Audio**: phone audio is 8 kHz G.711 (G.722 16 kHz if the phone and line support it);
  resample to 16 kHz for the live model and its 24 kHz output back to 8 kHz. Same
  live-model path as section 8, so tools, memory and approvals work during calls.

### 16.3 Incoming call flow

1. **Ring**: the overlay pops up with caller ID resolved from contacts and memory
   ("Ana, your manager, last call Tuesday about the release"), plus a spam/unknown flag.
2. **Rules decide** what happens: VIP list always rings through; unknown numbers are
   screened; focus time, meetings or "do not disturb" send calls to the agent; working
   hours; webcam presence (away -> agent answers).
3. **Your options** (click, hotkey or voice: "take it", "let it answer", "decline"):
   answer on desk phone, answer on PC, **let KAgent answer**, take a message, offer a
   callback slot from your calendar, decline.
4. **Screening**: KAgent answers ("Hello, this is Marko's AI assistant. What is it
   about?"), then whispers a one-line summary to you; you **take over** live or let it
   finish (message, booking, "he'll call you back at 3").
5. **Interrupting your work**: pauses media (MPRIS), mutes KAgent's own speech, marks
   you busy; after the call restores everything.
6. **After the call**: summary, action items into tasks, follow-up email draft in your
   style, callback reminder, and an entry in the time-tracking timeline (mapped to a
   ticket if it was work).

### 16.4 More
- **Live assist during your own calls** (optional): live transcript and quiet
  suggestions in the overlay ("she asked for the invoice number: 2026-114").
- **Voicemail** transcribed and summarized; urgent ones announced.
- **Outbound calls by the agent** ("call the dentist and book next week"): outward
  action under your name, always a spoken "yes", late phase.

### 16.5 Doorbell and video intercom (Hikvision, home SIP)

The installed system is a **Hikvision** door station + indoor monitor. Hikvision exposes
four interfaces, each with model- and firmware-specific caveats (sources: Home Assistant
community threads, pergolafabio's Hikvision add-ons, Yeastar interop guides):

| Interface | What it gives | Caveats found |
|---|---|---|
| **SIP** | The actual call (audio, video). Indoor stations like DS-KH6320 can register to a PBX (Network > SIP Server); Yeastar tested registration and audio both ways | Outbound video failed in Yeastar's test; one community "Asterisk as indoor station" method works only with DS-KH9310/9510; registration errors (401/403, 488) reported. If the PBX is down and the device depends on it, calls are missed |
| **ISAPI** (HTTP + digest auth) | Door relay: `PUT /ISAPI/AccessControl/RemoteControl/door/1` (open); call signal (answer / hang up / reject); call status; alert stream; snapshots | Event reliability varies: call status and button events stay "idle" on some firmware; some calls return "Invalid Operation"; some models (e.g. DS-KV8102-IM) have no ISAPI |
| **Hikvision SDK (HCNetSDK)** | Events (call, motion, door opened) and commands; what pergolafabio's Home Assistant doorbell app uses because it is more reliable than ISAPI events | Native C library (Linux x64 available) -> FFI or a small sidecar |
| **RTSP** | Video: `/Streaming/Channels/101` (main, e.g. 1080p25) and `/102` (sub) | No snapshots over RTSP; use the ISAPI picture endpoint |

Firmware updates have changed or removed local APIs for some users, so the plan is:
note the exact model + firmware, test each interface, then **pin firmware** (no auto
updates) once it works. Answering a doorbell call without Hik-Connect requires the SIP
route; Hik-Connect (cloud app) is not used by KAgent.

KAgent integration:
- **Events and door relay** through the SDK sidecar (fallback ISAPI), one adapter
  `hikvision` with a stable internal interface (ring, motion, door opened, open relay,
  answer, reject), so another brand is just another adapter.
- **Call audio** over SIP through the local Asterisk (indoor monitor and/or door station
  registered as extensions), into the same call agent as 16.3.
- **Video in the overlay**: RTSP converted to WebRTC for the Tauri/browser view (e.g. a
  go2rtc-style bridge, which also supports two-way audio on some Hikvision models).
- **Monitor stays primary**: the indoor monitor keeps ringing and opening the door as
  today; KAgent is an extra listener, never the only path.

- **Same Asterisk**: the door station registers to (or calls) the local Asterisk like any
  phone; a ring reaches the indoor monitor, the IP phone and KAgent in parallel. The
  existing monitor keeps working unchanged.
- **Ring on the PC**: the overlay shows the live camera (RTSP) and a vision model's one
  line description ("courier with a parcel", "two people, no uniform"). The doorbell
  ring also pauses media and interrupts like a call (16.3).
- **Your options**: talk through the PC (mic/speaker), let KAgent answer ("please leave
  the parcel by the door, thank you"), send a reply to your phone channel when you are
  away (snapshot + "someone at the door"), ignore.
- **Log**: every ring with snapshot and description, searchable ("did the courier come
  today?").
- **Door relay (open the door)** is a physical-security action:
  - never automatic, never by voice alone, never because a visitor claims to be someone
    (social engineering);
  - click on the approval card, or from the phone channel with a PIN, privileged level;
  - every opening in the audit log with the snapshot.
- **Privacy**: no face recognition by default; the camera may cover the street or
  neighbours, so snapshots stay local with short retention.
- First step on review: identify the brand/model, then check its SIP (direct call vs
  registration), RTSP URL and relay API.

### 16.6 Legal and safety (built in, not optional)
- **EU AI Act Article 50** (in force since 2 August 2026): when KAgent speaks on a call it
  must say it is an AI and on whose behalf it calls, at the start and again in longer
  calls. The greeting includes this and cannot be removed.
- **Recording and transcription** are personal-data processing (GDPR): off by default;
  when on, the caller is told and the reason is stated. Slovenian sources disagree on
  one-party vs all-party consent for recording, so KAgent always announces it.
  Confirm with a lawyer before recording business calls.
- Call audio stays local except what the live model needs; transcripts follow the
  memory retention settings.

## 17. Research: what already exists and what KAgent takes

No existing project combines all of this. The closest are OpenClaw and Hermes Agent
(personal assistants with memory, skills, cron and chat channels, but no system-level
safety model, no sci-fi voice presence and no council), SysKnife (safe Linux admin, no
assistant) and Personal Jarvis / JARVIS-OS (voice orb assistants, little safety or
organizer depth).

| Project | Take |
|---|---|
| [OpenClaw](https://cussid-huaz.github.io/agent-brain/entities/ai/openclaw/) | Local gateway as single control plane; chat channels (Telegram, Signal...); SKILL.md skills + registry; heartbeat (30 min, own session) and cron; memory exposed as tools; `doctor` check for risky settings |
| [Hermes Agent](https://hermes-agent.nousresearch.com/docs) | MEMORY.md + USER.md; session search; self-written skills + curator; skill quarantine; natural-language cron; pluggable memory providers |
| [Personal Jarvis](https://github.com/PersonalJarvis/PersonalJarvis), [JARVIS-OS-V.2](https://github.com/MAL19INDUSTRIES/JARVIS-OS-V.2) | Orb overlay + desktop mode + headless server mode; Gemini Live desktop assistant with screen/file/browser tools; one spoken request fanned out to agents |
| [SysKnife](https://mcpservers.org/servers/lacs-project/sysknife) | Typed privileged actions; hash-chained audit with verify; daemon executes, model proposes; rollback |
| [LLMAgent](https://github.com/imsmith/llmagent), [Bashir](https://github.com/newsbubbles/bashir) | D-Bus, udev, inotify tools; explicit flag for superuser |
| [ShellGPT](https://docsearch.algolia.com/mcp/docs/repo/ther1d/shell_gpt), [ishell](https://github.com/smart-linux-shell/ishell), [fewshell](https://github.com/few-sh/fewshell) | Roles; terminal pane with take-over |
| [OpenAI Codex](https://developers.openai.com/codex/concepts/sandboxing) | Landlock + seccomp sandbox; sandbox and approval separate; once/session/always |
| [Goose](https://block.github.io/goose/docs/guides/tool-permissions) | MCP extensions; per-tool overrides; recipes; `/mode`; under 25 active tools |
| [AIChat](https://github.com/sigoden/aichat) | Agents = instructions + tools + documents; RAG; OpenAI-compatible serve |
| [Open Interpreter](https://docs.openinterpreter.com/safety/safe-mode) | Script scanning before run; immediate stop |
| [llm-council](https://www.analyticsvidhya.com/blog/2025/12/llm-council-by-andrej-karpathy/) | Independent answers -> anonymous peer ranking -> chairman synthesis |
| [Inbox Zero](https://github.com/elie222/inbox-zero) | Plain-English inbox rules; drafts in your tone; reply tracking; bulk unsubscribe; inbox via chat apps |
| [Letta / Mem0 / Graphiti](https://dev.to/agdex_ai/letta-vs-zepgraphiti-vs-mem0-choosing-an-ai-agent-memory-architecture-155a) | Simple memory layer (Mem0 style) with temporal facts (Graphiti idea); not agent-managed tiers (Letta) because of token cost |
| [Home Assistant Assist / Wyoming](https://github.com/rhasspy/wyoming/) | Wyoming satellites; local wake word -> STT -> LLM -> TTS pipeline |
| [Gemini Live docs](https://ai.google.dev/gemini-api/docs/live-api/session-management) | Session resumption, sliding-window compression, goAway handling |
| [voice-call](https://github.com/daure/voice-call/releases), [speech-to-speech](https://github.com/troed/speech-to-speech), [Jarvis](https://docsearch.algolia.com/mcp/docs/repo/isair/jarvis), [voicebot](https://github.com/madcato/voicebot) | OpenAI Realtime; local pipeline; MCP in voice agents |
| [rustpotter](https://github.com/GiviMAD/rustpotter), [oww-rs](https://docs.rs/crate/oww-rs) | Offline wake word in Rust |
| [MediaPipe Iris](https://research.google/blog/mediapipe-iris-real-time-iris-tracking-depth-estimation/), [GazeTracking](https://github.com/antoinelame/GazeTracking), [WebGazer](https://webgazer.cs.brown.edu/) | Local iris landmarks and gaze direction; self-calibration from clicks |
| [Ingero](https://dev.to/ingero/mcp-as-observability-interface-connecting-ai-agents-to-kernel-tracepoints-4gaa), [ebpf-mcp](https://mcpservers.org/en/servers/sameehj/ebpf-mcp), [bpf-mcp](https://github.com/Stringy/bpf-mcp) | Kernel events in SQLite for queries; allowlisted probes only |
| [models.dev](https://raw.githubusercontent.com/anomalyco/models.dev/HEAD/README.md) | Model catalog |
| [ACP](https://zed.dev/acp), [A2A](https://www.linuxfoundation.org/press/a2a-protocol-surpasses-150-organizations-lands-in-major-cloud-platforms-and-sees-enterprise-production-use-in-first-year), [AG-UI](https://www.infoworld.com/article/4248550/getting-started-with-ag-ui-the-agent-user-interaction-protocol.html) | Standards in section 4 |
| Rig, agentix, rustic-ai | Not used as dependencies (ZCode provider model and full stream control needed); references only |
| [AVA for Asterisk](https://docsearch.algolia.com/mcp/docs/repo/hkjarral/ava-ai-voice-agent-for-asterisk), [Asterisk AudioSocket voicebots](https://community.asterisk.org/t/build-a-real-time-ai-voicebot-with-asterisk-audiosocket-stt-llm-tts-or-sts/109373), [self-hosted Asterisk voice AI](https://community.asterisk.org/t/fully-self-hosted-voice-ai-the-whole-stt-llm-tts-stack-on-your-own-nvidia-gpu-works-with-asterisk/113523) | Asterisk + ARI/AudioSocket as the bridge between a phone line and an AI voice; fully self-hosted possible |
| [LiveKit SIP](https://docs.livekit.io/agents/openai/telephony), [phone-assistant](https://github.com/livekit-examples/phone-assistant) | Call joins a room, agent joins as participant; warm transfer pattern (some parts cloud-only, so not used) |
| [xphone](https://docs.rs/crate/xphone/latest), [rsiprtp](https://docs.rs/crate/rsiprtp/0.4.1), [wavekat-sip](https://docs.rs/wavekat-sip) | Rust SIP/RTP stacks, all early; candidates for a later PBX-free mode |
| [Telemach VoIP threads](https://slo-tech.com/forum/t632471), [Telemach terms](https://telemach.si/download/terms/splosni-pogoji-poslovanja-za-storitve-telemach-telefonija/2422) | SIP credentials historically not given, port 5060 blocked (old reports): plan for routes A/B/C |
| [EU AI Act Art. 50 FAQ](https://digital-strategy.ec.europa.eu/en/faqs/transparency-obligations-under-article-50-ai-act), [CMS examples](https://cms.law/en/gbr/legal-updates/eu-ai-act-transparency-real-world-examples) | AI must disclose itself on calls; built into the greeting |
| [pergolafabio Hikvision add-ons](https://raw.githubusercontent.com/pergolafabio/Hikvision-Addons/HEAD/doorbell/README.md), [HA Hikvision intercom thread](https://community.home-assistant.io/t/ds-kd8003-ds-kv8113-ds-kv8213-ds-kv6113-ds-kv8413-and-integration-hikvision-hikconnect-video-intercom-doorbell/238535), [Yeastar DS-KH6320 guide](https://help.yeastar.com/en/p-series/integrations/hikvision/register-hikvision-ds-kh6320-indoor-station-to-yeastar-p-series-pbx-system.html) | Hikvision events via SDK, door relay via ISAPI, SIP registration of the indoor station to a PBX, firmware caveats |
| [Zeltser: honeytokens](https://zeltser.com/honeytokens-canarytokens-setup), [Canary FSWatcher](https://github.com/kloyan/canary-fswatcher/blob/main/README.md), [Tetragon canary tutorial](https://isovalent.com/blog/post/tetragon-canary-tokens-tutorial) | Decoy credential files, file-watch tripwires, eBPF file-access probes (with false-positive care) |
| [OpenCanary](https://www.admin-magazine.com/Articles/OpenCanary-attack-detection) | Low-interaction honeypot on a dedicated host |
| [RouterOS REST API](https://manual.mikrotik.com/docs/7.25/developer-guides/rest-api), [address lists](https://manual.mikrotik.com/docs/7.25/firewall-and-quality-of-service/firewall/address-lists), [psd discussion](https://forum.mikrotik.com/t/can-someone-please-explain-the-psd-attributes/98488), [CrowdSec RouterOS bouncer](https://forum.mikrotik.com/t/cs-routeros-bouncer-crowdsec-bans-enforced-on-routeros-through-the-api-address-lists-raw-rules-open-source-mit/273590) | REST over HTTPS with `rest-api`+`api` policies; dynamic address lists with timeouts; psd catches only automated scanners |
| ZCode (local) | Provider registry and rules, tool contracts, scheduler, permission broker, loop invariants, compaction |
| ReelGen (local) | Gemini Live audio plumbing, KITT persona, ZCode-style flat provider file |

Gaps: nothing found on OpenHands, Pipecat (incl. its SIP support), jambonz or Khoj; most sources are READMEs, docs and
third-party write-ups; stars, maintenance and licences not verified.

## 18. Phases (refocused)

Phase 1 - diagnose (read-only, already useful on day one)
- [ ] Daemon, provider registry (ZCode import), streaming adapters, agent loop,
      permission engine + broker, hash-chained audit, sessions.
- [ ] Read-only tools: system, processes, kernel (version, cmdline, taint, modules,
      kernel log), journal, systemd (+ `systemd-analyze`), network, disks + SMART,
      sensors, power/battery, audio (PipeWire), Bluetooth, GPU, coredumps.
- [ ] Diagnostic playbooks as skills: slow machine, no sound, Wi-Fi, slow boot,
      battery, disk full, crashed app.
- [ ] Chat GUI (React, browser first) with tool cards.

Phase 2 - change safely, control by voice
- [ ] Typed change actions: sysctl, kernel modules/params, services, packages, drivers,
      config files, power profile, governor, swap/zram.
- [ ] Change journal with undo, dry run, before/after measurement, optional snapper
      snapshot; `sudo -n` allowlist; approval cards.
- [ ] Desktop and hardware control over D-Bus: PipeWire, BlueZ, NetworkManager, UPower,
      power-profiles-daemon, KWin/Plasma.
- [ ] Voice: Gemini Live proxy in the daemon (tools, approvals, resumption), orb
      overlay, global hotkey. Tauri shell for the overlay.
- [ ] MCP server mode for Claude Code / ZCode / Codex.

Phase 3 - watch and warn
- [ ] Watchers: OOM, kernel warnings/taint, failed units, DKMS failures, SMART,
      thermal throttling, disk space, AppArmor denials, SSH failures, security updates.
- [ ] Notifications + spoken alerts with cause and proposed fix.
- [ ] Security posture report.
- [ ] Intrusion detection (7.4): honeytokens + fanotify, persistence watch,
      file-integrity baseline, login watch, agent heartbeat.
- [ ] MikroTik adapter: Traffic Flow collector, REST read (leases, ARP, connections,
      log, address lists), new-device and traffic-anomaly alerts; containment with
      approval later.
- [ ] Optional OpenCanary honeypot on a separate VLAN / Raspberry Pi.
- [ ] Read-only eBPF `trace` tool (allowlisted bpftrace probes: who does the I/O,
      run-queue latency, syscall hot spots, TCP retransmits).

Phase 4 - learn, remember, extend (planned 2026-11-24; stages S17-S33 in
IMPLEMENTATION.md, not started; specs under docs/specs/, one file per feature)
- [ ] Foundations: awaitable turns, allowances that stay in their session, merged
      settings patches, a shared notification path.
- [ ] Skills library: several roots, the agent writes skills, import from other tools
      and from git sources (disabled until reviewed). Takes the skills part of 9 off
      the parked list.
- [ ] Settings in one screen with a side menu (ZCode's layout).
- [ ] Context compaction, todo lists, plan mode.
- [ ] Memory (file memory, lexical recall, facts with a time) and session search:
      takes "memory" off the parked list.
- [ ] Voice brain: the chat model thinks, the live model listens and speaks (8.1).
- [ ] Attach to the running Brave through its DevTools remote debugging.
- [ ] Automations with real schedules (9), and a Telegram channel for text.
- [ ] Web fetch and search, subagents and background tasks, hooks, inline widgets.

Parked (designs kept in this document, not planned until the core is done)
- Organizer and email (section 10), phone calls and doorbell (section 16), council
  (13), time tracking (14), webcam presence (15), coding delegation beyond MCP (12),
  phone channels, Wyoming satellites, A2A,
  OpenAI-compatible endpoint, RAG, OpenAI Realtime, wake word, offline voice pipeline,
  KRunner/plasmoid, Fedora/Arch modules.

## 19. Defaults (change any of them on review)

| Item | Default |
|---|---|
| Platform | Linux core, Ubuntu + KDE first |
| GUI | Tauri + React (browser mode kept) |
| Root method | `sudo -n` with a sudoers allowlist (pkexec optional) |
| Voice path | daemon proxy |
| Provider config | own file, keys in Secret Service, ZCode templates imported |
| Permission mode | `ask` |
| Persona | calm, quietly witty (KITT-style), Slovenian/English |
| Overlay theme | orb, all four available |
| Screen awareness | phase 2, only on request |
| Wake word | phase 3, on; look-to-talk with webcam in phase 4 |
| Coding worker | ask each time |
| Council chair | rotating |
| Email sending | one spoken "yes" |
| Jira | drafts + commands, you submit |
| Webcam | off by default |
| Phone routing | Asterisk in Docker; route A if Telemach gives SIP credentials, else B |
| Unknown callers | screened by the agent; VIPs always ring through |
| Call recording | off; announced when on |
| Door relay | click or phone-channel PIN only; never by voice or automatically |

## 20. Open questions (core only)

1. Which machine problems bite you most often? They become the first playbooks.
2. Root method: `sudo -n` with an allowlist, or a `pkexec` prompt each time?
3. Root filesystem: ext4 or btrfs? (btrfs/snapper makes big changes undoable.)
4. GPU: NVIDIA, AMD or Intel? (drivers, DKMS, `nvidia-smi` tooling.)
5. MikroTik: model and RouterOS version? Is it the main router (sees all LAN traffic)?
   Any VLANs already?
6. Keep or delete the scaffolded Rust drafts in `crates/kagent-core/`?

Parked-feature questions (persona name, mail, phone, Hikvision models, council) wait
until those features are back on the table.
