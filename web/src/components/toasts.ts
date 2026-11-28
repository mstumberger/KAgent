// The notices of the window as toasts (notices.md): what the daemon sends as `notice` events
// and what the page itself reports. A small store of its own (the store of the window writes
// into it through `notify`), newest first. Closed toasts are not kept.
import { useSyncExternalStore } from "react";
import { voice } from "../voice/controller";

export type NoticeKind = "error" | "recovery" | "info";

export interface Toast {
  id: number;
  /** What makes two notices the same one; `null` is never merged. */
  key: string | null;
  level: string;
  kind: NoticeKind;
  text: string;
  /** How many times the same key came while the toast was open. */
  count: number;
  /** The time of the latest, ms since the epoch. */
  at: number;
  sessionId: string | null;
}

/** A notice as the event carries it; the page's own failures give `text` alone. */
export interface Notice {
  level?: string;
  text: string;
  key?: string | null;
  sessionId?: string | null;
  kind?: NoticeKind | string | null;
}

/** An `info` or `recovery` toast closes after this long (notices.md section 2). */
export const INFO_MS = 6000;
/** How many toasts are shown before the rest fold into "N more". */
export const SHOWN = 4;

let toasts: Toast[] = [];
let next = 1;
/** The keys of the error toasts said aloud in the live session already. */
const said = new Set<string>();
const listeners = new Set<() => void>();

function set(list: Toast[]) {
  toasts = list;
  listeners.forEach((l) => l());
}

/** `error` and `warn` are `error`, `info` is `info`, unless the notice says its kind. */
export function kindOf(n: Notice): NoticeKind {
  if (n.kind === "error" || n.kind === "recovery" || n.kind === "info") return n.kind;
  return n.level === "info" ? "info" : "error";
}

export function pushNotice(n: Notice) {
  const kind = kindOf(n);
  const key = n.key || null;
  const at = Date.now();
  // A recovery closes the open error toast of its key, then shows itself.
  let list = kind === "recovery" && key ? toasts.filter((t) => !(t.key === key && t.kind === "error")) : toasts;
  const same = key ? list.find((t) => t.key === key && t.kind === kind) : undefined;
  if (same) {
    const merged = { ...same, count: same.count + 1, text: n.text, at, sessionId: n.sessionId ?? same.sessionId };
    list = [merged, ...list.filter((t) => t !== same)];
  } else {
    list = [{ id: next++, key, level: n.level ?? "info", kind, text: n.text, count: 1, at, sessionId: n.sessionId ?? null }, ...list];
  }
  set(list);
  // An error is said once per key in a live session, by the status line of the voice.
  if (kind === "error" && key && !said.has(key) && voice.active) {
    said.add(key);
    voice.say(n.text);
  }
  if (kind === "recovery" && key) said.delete(key);
}

/** The page's own message: a failure (`error`, stays until closed) or a result (`info`, 6 s). */
export function notify(text: string, kind: "error" | "info" = "error", sessionId?: string) {
  pushNotice({ level: kind === "info" ? "info" : "error", text, kind, sessionId });
}

export function dismiss(id: number) {
  set(toasts.filter((t) => t.id !== id));
}

export function dismissAll() {
  set([]);
}

export function getToasts(): Toast[] {
  return toasts;
}

export function useToasts(): Toast[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    getToasts,
  );
}
