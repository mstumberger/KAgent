# Plan: KAgent as a native Plasma app

Status: plan only (2026-11-24). Today the widget and the full window are a Chromium
`--app` window (`kagent-app`) driven by the daemon (frameless via KWin, dragged with
xdotool, mic permission pre-granted in its own profile). It works, but it is a browser
pretending to be an app: ~150 MB of Chromium, X11-only tricks for drag and focus, and
voice that needs a page to be open.

Goal: the widget, tray and main window become native Plasma 6 / Qt 6 components. The
Rust daemon stays the brain and its HTTP + WebSocket API stays the contract, so the
web UI keeps working (GNOME, other desktops, remote) and nothing in the agent changes.

## What changes and what stays

| Part | Today | Native target |
|---|---|---|
| Agent, tools, approvals, voice bridge, watchers | Rust daemon | unchanged |
| Voice audio (mic, playback, echo cancel) | browser (getUserMedia, WebAudio) | **daemon, PipeWire** (stage 0) |
| Tray icon + menu | ksni (StatusNotifierItem) | plasmoid in the system tray (KDE); ksni stays for GNOME |
| Small widget (orb, controls) | web page in Chromium | **plasmoid** (panel popup + desktop/floating) |
| Full window (chat, sessions, settings) | web page in Chromium | **Kirigami app** `kagent` |
| Approvals | cards in the page | cards in app/plasmoid + **notification actions** (Allow / Deny) |
| Push-to-talk, window focus, typing | xdotool, KWin scripts | KGlobalAccel shortcut, KWin scripts stay for other apps |
| Design tokens (night, glass, ice, signal) | CSS variables | Kirigami theme + QML constants |

## Stages

### Stage 0: voice in the daemon (prerequisite, ~3-4 days)
Without this, any native UI still needs a browser for the microphone.
- Capture and playback with PipeWire from Rust (`pipewire` crate, or `pw-cat` child
  processes first): 16 kHz mono in, 24 kHz out, into the existing Gemini Live bridge.
- Echo cancellation: route through PipeWire's `libpipewire-module-echo-cancel` (WebRTC
  AEC). Ship a user config snippet `~/.config/pipewire/pipewire.conf.d/kagent-aec.conf`
  (no root) and pick the `echo-cancel-source` / `-sink` nodes. Without AEC KAgent hears
  itself and interrupts itself.
- Voice state, levels and transcripts already go out on the event stream
  (`/api/voice/levels`, `LiveStatus`, transcripts): the UIs only display.
- New API: `POST /api/voice/{start,stop,mute}`, `volume`; the tray and plasmoid call
  these instead of telling a web page to start.
- Result: "Listen" works from the tray with no window at all; no browser permission.

### Stage 1: the widget as a plasmoid (~4-5 days)
Extend `plasmoid/org.kde.kagent.voice` (it already draws the waveform from the levels
API) into the full widget:
- Compact representation: tray/panel icon whose colour shows the state (idle, listening,
  speaking, muted, alerts), as the ksni icon does today.
- Full representation (popup, or on the desktop / floating): the orb, status, last
  exchange, talk / mute / volume / look, permission picker, expand-to-app button.
- Orb: QtQuick `ShaderEffect` (GLSL) for the glass sphere, corona and spectrum: the
  current canvas code ports almost 1:1 and runs on the GPU. Waveform from the levels
  endpoint (bands already computed in the daemon).
- Data: a small QML `WebSocket` client for events + `XMLHttpRequest` for commands
  (token from `~/.config/kagent/token`, as the current plasmoid does).
- Always-on-top floating: Plasma 6 desktop widgets can't float above windows; use a
  plasmoid popup pinned open, or a small Kirigami `ApplicationWindow` with
  `Qt.WindowStaysOnTopHint | Qt.FramelessWindowHint` (real native drag via
  `startSystemMove()`, works on Wayland too).

### Stage 2: the main window as a Kirigami app (~2-3 weeks)
- `kagent` app: Kirigami `ApplicationWindow` with pages Chat, Voice, Alerts, Changes,
  Security, System, Providers, Tools, Settings (same as the web tabs).
- Language: **Rust with cxx-qt** for the models (sessions, messages, approvals, alerts
  as `QAbstractListModel`s over the daemon API) and QML for the views. Keeps one
  language in the repo; KDE's own stack (C++ + QML) is the fallback if cxx-qt blocks.
- Markdown in replies: `TextEdit { textFormat: TextEdit.MarkdownText }`; tool chips and
  approval cards as QML components; theme from Kirigami with KAgent accents.
- Frameless + own title bar with `startSystemMove()` / `startSystemResize()`: native
  drag and resize, no xdotool, works on X11 and Wayland.
- Packaging: CMake + `kde_package_app_templates` or `cargo` + `cxx-qt-build`; `.desktop`
  file, AppStream metadata; later Flatpak (KDE runtime) with the daemon on the host.

### Stage 3: deeper Plasma integration (~1 week)
- KNotifications with actions: approvals arrive as notifications with Allow / Deny
  (works when no KAgent window is open); alerts with Investigate / Acknowledge.
- KGlobalAccel: real global push-to-talk shortcut (hold to talk), registered by the app.
- KRunner (already native D-Bus runner) gains "open in KAgent".
- Plasma Activities: sessions tagged per activity.
- Wayland: no xdotool, no xprop. Window control for other apps stays in KWin scripts;
  synthetic input needs `ydotool` or the XDG RemoteDesktop portal (ask once).

### Stage 4: retire the Chromium windows (~2 days)
- Tray "Open KAgent" starts the Kirigami app; the widget is the plasmoid.
- Keep the web UI served by the daemon for GNOME, other desktops, remote access and
  as a fallback (`kagent --web`).
- Remove `appwin.rs` frameless/drag code paths that only existed for Chromium.

## Order and effort

| Stage | Effort | User-visible result |
|---|---|---|
| 0 voice in daemon | 3-4 days | listen from tray, no mic prompt, no window needed |
| 1 plasmoid widget | 4-5 days | native widget, GPU orb, Wayland-ready |
| 2 Kirigami app | 2-3 weeks | native main window, native drag/resize |
| 3 integration | ~1 week | approve from notifications, hold-to-talk |
| 4 retire Chromium | 2 days | ~150 MB less, one fewer moving part |

Stages 0 and 1 give most of the value; 2 can grow page by page (start with Chat and
Voice, embed the web view for Settings until ported, via `QtWebEngine` if needed).

## Risks

- **Echo cancellation quality** decides whether barge-in works without headphones; test
  the PipeWire AEC module on this machine first (one afternoon) before stage 0.
- **cxx-qt maturity** for list models and async: prototype one model (sessions) first.
- **Plasma API changes** between 6.x releases: keep the plasmoid thin (QML + API).
- **Wayland**: everything native is fine; automation of *other* apps (typing into
  Signal, focusing windows) needs portals or ydotool and stays the hard part.
- **Two UIs to maintain** (web + native): keep the daemon API as the single source of
  truth; no logic in either UI.

## First step when we start

One-afternoon spike: PipeWire echo-cancel module + `pw-cat` capture piped into the
existing Gemini bridge, talking with speakers on. If barge-in works, do stage 0.
