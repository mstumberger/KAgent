// The notices of the window as toasts (notices.md): what the daemon sends as `notice` events
// and what the page itself reports. A small store of its own (the store of the window writes
// into it through `notify`), newest first. Closed toasts are not kept.
import { useSyncExternalStore } from "react";
import { voice } from "../voice/controller";

export type NoticeKind = "error" | "recovery" | "info";

/** A button of a notice: the window posts `body` to `path` of the daemon when it is pressed. */
export interface NoticeAction {
  label: string;
  path: string;
  body?: unknown;
}

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
  action: NoticeAction | null;
}

/** A notice as the event carries it; the page's own failures give `text` alone. */
export interface Notice {
  level?: string;
  text: string;
  key?: string | null;
  sessionId?: string | null;
  kind?: NoticeKind | string | null;
  action?: NoticeAction | null;
}

/** An `info` or `recovery` toast closes after this long (notices.md section 2). */
export const INFO_MS = 6000;
/** How many toasts are shown before the rest fold into "N more". */
export const SHOWN = 4;

/** One line of the Activity section (work-windows.md section 5.2): the last notices, kept for the page's life. */
export interface ActivityEntry {
  id: number;
  key: string | null;
  level: string;
  kind: NoticeKind;
  text: string;
  count: number;
  at: number;
  /** `null` for the page's own notices: they are listed in every conversation. */
  sessionId: string | null;
}

/** How many notices of a conversation the Activity section keeps. */
export const ACTIVITY_MAX = 20;

let toasts: Toast[] = [];
let activity: ActivityEntry[] = [];
let next = 1;
/** The keys of the error toasts said aloud in the live session already. */
const said = new Set<string>();
const listeners = new Set<() => void>();

function set(list: Toast[]) {
  toasts = list;
  listeners.forEach((l) => l());
}

/** A notice goes in the log whatever happens to its toast; the same key and kind in a row is one entry with a count. */
function record(n: Notice, kind: NoticeKind, key: string | null, at: number) {
  const sessionId = n.sessionId ?? null;
  const same = key ? activity.find((a) => a.key === key && a.kind === kind && a.sessionId === sessionId) : undefined;
  let list: ActivityEntry[];
  if (same) {
    list = [{ ...same, count: same.count + 1, text: n.text, at }, ...activity.filter((a) => a !== same)];
  } else {
    list = [{ id: next++, key, level: n.level ?? "info", kind, text: n.text, count: 1, at, sessionId }, ...activity];
  }
  // At most 20 per conversation, so a chatty one does not push out another's.
  const seen = new Map<string | null, number>();
  activity = list.filter((a) => {
    const c = (seen.get(a.sessionId) ?? 0) + 1;
    seen.set(a.sessionId, c);
    return c <= ACTIVITY_MAX;
  });
}

/** The last notices of a conversation, newest first, with the page's own ones (no session). */
export function activityOf(sessionId: string | null): ActivityEntry[] {
  return activity.filter((a) => a.sessionId === null || a.sessionId === sessionId).slice(0, ACTIVITY_MAX);
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
  record(n, kind, key, at);
  // A recovery closes the open error toast of its key, then shows itself.
  let list = kind === "recovery" && key ? toasts.filter((t) => !(t.key === key && t.kind === "error")) : toasts;
  const same = key ? list.find((t) => t.key === key && t.kind === kind) : undefined;
  if (same) {
    const merged = { ...same, count: same.count + 1, text: n.text, at, sessionId: n.sessionId ?? same.sessionId, action: n.action ?? same.action };
    list = [merged, ...list.filter((t) => t !== same)];
  } else {
    list = [{ id: next++, key, level: n.level ?? "info", kind, text: n.text, count: 1, at, sessionId: n.sessionId ?? null, action: n.action ?? null }, ...list];
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

export function getActivity(): ActivityEntry[] {
  return activity;
}

/** The log as it is now; it changes (a new array) with each notice. */
export function useActivity(): ActivityEntry[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    getActivity,
  );
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

/** How many notices the conversation has in the log. */
export function useActivityCount(sessionId: string | null): number {
  useActivity();
  return activityOf(sessionId).length;
}
