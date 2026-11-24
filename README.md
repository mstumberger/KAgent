# KAgent

A local AI agent for your Linux machine. It diagnoses from real system data (kernel,
services, journal, hardware, network), changes things only after you approve them
(with undo), controls the desktop, apps, media, editor and a browser by text or voice,
and watches for problems and intruders.

Rust daemon (`kagentd`) + small root helper (`kagent-priv`) + React web UI, with KDE
and GNOME integration. Runs on systemd Linux: Ubuntu/Debian (apt), Fedora/RHEL (dnf),
Arch (pacman). KDE Plasma 6 is the best-supported desktop; GNOME works with a few
extras (see [platform support](#platform-support)).

## Contents

- [Quick start](#quick-start)
- [Using it](#using-it)
- [One-time setup](#one-time-setup-you-run-these-kagent-never-runs-sudo)
- [What it can do](#what-it-can-do)
- [Safety model](#safety-model)
- [Settings](#settings)
- [Platform support](#platform-support)
- [Files, ports, environment](#files-ports-environment)
- [Troubleshooting](#troubleshooting)
- [Development](#development)

## Quick start

Prerequisites: Rust (stable, via rustup), Node 22 + pnpm, systemd. No system
libraries are needed to build (TLS is rustls, D-Bus is pure Rust, SQLite is bundled;
the binary links only glibc). For a local model: [Ollama](https://ollama.com) with a
tool-calling model, e.g. `ollama pull qwen2.5:7b-instruct`.

```bash
# On this machine Rust is not on PATH:
export PATH=$HOME/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:$PATH
cargo build --release -p kagentd -p kagent-priv
(cd web && pnpm install && pnpm build)
./target/release/kagentd            # prints http://127.0.0.1:7878/#token=...
```

Open the printed URL (the token is in the link; it is also in
`~/.config/kagent/token`). Then in **Providers** add a model:

- **Ollama (local)**: template "Ollama (local)", Fetch models, enable one with tool
  support (e.g. `qwen2.5:7b-instruct`), "Use for chat". KAgent uses Ollama's native API
  with a 16k window: the OpenAI-compatible endpoint silently truncates prompts to the
  server default (4k) and the model starts looping.
- **Anthropic / OpenAI / Gemini / OpenRouter / Z.ai / ...**: pick the template, paste
  the key (stored in the desktop keyring: KWallet or GNOME Keyring), Fetch models.

Without the root helper everything read-only works; changes that need root show the
command to install it (`kagentd sudoers`).

## Using it

| Where | How |
|---|---|
| Web UI | `http://127.0.0.1:7878/#token=...` - tabs: chat, voice, alerts, changes, security, system, providers, tools, settings |
| Tray icon | KAgent lives in the tray and starts as a small widget (always on top, all desktops, draggable; `settings.startWidget`). Icon: grey idle, green pulsing listening, blue speaking, red mic muted, orange dot = alerts. Left click: show/hide the window; middle click: mute; scroll: voice volume; menu: Widget / Full window / Only in tray, listen, mute, volume, full control, alerts, stop all, quit |
| Widget and full window | one window, two layouts, voice keeps running: the widget's expand button opens chat, transcript and settings; the full window's "widget" / "tray" buttons go back |
| Voice | tray "Listen", the Voice tab, or the orb widget (Space: talk, M: mute, Esc: stop); keeps running when you switch tabs (header: listening / mute / end) |
| KDE | panel widget "KAgent Voice" (waveform + alert badge); KRunner: Alt+Space, `ka <question>`, `ka doctor`, `ka stop` |
| GNOME | Super+Alt+Space push-to-talk (after `scripts/desktop-integrate.sh`), app menu entry |
| Other agents | MCP over stdio: `claude mcp add kagent -- /path/to/kagentd mcp` |
| CLI | `kagentd [serve]`, `kagentd audit verify`, `kagentd sudoers`, `kagentd mcp`, `kagentd mikrotik-setup <PC-IP>`, `kagentd router-unblock <IP>` |

Things to ask:

- Diagnose: "my machine feels slow", "what changed since yesterday?", "is the NVIDIA
  driver OK after the kernel update?", "what is writing to my disk?", "who is using my
  microphone?"
- Change (with approval + undo): "lower swappiness to 10", "restart bluetooth",
  "install htop", "turn on dark mode"
- Apps and media: "pause", "next song", "play the newest video from Downloads in VLC",
  "open this file in Kate"
- Code: "open ReelGen in VS Code where the Gemini websocket is opened", "which file in
  KAgent defines the tool budget?" (finds without opening)
- Browser: "open github.com in your browser and search for kagent", "read this page"
- Security: "anything suspicious?", "who tried to log in over SSH?"
- Other apps and terminals: "tell the Claude Code session in KAgent: ...", "what is on my
  screen?", "export this conversation", "reboot in 10 minutes"
- Its own UI: "the voice page looks broken, what is wrong?" (reads its console errors
  and the page), "switch to the alerts tab", "use the KITT look"

**Kill switch**: the red "stop all" button in the header (shown while anything runs),
`ka stop` in KRunner, or `POST /api/stop-all`. It cancels every turn and running tool.

## One-time setup (you run these; KAgent never runs sudo)

| What | Command |
|---|---|
| Root actions (sysctl, modules, system services, packages, SMART, firewall, eBPF) | `./target/release/kagentd sudoers` prints ONE line: paste it into a terminal (asks your password once) |
| Typing into Konsole tabs (terminal tool) | Konsole -> Settings -> Configure Konsole -> General -> "Enable the security sensitive parts of the DBus API", restart Konsole |
| Voice | Providers: add Google Gemini with your key; Voice tab: pick the live model |
| App menu; KDE: KRunner plugin; GNOME: Super+Alt+Space push-to-talk | `scripts/desktop-integrate.sh` (KDE: bind a key to "KAgent Talk" in System Settings -> Shortcuts) |
| KDE panel widget (voice waveform, alert badge) | `kpackagetool6 -t Plasma/Applet -i plasmoid/org.kde.kagent.voice`, add "KAgent Voice" to the panel (update: `-u`) |
| Start with your session (systemd user service) | `scripts/install-service.sh` |
| "KAgent stopped" alarm | `scripts/install-heartbeat.sh` |
| GNOME on Wayland: window control and "active window" | install the [Window Calls](https://extensions.gnome.org/extension/4724/window-calls/) extension |
| Keyboard/mouse input tool (optional) | X11: `xdotool`; Wayland: `ydotool` + the ydotoold service |
| Window control on X11 outside KDE (optional) | `wmctrl` |
| MikroTik traffic monitoring | `./target/release/kagentd mikrotik-setup <this-PC-IP>`, paste into RouterOS, then Security -> Network |
| Agent browser | the first browser request opens KAgent's own Brave/Chrome window; sign in there to sites the agent should use |
| Desktop app (tray, overlay window, Super+Space) | see `app/README.md` (needs GTK/WebKit dev packages) |

## What it can do

56 typed tools in groups (Settings -> Tools the agent may use):

- **Diagnose** (system, kernel, services, hardware): processes, kernel (taint,
  modules, log), sysctl, systemd + boot analysis, journal, disks + SMART, network,
  sensors, power, audio (PipeWire), Bluetooth, GPU/drivers/DKMS, crashes,
  `what_changed` timeline (packages, boots, /etc, KAgent changes), `doctor` health
  score 0-100 (pressure stall info, failed units, errors, heat, swap, reboot needed,
  security updates), eBPF traces (who does the I/O, run-queue latency, syscall hot
  spots, TCP retransmits, exec/open snoop).
- **Change safely** (change): packages (apt/dnf/pacman), sysctl, kernel modules,
  systemd units, power profile, files; every change shows a dry run, is journaled and
  undoable from the Changes tab.
- **Desktop and apps** (desktop): audio devices and volume, Bluetooth, Wi-Fi/VPN,
  brightness, dark/light mode, windows, notifications; open URLs/files/apps, also in a
  chosen app; media players over MPRIS (play, pause, next, seek, volume; paused while
  you talk); clipboard; "this file" (`focus_context`: active window and its open files,
  Dolphin copy, recent files, downloads); keyboard/mouse input (asks first).
- **Code**: `projects` (your project folders, code search), `open_in_editor` (VS Code,
  Kate, IntelliJ/PyCharm/WebStorm/PhpStorm/CLion, Zed, GNOME Text Editor, gedit) at a
  file and line.
- **Browser**: KAgent's own Brave/Chrome window over DevTools: open, search, read the
  page, click, type, keys, screenshot.
- **Voice**: Gemini Live through the daemon (the key never reaches the browser),
  barge-in, session resumption, tool calls with the same approvals (answerable by
  voice); live visualizer looks: orb (glass sphere with your waveform inside and a
  spectrum corona), spectrum, wave, KITT, HAL; ice = KAgent speaking, green = you.
- **Watch and warn**: OOM kills, kernel warnings, crashes, failed services, disk
  space, heat, NVIDIA/DKMS after kernel updates, SSH brute force, security updates;
  grouped, rate-limited, quiet hours.
- **Intrusion detection** (security): honeytoken decoys (any read = high alert),
  persistence watch (SSH keys, units, autostart, cron, SUID, ld.so.preload, sudo/PAM,
  shell rc, foreign kernel modules), heartbeat; with a MikroTik: new LAN devices, port
  scans from inside, traffic spikes, night-time new destinations, router login traps,
  IP blocking.
- **Skills**: diagnostic playbooks in `skills/` (and `~/.config/kagent/skills/`) the
  agent loads on demand.

## Safety model

- **Permission modes** (Tools tab, tray "Full control"): `read-only` (no changes at
  all), `ask` (default: every change shows an approval card), `trusted` (no cards except
  root actions and "always ask" tools such as typing into terminals, messages, reboot),
  `full` (full control: no cards at all except root actions). Per-tool overrides
  (allow/ask/deny) and "allow for this session" on the card.
- **Voice approvals**: in a voice session answer a card by saying "yes" / "go ahead" /
  "ja" or "no" / "wait" / "ne"; root actions need "approve" / "potrdi". Only your own
  speech transcript counts, never the model's words.
- **Harmless actions** (opening a page/folder/app, playback control, opening the
  editor) need no card but are still blocked in read-only mode. Acting inside apps
  (synthetic input, browser clicks/typing) asks; browser `eval` and root actions ask
  every time, even in trusted mode.
- **Root** only through `kagent-priv`: a root-owned helper started with `sudo -n` that
  accepts a fixed set of typed, validated operations, computes their undo and refuses
  everything else (no root shell unless you create `/etc/kagent/allow-root-shell`).
  Spec: `docs/specs/priv-ops.md`.
- **Undo**: every change goes to the change journal (`Changes` tab, `list_changes`,
  `undo_change`).
- **Audit**: hash-chained log of every tool call, approval and change;
  `kagentd audit verify` checks the chain.
- **Network**: the API listens on 127.0.0.1 only, needs the bearer token and checks
  the Host header. Tool output, files and web pages are treated as data, never as
  instructions.
- **Secrets**: provider keys in the desktop keyring (Secret Service); the agent browser
  uses its own profile, never your main one.

## Settings

Settings tab, or `~/.config/kagent/settings.json` (full format: `docs/specs/config.md`): fallback chat model (used when the chat model is busy), vision model,
chat/fast/live model, voice and orb theme, permission mode, rules and per-tool
overrides, tool groups (for chat and for MCP), persona (name, how to address you,
language: auto/sl/en), preferred editor and project folders, agent browser port,
pause media while talking, watcher quiet hours, sentinel and MikroTik options.

## Platform support

| | KDE Plasma 6 | GNOME | other desktops |
|---|---|---|---|
| diagnostics, changes, watchers, security, voice, browser, media, editor | yes | yes | yes |
| dark/light mode | Plasma color schemes | gsettings (+ GTK3 theme) | no |
| windows | KWin (X11 + Wayland) | Wayland: Window Calls extension; X11: wmctrl | X11: wmctrl |
| clipboard | Klipper | wl-clipboard / xclip | wl-clipboard / xclip |
| launcher | KRunner, panel widget | push-to-talk shortcut | menu entries |

| | apt (Ubuntu, Debian, Mint) | dnf (Fedora, RHEL, Rocky) | pacman (Arch, Manjaro) |
|---|---|---|---|
| install/remove with dry run + undo | yes | yes | yes |
| security updates | yes | yes | no advisories on Arch (update count instead) |
| package history | dpkg log | rpm | pacman log |
| firewall status | ufw | firewalld | nftables |

Distros without systemd are not supported. Details: `docs/specs/platform.md`.

## Files, ports, environment

| | |
|---|---|
| config | `~/.config/kagent/` (token, settings.json, providers.json, skills/) |
| data | `~/.local/share/kagent/` (sessions, audit.jsonl, changes, alerts, logs, sentinel, net.sqlite, browser-profile, screenshots) |
| ports | 7878 web UI + API (127.0.0.1), 9333 agent browser DevTools (127.0.0.1), UDP 2055 NetFlow from the router (only when MikroTik is configured) |

| Variable | Effect |
|---|---|
| `KAGENT_HOME` | put config and data under one folder (tests, second instance); such an instance never uses the desktop keyring (`KAGENT_KEYRING=1` to opt in) |
| `KAGENT_NO_NOTIFY` | no desktop notifications for alerts |
| `KAGENT_PRIV` | path of the root helper (default `/usr/local/libexec/kagent-priv`) |
| `KAGENT_DESKTOP` | force `kde`, `gnome` or `other` |
| `KAGENT_PM` | force `apt`, `dnf` or `pacman` |
| `KAGENT_GEMINI_WS` | Gemini Live WebSocket URL (tests use a fake server) |
| `RUST_LOG` | log level, e.g. `kagentd=debug` (logs: `<data>/logs/kagentd.log`) |

## Troubleshooting

| Symptom | Fix |
|---|---|
| Page says "No token" | open the URL kagentd printed, or `#token=$(cat ~/.config/kagent/token)` |
| Local model loops or ignores tools | use the "Ollama (local)" template (native API, 16k window) and a tool-calling model; small models handle fewer tools, so KAgent advertises the groups your request mentions first |
| "root helper missing" / "unsafe" | run `kagentd sudoers` and the printed lines (the helper must be root-owned and not writable by others) |
| Window or input commands fail on Wayland | GNOME: Window Calls extension; input: ydotool + ydotoold; KDE: activate/close by name works, listing needs X11 |
| Browser tool says the port did not open | another Chrome/Brave instance is using `browser-profile`, or the port (Settings -> Desktop and apps) is taken |
| Voice does not connect | add Google Gemini in Providers and pick the live model in the Voice tab; check `<data>/logs/kagentd.log` |
| Port 7878 in use | another kagentd is running: `pkill -x kagentd` |
| UI blank or frozen | ask KAgent "what is wrong with the UI?" (it reads the page's console via the `ui` tool), or tray -> Open KAgent; crashes show a card with "Try again" |
| No tray icon on GNOME | install the AppIndicator extension |

## Development

`crates/kagent-core` (agent loop, permissions, tools, platform backends),
`crates/kagent-providers` (provider registry from ZCode's catalog, streaming adapters),
`crates/kagent-priv` (root helper), `crates/kagentd` (daemon: API, voice, watchers,
sentinel, network, browser, MCP, KRunner), `web/` (React UI), `plasmoid/` (KDE widget),
`app/` (Tauri shell, separate workspace), `skills/` (playbooks), `scripts/` (setup and
test scripts), `docs/`.

```bash
cargo test --workspace                      # ~160 unit/integration tests, no root/network
KAGENT_IT=1 cargo test -p kagent-providers --test ollama   # live Ollama round trip
bash scripts/smoke-api.sh                   # daemon + Ollama end to end
python3 scripts/test-live.py                # voice bridge against a fake Gemini server
(cd web && pnpm lint && pnpm build)
```

Docs: `AGENTS.md` (rules for coding agents), `docs/IMPLEMENTATION.md` (stages,
decisions log, owner actions), `docs/specs/` (tools, API, config, root helper ops,
alerts, platform, scenarios), `docs/PLAN.md` (product background).
