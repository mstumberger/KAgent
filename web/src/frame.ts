// Frameless window chrome for KAgent's own window: the page's title bar drags the
// window, a corner grip resizes it, double-click maximizes. The daemon moves the real
// X11 window (POST /api/ui/frame): the page only says start/stop, the daemon follows
// the real pointer (browser screen coordinates do not match X11 on multi-monitor setups).
import type React from "react";
import { api } from "./api";

export const isAppWindow = () => new URLSearchParams(location.search).has("app");

// Element, not HTMLElement: clicks on an icon land on its <svg>/<path>, which are SVG
// elements; treating them as drag handles captured the pointer and ate the click.
const interactive = (t: EventTarget | null) => t instanceof Element && !!t.closest("button, select, input, textarea, a, label, [data-nodrag]");

/** Follow the pointer in the daemon (X11 coordinates) until the button is released. */
function daemonDrag(e: React.PointerEvent<HTMLElement>, kind: string) {
  const el = e.currentTarget;
  el.setPointerCapture(e.pointerId);
  void api.post("/api/ui/frame", { drag: kind });
  const up = () => {
    void api.post("/api/ui/frame", { drag: "stop" });
    el.removeEventListener("pointerup", up);
    el.removeEventListener("pointercancel", up);
    el.removeEventListener("lostpointercapture", up);
  };
  el.addEventListener("pointerup", up);
  el.addEventListener("pointercancel", up);
  el.addEventListener("lostpointercapture", up);
}

/** onPointerDown for a drag handle (title bar, widget top). */
export function startDrag(e: React.PointerEvent<HTMLElement>) {
  if (!isAppWindow() || e.button !== 0 || interactive(e.target) || e.detail > 1) return;
  daemonDrag(e, "move");
}

/** onPointerDown for a resize edge or corner (`edge`: n, s, e, w, ne, nw, se, sw). */
export function startResize(e: React.PointerEvent<HTMLElement>, edge = "se") {
  if (!isAppWindow() || e.button !== 0) return;
  e.preventDefault();
  e.stopPropagation();
  daemonDrag(e, `resize:${edge}` as "resize");
}

export function toggleMaximize(e?: React.MouseEvent) {
  if (!isAppWindow() || (e && interactive(e.target))) return;
  void api.post("/api/ui/frame", { maximize: true });
}
