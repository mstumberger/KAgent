# Spec: inline widgets (S33)

The agent can put a small interactive HTML panel inside its answer: a gauge for the
doctor score, bars for disk use, a table of top processes, a sparkline of traffic. It
is ZCode's `visualize` feature, reduced to static fragments.

## 1. Tool

`widget_show` (tools.md section 22) saves the fragment and returns the marker line
`::widget{id="<sessionId>/<n>"}`. The model puts the marker on its own line where the
panel belongs.

## 2. Rendering (`web/src/components/Widget.tsx`, `components/Markdown.tsx`)

- `Markdown` gets a directive step: a line that matches `^::([a-z]+)\{(.*)\}$` renders
  a directive component when the name is `widget` and the braces hold the marker as the
  tool answers it (`id="<session>/<n>"`) or the same as a JSON object; an unknown name,
  or anything else in the braces, renders as text. The marker itself is not JSON.
  `widget` is the only directive.
- `Widget` fetches `GET /api/widgets/:session/:id` (bearer token) and shows
  `<iframe sandbox="allow-scripts" referrerpolicy="no-referrer">` with a `srcDoc`
  built by the UI: `<!doctype html><html data-theme="..."><head>` with the CSP meta
  tag and a `<style>` that defines the theme variables, then `<body>`, the fragment,
  the resize script, `</body></html>`. The canvas of the frame is `--k-glass` and its
  `color-scheme` is the theme's. The tool's `title` is shown above the panel.
- CSP meta tag in every fragment:
  `default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src
  data:; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'`.
- The sandbox has no `allow-same-origin`: the fragment has an opaque origin. It cannot
  read the app's storage or cookies, cannot navigate the top window, cannot submit a
  form, open a window, or fetch, connect a socket or load an image, a font, a style or
  a script from the network (measured: 10 attempts, 10 blocked). It can still send its
  own frame to another address (one GET, without referrer or cookies): the app's
  `frame-src 'none'` stops that (section 6).
- Theme variables, from the current theme (the page's tokens of the same names):
  `--k-night`, `--k-glass`, `--k-ink`, `--k-dim`, `--k-ice`, `--k-ok`, `--k-bad`,
  `--k-signal`.
- Height: the UI's resize script posts `{source: "kagent-widget", height}` to
  `parent.postMessage` on load, on resize, on a change of the content and three times
  after loading (a frame off the screen gets no resize callback in Chrome). The UI
  accepts the message only from that iframe's `contentWindow`. Height 300 to 600 px,
  360 by default. Nothing else crosses the boundary.
- A fragment is fetched once per id. A missing one (404) shows "widget no longer
  available".
- Session export (`sessions export`) replaces the marker with `[widget <n>]`.

## 3. Authoring guidance

- Bundled skill `skills/widget-design/SKILL.md`, ported from ZCode's `visualize`
  guidance (at most 60 lines): one message per widget; layout first, then content;
  theme variables only; no external fonts, scripts, images or links; no forms; under
  120 lines; readable at 300 px wide; nothing loops forever; data goes inline as
  constants.
- The tool description carries five rules: static HTML with inline CSS and JS; the
  `--k-*` variables; no external resources; one idea per widget; call it once per
  answer, with the final data.

## 4. Voice and other channels

`speakable` (`voice.md` section 7) drops directive lines; Telegram (`bots.md` section
3) drops them too. A voice answer describes the content in words.

## 5. Storage

`<data>/widgets/<sessionId>/<n>.html`, mode 0600, at most 24 KB each, with its title in
`<n>.title` and the last number in `.count`; at most 200 per session (the oldest are
overwritten). Folders 0700. Deleted with the session and with the subagents deleted
with it.

The route answers `text/html; charset=utf-8`, the title percent-encoded in
`X-KAgent-Title`, `Cache-Control: no-store`: 400 for a session id that is not plain or
an id that is not a number from 1 to 200, 404 when there is no such fragment (a link is
not followed).

## 6. Security

- A fragment is model output and is rendered without privileges, under the sandbox and
  CSP above. The route needs the bearer token like every `/api` route.
- The renderer never uses `innerHTML` for model output.
- There is no bridge to the agent and no live data in this stage. (Later, if wanted: a
  `window.kagent` channel through `postMessage` with a fixed set of messages.)
- The app's own document (`web/index.html`, decision of 2026-11-27) has a CSP meta tag
  with nothing remote: `default-src 'self'; script-src 'self' 'unsafe-inline' blob:;
  style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:;
  media-src 'self' blob: data:; connect-src 'self'; worker-src 'self' blob:; frame-src
  'none'; child-src 'none'; object-src 'none'; base-uri 'none'; form-action 'self'`.
  - A srcdoc frame inherits the policy of the app (measured in Chrome: with
    `script-src 'self'` a widget's inline script does not run), so `script-src` keeps
    `'unsafe-inline'`; the widget's own meta stays the stricter one inside the frame.
  - `frame-src 'none'` still loads the srcdoc frame and blocks the frame's navigation to
    another address (measured: the GET of a widget's `location.href = ...` no longer
    arrives). `blob:` in `script-src` is the voice audio worklet. `connect-src 'self'`
    covers the same-origin WebSockets (`/ws`, `/live`).
  - A remote image in a chat answer is not loaded.

## 7. Tests

- `widgets.rs`: the size cap; an id that is not a plain id; an unknown id is 404; the
  files go with the session; the title and `.count` files; export replaces the markers.
- `speakable` drops `::widget{...}` (voice.md).
- UI, in headless Chrome, dark and light: a gauge, a table and a sparkline render; the
  iframe has `sandbox="allow-scripts"`; a fragment that tries `fetch`, `top.location`,
  the storages, a form, an image or a socket stays blocked; the height follows the
  content.

## 8. Not in scope

- Live data, buttons that call tools, a shared template gallery, the whiteboard.
