# Spec: inline widgets (S33)

The agent can put a small interactive HTML panel inside its answer: a gauge for the
doctor score, bars for disk use, a table of top processes, a sparkline of traffic. It
is ZCode's `visualize` feature, reduced to static fragments.

## 1. Tool

`widget_show` (tools.md section 22) saves the fragment and returns the marker line
`::widget{id="<sessionId>/<n>"}`. The model puts the marker on its own line where the
panel belongs.

## 2. Rendering (`web/src/components/Widget.tsx`, `components/Markdown.tsx`)

- `Markdown` gets a directive step: a line that matches `^::([a-z]+)\{(.*)\}$` with a
  JSON object renders a directive component; an unknown name renders as text. `widget`
  is the only directive.
- `Widget` fetches `GET /api/widgets/:session/:id` (bearer token) and shows
  `<iframe sandbox="allow-scripts" referrerpolicy="no-referrer">` with a `srcDoc`
  built by the UI: `<!doctype html><html data-theme="..."><head>` with the CSP meta
  tag and a `<style>` that defines the theme variables, then `<body>`, the fragment,
  the resize script, `</body></html>`.
- CSP meta tag in every fragment:
  `default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src
  data:; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'`.
- The sandbox has no `allow-same-origin`: the fragment has an opaque origin. It cannot
  read the app's storage or cookies, cannot navigate the top window, cannot submit a
  form and cannot open a network connection.
- Theme variables, from the current theme: `--k-night`, `--k-glass`, `--k-ink`,
  `--k-dim`, `--k-ice`, `--k-ok`, `--k-bad`, `--k-signal`.
- Height: the UI's resize script posts `{source: "kagent-widget", height}` to
  `parent.postMessage` on load and on resize (`ResizeObserver`). The UI accepts the
  message only from that iframe's `contentWindow`. Height 300 to 600 px, 360 by
  default. Nothing else crosses the boundary.
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

`<data>/widgets/<sessionId>/<n>.html`, mode 0600, at most 24 KB each, at most 200 per
session (the oldest are overwritten). Deleted with the session.

## 6. Security

- A fragment is model output and is rendered without privileges, under the sandbox and
  CSP above. The route needs the bearer token like every `/api` route.
- There is no bridge to the agent and no live data in this stage. (Later, if wanted: a
  `window.kagent` channel through `postMessage` with a fixed set of messages.)

## 7. Tests

- `widgets.rs`: the size cap; an unknown id is 404; files go with the session; export
  replaces the markers.
- `speakable` drops `::widget{...}` (voice.md).
- UI, in headless Chrome, dark and light: a gauge, a table and a sparkline render; the
  iframe has `sandbox="allow-scripts"`; a fragment that tries `fetch`,
  `top.location` and `localStorage` stays blocked; the height follows the content.
