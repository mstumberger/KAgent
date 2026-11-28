// The screen watch as the daemon reports it (screen-watch.md section 4). The title bar chip reads
// `store.watch` and nothing else: the page has no way to hide it while frames are being taken.
import { api } from "./api";
import { patch } from "./store";

export interface Watch {
  id: string;
  sessionId: string;
  goal: string;
  target: "window" | "monitor";
  windowId?: string;
  windowTitle?: string;
  /** When the time is up (RFC 3339). */
  until: string;
  minutes: number;
  everySec: number;
  mode: "once" | "repeat";
  state: "watching" | "met" | "expired" | "stopped" | "failed";
  looks: number;
  lastLookAt?: string;
  lastNote: string;
  /** The screen is locked: no frame is taken. */
  paused: boolean;
  reason?: string;
}

/** The watch that runs now, from `GET /api/screen-watch` (null when none): at the start, and again from time to time in case an event was missed. */
export async function refreshWatch() {
  try {
    const w = await api.get<Watch | null>("/api/screen-watch");
    patch({ watch: w && w.state === "watching" ? w : null });
  } catch {
    // The daemon is not there: the chip stays as the last event left it.
  }
}

/** Ends the watch (the chip's Stop). The daemon answers with the watch as it ended, or 404 when it was over already. */
export async function stopWatch() {
  try {
    await api.del("/api/screen-watch");
  } catch {
    // Already over: the next read says so.
  }
  await refreshWatch();
}

/** Whole minutes left (rounded up), never below 0. */
export function minutesLeft(w: Watch, now: number): number {
  return Math.max(0, Math.ceil((Date.parse(w.until) - now) / 60000));
}

/** What the watch looks at, in words. */
export function whatOf(w: Watch): string {
  return w.target === "monitor" ? "the monitor" : w.windowTitle || "the active window";
}
