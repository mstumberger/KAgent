// An inline widget (docs/specs/widgets.md): the marker line `::widget{id="<session>/<n>"}` of an
// answer becomes a panel. The fragment is model output and never enters the app's document: it
// runs in a frame without `allow-same-origin` (an opaque origin: no storage, no cookies, no way
// up to this page) under a CSP that allows inline script and style and nothing else. Only the
// height comes back, by postMessage, and only from this frame's own window.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import "./widget.css";

const TOKEN_KEY = "kagent.token";
/** The policy of every panel (widgets.md section 2), first in the document so it covers all of it. */
const CSP = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'";
/** The height of a panel, in px: what the fragment reports, between these; 360 until it does. */
const MIN_HEIGHT = 300;
const MAX_HEIGHT = 600;
const DEFAULT_HEIGHT = 360;
/** The variables a fragment may use: the app's tokens of the same names, read from the page. */
const TOKENS = ["night", "glass", "ink", "dim", "ice", "ok", "bad", "signal"];

/** Posts the height of the content to the parent: on load, on resize, on a change of the content, and three times after loading. */
const RESIZE_SCRIPT = `(function () {
  "use strict";
  function post() {
    var root = document.documentElement, body = document.body;
    // Until the frame is laid out there is nothing to measure: its height is 0 then.
    if (!root.clientHeight || !body) return;
    // The height of the content: the body's own box. (The root's scrollHeight is at least the
    // frame, so it would feed the frame's own height back.)
    var h = Math.ceil(Math.max(body.scrollHeight, body.getBoundingClientRect().height)) || root.scrollHeight;
    try { parent.postMessage({ source: "kagent-widget", height: h }, "*"); } catch (e) {}
  }
  window.addEventListener("load", post);
  // The body, not the root: the root changes with the frame, the body with the content.
  if (window.ResizeObserver) { new window.ResizeObserver(post).observe(document.body); }
  // A frame the browser is not rendering (off the screen) gets no resize callback, and right
  // after it loaded it has no layout yet: its own changes and three later looks cover that.
  new MutationObserver(post).observe(document.body, { childList: true, subtree: true, attributes: true, characterData: true });
  [60, 300, 1200].forEach(function (ms) { window.setTimeout(post, ms); });
  post();
})();`;

type Theme = "dark" | "light";

// App owns the theme and writes `data-theme` on the root; a panel follows it the moment it changes.
const readTheme = (): Theme => (document.documentElement.dataset.theme === "light" ? "light" : "dark");
function subscribeTheme(notify: () => void) {
  const observer = new MutationObserver(notify);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => {
    observer.disconnect();
  };
}

/** The theme as the page computes it now, under the names a fragment uses. */
function themeVariables(): Record<string, string> {
  const style = getComputedStyle(document.documentElement);
  return Object.fromEntries(TOKENS.map((token) => [`--k-${token}`, style.getPropertyValue(`--${token}`).trim()]));
}

const escapeText = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** The document of a panel, written here: the head (the policy, then the theme), then the fragment as the body. */
function panelDocument(theme: Theme, variables: Record<string, string>, title: string, fragment: string): string {
  const themeStyle = TOKENS.map((token) => `--k-${token}:${variables[`--k-${token}`]};`).join("");
  return [
    "<!doctype html>",
    `<html data-theme="${theme}"><head>`,
    `<meta http-equiv="Content-Security-Policy" content="${CSP}">`,
    '<meta charset="utf-8">',
    `<title>${escapeText(title)}</title>`,
    // The frame is the card: the panel colour is its canvas, whatever a browser paints under a
    // frame that says nothing (Chrome paints white). Native controls follow the theme too.
    `<style>:root{${themeStyle}color-scheme:${theme}}html{margin:0;padding:0;background:var(--k-glass);color:var(--k-ink);font:14px/1.45 system-ui,sans-serif}body{margin:0}</style>`,
    "</head><body>",
    fragment,
    `<script>${RESIZE_SCRIPT}</script>`,
    "</body></html>",
  ].join("\n");
}

type Fragment = { html: string; title: string };
/** What was fetched: a fragment, `null` when the daemon has none (404), `undefined` while loading. */
const fragments = new Map<string, Promise<Fragment | null>>();

/** The fragment of an id, asked for once per page whatever asks again; a failed ask is not kept. */
function fragmentOf(id: string): Promise<Fragment | null> {
  const known = fragments.get(id);
  if (known) return known;
  const pending = fetch(`/api/widgets/${id}`, {
    headers: { Authorization: `Bearer ${localStorage.getItem(TOKEN_KEY) ?? ""}` },
    cache: "no-store",
  })
    .then(async (res) => {
      if (res.status === 404) return null;
      if (!res.ok) throw new Error(`widget ${id}: HTTP ${res.status}`);
      return { html: await res.text(), title: decodeTitle(res.headers.get("x-kagent-title")) };
    })
    .catch((error: unknown) => {
      fragments.delete(id);
      throw error;
    });
  fragments.set(id, pending);
  return pending;
}

/** The title as the route sends it: percent-encoded (the header holds ASCII only). */
function decodeTitle(value: string | null): string {
  if (!value) return "";
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

type Loaded = Fragment | "missing" | "failed" | undefined;

export default function Widget({ id }: { id: string }) {
  const theme = useSyncExternalStore(subscribeTheme, readTheme, readTheme);
  // Read on each render: a panel renders a handful of times (loaded, its height, the theme).
  const variables = themeVariables();
  const frame = useRef<HTMLIFrameElement>(null);
  const [loaded, setLoaded] = useState<Loaded>(undefined);
  const [sized, setSized] = useState({ id: "", height: DEFAULT_HEIGHT });
  /** The height last taken for this id. */
  const shown = useRef(-1);

  useEffect(() => {
    let alive = true;
    shown.current = -1;
    fragmentOf(id).then(
      (got) => {
        if (alive) setLoaded(got === null ? "missing" : got);
      },
      () => {
        if (alive) setLoaded("failed");
      },
    );
    return () => {
      alive = false;
    };
  }, [id]);

  useEffect(() => {
    const onMessage = (event: MessageEvent<unknown>) => {
      const data = event.data as { source?: unknown; height?: unknown } | null;
      // Only this frame's own window may say how tall it is, and only that.
      if (event.source !== frame.current?.contentWindow || !data || data.source !== "kagent-widget") return;
      if (typeof data.height !== "number" || !Number.isFinite(data.height)) return;
      const height = Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, Math.ceil(data.height)));
      // The same height again is not news: a panel that animates would re-render for nothing.
      if (height === shown.current) return;
      shown.current = height;
      setSized({ id, height });
    };
    window.addEventListener("message", onMessage);
    return () => {
      window.removeEventListener("message", onMessage);
    };
  }, [id]);

  if (loaded === undefined) return <div className="inline-widget inline-widget-wait" role="status" aria-label="widget is loading" />;
  if (loaded === "missing" || loaded === "failed") {
    return (
      <div className="inline-widget inline-widget-missing" role="note">
        {loaded === "missing" ? "widget no longer available" : "widget could not be loaded"}
      </div>
    );
  }
  return (
    <figure className="inline-widget" data-widget={id}>
      {loaded.title ? <figcaption className="inline-widget-title">{loaded.title}</figcaption> : null}
      <iframe
        ref={frame}
        title={loaded.title || "widget"}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        srcDoc={panelDocument(theme, variables, loaded.title, loaded.html)}
        style={{ height: sized.id === id ? sized.height : DEFAULT_HEIGHT }}
      />
    </figure>
  );
}
