# Spec: browser backends: own window, or the running Brave (S27)

Before this stage the `browser` tool only drove a browser KAgent launches itself, with
its own profile and a DevTools port (tools.md section 14). The owner's real browser,
with their signed-in sessions, was out of reach. Chromium 136 and later ignore the
DevTools port on the default profile, so attaching needs the browser's own consent
flow.

## 1. How attach works (verified on this machine, 2026-11-24)

- In Brave, `chrome://inspect/#remote-debugging` has the checkbox "Allow remote
  debugging for this browser instance". The page warns that it gives external apps full
  control of the browser, including saved data and cookies. Brave 156 has it. No
  extension is involved.
- When it is on and the browser runs, the browser writes `DevToolsActivePort` into its
  user data directory: line 1 is the port, line 2 is the browser WebSocket path. This
  is what `chrome-devtools-mcp` reads for its auto-connect (its `BrowserManager`
  connects to `ws://127.0.0.1:<port><path>`). A browser started with
  `--remote-debugging-port` writes the same file.
- Every new WebSocket connection makes the browser ask "Allow". Nobody at the screen
  means the connection waits.
- The setting survives browser restarts.

## 2. Settings (config.md section 11)

- `browser.backend`: `own` (default, unchanged) or `attach`.
- `browser.attachProfile`: the browser's user data directory. Empty = the first one
  that exists: `~/.config/BraveSoftware/Brave-Browser`,
  `~/.config/google-chrome`, `~/.config/microsoft-edge`, `~/.config/chromium`.
- `browser.attachTrust`: `strict` (default) or `normal` (section 4).
- The backend and the trust level are read when the daemon starts, like `port` and
  `binary`. The profile folder is read again by the status and connect routes and by
  every call, so a saved folder applies at once. It must be an absolute path or start
  with `~/` (400 on a patch, refused on load).

## 3. Connection (`kagentd/src/browser/attach.rs`, `attach/{profile,socket,tabs}.rs`, `browser/cdp.rs`)

- `parse_devtools_active_port(text) -> Option<(u16, String)>`: port 1..65535, a path
  that starts with `/devtools/browser/`.
- `BrowserSession`: one browser-level WebSocket, multiplexed with `sessionId`
  (`Target.attachToTarget {flatten: true}`):
  - `pages() -> Vec<TargetInfo>` (`Target.getTargets`, type `page`, not
    `chrome-extension://`).
  - `attach(target_id) -> AttachedTab` with `call(method, params)` and `detach()`.
  - `create_target(url)`, `close_target(target_id)`, `activate(target_id)`.
  - A ping (`Browser.getVersion`) every 10 s; no answer in 5 s closes the socket. A
    dropped socket is reopened on the next use.
- The daemon connects lazily, on the first tool call, with a 20 s deadline (the Allow
  prompt). After 10 idle minutes the socket is closed, so the browser's "controlled"
  indicator goes away; the next call reconnects and the browser asks again. The calls
  of one batch share one connect attempt, and none is retried for 3 s after a failure.
- `Browser::run` gets its tab through one internal trait (`TabTarget`, in
  `browser/target.rs`), with the own-profile and the attached implementations, so the
  actions (`browser/actions.rs`: read, click, type, key, scroll, screenshot, console,
  eval, search) are the same code for both backends.

## 4. Tab rules in attach mode

Agent tabs are the tabs the agent opened in this daemon run (`Target.createTarget`),
tracked by target id; `tabs` marks them "(opened by the agent)" and `open` says the
new tab is the agent's.

| Action | Agent tab | Owner's tab, `strict` | Owner's tab, `normal` |
|---|---|---|---|
| read, screenshot, console | read-only | read-only | read-only |
| open (new tab), activate, back | no approval | no approval | no approval |
| navigate, scroll | no approval | asks, always (never rememberable) | no approval |
| click, type, key | approval, medium, rememberable | asks always, high | approval, medium, rememberable |
| close | no approval | refused: "only tabs the agent opened can be closed" | refused the same way |
| eval | asks always, high | always asks, high | always asks, high |
| launch | not available: "this is the owner's running browser" | same | same |

- The "always" rules, and `eval` (in an agent tab too), use the permission engine's
  destructive gate: a card in every mode, including `full`, never rememberable; by
  voice only an explicit confirmation answers. (The engine lets `always_ask` through in
  `full` mode, and the spec wants a card there.)
- A call judged for one of the owner's tabs has the rule key `browser@owner <action>`
  and a card summary that ends in "(the owner's tab)"; any other call has
  `browser <action>`. An allowance remembered for one kind of tab, or in the own
  window, never covers the other.
- `launch` stays in the schema and is refused when the call runs.
- Target without `tab`: the agent's most recent agent tab, else the tab whose
  `document.visibilityState` is `visible` (one `Runtime.evaluate` per candidate, at
  most 12), else the first page.
- The assessment takes a call without `tab` as an agent tab while one is open, and a
  call that names a tab by a word of its title or URL as one of the owner's. The rule
  is enforced again when the call runs: a call that resolves to a tab that needs more
  than the engine was shown does not run, and says so. A needle of 32 hex characters is
  an id and matches by id only.
- Every output starts with `Browser: <product name> (the owner's own window)`.

## 5. Tool description

- In attach mode the `browser` description says: these are the owner's real tabs with
  their signed-in sessions; act in a tab only when the request is about it; only tabs
  you opened can be closed. It follows the trust level: `strict` says acting asks every
  time; `normal` says clicks, typing and keys ask and navigating and scrolling do not.
- No new input field; tab targeting is unchanged.

## 6. Status and errors (`GET /api/browser/status`, `attach.debugging`)

| Situation | `debugging` | Detail |
|---|---|---|
| profile directory not found | `unknown` | "no browser profile found; set the profile folder" |
| no `DevToolsActivePort` file | `off` | "<browser> is not running, or remote debugging is off" |
| file present, not connected | `on` | "waiting for Allow in the browser" or the connect error. A timed-out attempt counts as waiting. `browser` is the product name once connected, else the profile's browser; `port` comes from the port file |
| connected | `on` | number of tabs |

The instruction line, in the tool output and in the UI: "In <browser>, open
chrome://inspect/#remote-debugging and tick 'Allow remote debugging for this browser
instance'." The tool returns it as an error result, so the model can tell the owner.

## 7. UI: Settings -> Browser

- Radio: "KAgent's own window (separate profile)" / "My running browser: attach
  through its DevTools remote debugging".
- Profile folder (detected, editable, with its own "Save folder"), trust level
  ("Strict (asks every time)" / "Normal (as in KAgent's own window)"), Connect (starts
  the connection and makes the browser ask), Disconnect, and a status card with the
  instruction line, "Copy link" and the daemon's detail in every state.
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
- The tab rules table: the actions of section 4 for the three tab kinds, also through
  `PermissionEngine` in `full` mode with a stored rule.
- `BrowserSession` against a fake CDP server (`browser/attach/fake.rs`, a `TcpListener`
  on 127.0.0.1 in the test): one socket, two attached targets, responses routed by
  `sessionId`, a ping keeps it open and a silent one is dropped, a dropped socket is
  reopened by the next call, an idle one is closed.
- Target choice: the pure function that picks among evaluated candidates.
- One test is `#[ignore]`d: the round trip on a real browser. By hand, against a
  throwaway instance (its own profile, `--headless=new`,
  `--remote-debugging-port=0`):
  `KAGENT_S27_REAL=1 KAGENT_S27_PROFILE=<profile folder> cargo test -p kagentd --bin kagentd real_browser -- --ignored --nocapture`.

## 10. Owner check (manual, once)

Tick the checkbox in Brave, Connect, click Allow, then ask by voice or chat: "read the
page I have open". A click in that tab shows a card; closing it is refused; a tab the
agent opened can be closed.
