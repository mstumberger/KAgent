# Spec: browser backends: own window, or the running Brave (S27)

Today the `browser` tool only drives a browser KAgent launches itself, with its own
profile and a DevTools port (tools.md section 14). The owner's real browser, with
their signed-in sessions, is out of reach. Chromium 136 and later ignore the DevTools
port on the default profile, so attaching needs the browser's own consent flow.

## 1. How attach works (verified on this machine, 2026-11-24)

- In Brave, `chrome://inspect/#remote-debugging` has the checkbox "Allow remote
  debugging for this browser instance". The page warns that it gives external apps full
  control of the browser, including saved data and cookies. Brave 156.1.98.63 has it.
  No extension is involved.
- When it is on and the browser runs, the browser writes `DevToolsActivePort` into its
  user data directory: line 1 is the port, line 2 is the browser WebSocket path. This
  is what `chrome-devtools-mcp` reads for its auto-connect (its `BrowserManager`
  connects to `ws://127.0.0.1:<port><path>`).
- Every new WebSocket connection makes the browser ask "Allow". Nobody at the screen
  means the connection waits.
- The setting survives browser restarts.

## 2. Settings (config.md section 11)

- `browser.backend`: `own` (default, unchanged) or `attach`.
- `browser.attachProfile`: the browser's user data directory. Empty = the first one
  that exists: `~/.config/BraveSoftware/Brave-Browser`,
  `~/.config/google-chrome`, `~/.config/microsoft-edge`, `~/.config/chromium`.
- `browser.attachTrust`: `strict` (default) or `normal` (section 4).
- The backend is read when the daemon starts, like `port` and `binary` today.

## 3. Connection (`kagentd/src/browser/attach.rs`, `browser/cdp.rs`)

- `parse_devtools_active_port(text) -> Option<(u16, String)>`: port 1..65535, a path
  that starts with `/devtools/browser/`.
- `BrowserSession`: one browser-level WebSocket, multiplexed with `sessionId`
  (`Target.attachToTarget {flatten: true}`):
  - `pages() -> Vec<TargetInfo>` (`Target.getTargets`, type `page`, not
    `chrome-extension://`).
  - `attach(target_id) -> AttachedTab` with `call(method, params)` and `detach()`.
  - `create_target(url)`, `close_target(target_id)`, `activate(target_id)`.
  - A ping every 10 s; a dropped socket is reopened on the next use.
- The daemon connects lazily, on the first tool call, with a 20 s deadline (the Allow
  prompt). After 10 idle minutes the socket is closed, so the browser's "controlled"
  indicator goes away; the next call reconnects and the browser asks again.
- `Browser::run` gets its tab through one internal trait (`TabTarget`), so the actions
  (read, click, type, key, scroll, screenshot, console, eval, search) are the same code
  for both backends.

## 4. Tab rules in attach mode

Agent tabs are the tabs the agent opened in this daemon run (`Target.createTarget`),
tracked by target id.

| Action | Agent tab | Owner's tab, `strict` | Owner's tab, `normal` |
|---|---|---|---|
| read, screenshot, console | read-only | read-only | read-only |
| open (new tab), activate, back | no approval | no approval | no approval |
| navigate, scroll | no approval | asks, always (never rememberable) | no approval |
| click, type, key | approval, medium, rememberable | asks always, high | approval, medium, rememberable |
| close | no approval | refused: "only tabs the agent opened can be closed" | refused the same way |
| eval | always asks, high | always asks, high | always asks, high |
| launch | not available: "this is the owner's running browser" | same | same |

- Target without `tab`: the agent's most recent agent tab, else the tab whose
  `document.visibilityState` is `visible` (one `Runtime.evaluate` per candidate, at
  most 12), else the first page.
- Every output starts with `Browser: <product name> (the owner's own window)`.

## 5. Tool description

- In attach mode the `browser` description says: these are the owner's real tabs with
  their signed-in sessions; act in a tab only when the request is about it; only tabs
  you opened can be closed; acting in the owner's tabs asks every time.
- No new input field; tab targeting is unchanged.

## 6. Status and errors (`GET /api/browser/status`, `attach.debugging`)

| Situation | `debugging` | Detail |
|---|---|---|
| profile directory not found | `unknown` | "no browser profile found; set the profile folder" |
| no `DevToolsActivePort` file | `off` | "<browser> is not running, or remote debugging is off" |
| file present, not connected | `on` | "waiting for Allow in the browser" or the connect error |
| connected | `on` | number of tabs |

The instruction line, in the tool output and in the UI: "In <browser>, open
chrome://inspect/#remote-debugging and tick 'Allow remote debugging for this browser
instance'." The tool returns it as an error result, so the model can tell the owner.

## 7. UI: Settings -> Browser

- Radio: "KAgent's own window (separate profile)" / "My running browser: attach
  through its DevTools remote debugging".
- Profile folder (detected, editable), trust level, Connect (starts the connection and
  makes the browser ask), Disconnect, and a status card with the instruction line and
  "Copy link".
- Note: "The backend applies after a daemon restart."

## 8. Security

- Attach is off by default. The owner opts in twice: the setting, and Allow per
  connection in the browser.
- With `strict` trust every action inside the owner's own tabs needs a card in every
  mode, including `full`. Eval always asks.
- Nothing reads passwords or cookies: the tool sees page text and screenshots, as in
  the own-profile mode.
- Attach mode keeps the tool budget, the audit log and the "page content is untrusted"
  rule.

## 9. Tests (loopback only, no root)

- `parse_devtools_active_port`: a valid file; port 0; port over 65535; missing path; a
  path outside `/devtools/browser/`; trailing newline.
- Tab rules table: the actions of section 4 for the three tab kinds.
- `BrowserSession` against a fake CDP server (a `TcpListener` on 127.0.0.1 in the
  test): one socket, two attached targets, responses routed by `sessionId`, a ping
  keeps it open, a dropped socket is reopened by the next call.
- Target choice: the pure function that picks among evaluated candidates.

## 10. Owner check (manual, once)

Tick the checkbox in Brave, Connect, click Allow, then ask by voice or chat: "read the
page I have open". A click in that tab shows a card; closing it is refused; a tab the
agent opened can be closed.
