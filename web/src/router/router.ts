// The router (shell.md section 2): the store owns the state; this mirrors the place into the address with the History
// API, reads it back on Back and Forward, and keeps a mirror of the stack (the History API cannot say whether Forward
// is possible). No dependency.
import { useSyncExternalStore } from "react";
import { getStore, openSession, patch, subscribe } from "../store";
import { notify } from "../components/toasts";
import { fromLegacy, labelOf, buildPath, parsePath, place, type Place } from "./route";
import { isQuiet, quiet } from "./quiet";
import { closeAllOverlays, closeTopOverlay } from "./overlays";

interface Entry {
  key: string;
  path: string;
  label: string;
  /** The scroll of each `data-scroll-root` element, put back on Back and Forward. */
  scroll: Record<string, number>;
}

const KEY = "kagent.nav";
const MAX = 200;
let entries: Entry[] = [];
let cur = 0;
let applying = false;
let started = false;
const listeners = new Set<() => void>();
let snapshot = { canBack: false, canForward: false, back: "", forward: "" };

function emit() {
  const back = cur > 0 ? entries[cur - 1] : undefined;
  const fwd = cur < entries.length - 1 ? entries[cur + 1] : undefined;
  snapshot = { canBack: !!back, canForward: !!fwd, back: back ? `Back to ${back.label}` : "Back", forward: fwd ? `Forward to ${fwd.label}` : "Forward" };
  listeners.forEach((l) => l());
}

function save() {
  try {
    sessionStorage.setItem(KEY, JSON.stringify({ cur, entries }));
  } catch {
    // the buttons then forget the stack at a reload; the browser's history still works
  }
}

/** Back and Forward as the buttons of the title bar read them. */
export function useHistoryState() {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => snapshot,
  );
}

/** The place the store describes. */
export function placeOfStore(): Place {
  const s = getStore();
  return place(s.view, {
    section: s.view === "settings" ? s.settingsSection || null : null,
    id: s.view === "settings" || s.view === "tickets" || s.view === "runs" ? s.placeId : null,
    sub: s.view === "runs" ? s.placeSub : null,
    session: s.view === "chat" ? s.activeSession : null,
  });
}

const search = () => location.search;
const newKey = () => Math.random().toString(36).slice(2, 10);

function titleOf(p: Place): string | undefined {
  return p.session ? getStore().sessions.find((x) => x.id === p.session)?.title || undefined : undefined;
}

/** True when `next` only fills an empty part of `prev` (the first conversation, the default settings page). */
function fills(prev: Place | null, next: Place): boolean {
  if (!prev || prev.view !== next.view) return false;
  if (next.view === "chat") return prev.session === null && next.session !== null;
  if (next.view === "settings") return prev.section === null && next.section !== null;
  return false;
}

function commit(path: string, mode: "push" | "replace") {
  const label = labelOf(parsePath(path) ?? place("chat"), titleOf(parsePath(path) ?? place("chat")));
  if (mode === "replace" || entries.length === 0) {
    const key = entries[cur]?.key ?? newKey();
    entries[cur] = { key, path, label, scroll: {} };
    history.replaceState({ key }, "", path + search());
  } else {
    const key = newKey();
    entries = [...entries.slice(0, cur + 1), { key, path, label, scroll: {} }].slice(-MAX);
    cur = entries.length - 1;
    history.pushState({ key }, "", path + search());
    // A new place starts at the top, also where the page element is the one that was there.
    requestAnimationFrame(() => document.querySelectorAll<HTMLElement>("[data-scroll-root]").forEach((el) => { el.scrollTop = 0; }));
  }
  save();
  emit();
}

function onStore() {
  if (applying || !started) return;
  const next = placeOfStore();
  const path = buildPath(next);
  const at = entries[cur]?.path;
  if (path === at) return;
  commit(path, isQuiet() || fills(at ? parsePath(at) : null, next) ? "replace" : "push");
}

/** Put a place into the store (the address was read: a Back, a Forward, a first load). */
function apply(p: Place) {
  applying = true;
  try {
    const s = getStore();
    const sameView = p.view === s.view;
    patch({
      view: p.view,
      settingsSection: p.view === "settings" && p.section ? p.section : s.settingsSection,
      placeId: p.view === "settings" || p.view === "tickets" || p.view === "runs" ? p.id : sameView ? s.placeId : null,
      placeSub: p.view === "runs" ? p.sub : null,
      sidebarDrawer: false,
    });
    if (p.view === "chat" && p.session && p.session !== s.activeSession) {
      openSession(p.session).catch(() => {
        // The conversation is gone (deleted, or a link from another machine): the latest one opens instead.
        notify("That conversation does not exist any more.", "info");
        const latest = [...getStore().sessions].sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))[0];
        quiet(() => patch({ activeSession: latest?.id ?? null }));
        if (latest) void openSession(latest.id);
      });
    }
  } finally {
    applying = false;
  }
  // What the store keeps of the place (a chat without a conversation yet) may differ from the path read.
  const path = buildPath(placeOfStore());
  if (entries[cur] && path !== entries[cur].path) {
    entries[cur] = { ...entries[cur], path, label: labelOf(placeOfStore(), titleOf(placeOfStore())) };
    history.replaceState({ key: entries[cur].key }, "", path + search());
    save();
  }
}

function restoreScroll(scroll: Record<string, number>) {
  const end = performance.now() + 2000;
  const todo = new Set(Object.keys(scroll));
  const tick = () => {
    for (const key of [...todo]) {
      const el = document.querySelector<HTMLElement>(`[data-scroll-root="${key}"]`);
      if (el && el.scrollHeight - el.clientHeight >= scroll[key] - 1) {
        el.scrollTop = scroll[key];
        todo.delete(key);
      }
    }
    if (todo.size > 0 && performance.now() < end) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function onPop(e: PopStateEvent) {
  // The browser's own Back (outside the app's buttons): whatever was open closes with the place.
  closeAllOverlays();
  const at = entries.findIndex((x) => x.key === e.state?.key);
  if (at >= 0) cur = at;
  else {
    // An entry this page did not make (the address was edited by hand): the path is read as it is.
    const key = newKey();
    entries = [...entries.slice(0, cur + 1), { key, path: location.pathname, label: "", scroll: {} }];
    cur = entries.length - 1;
    history.replaceState({ key }, "");
  }
  const p = parsePath(location.pathname) ?? place("chat");
  apply(p);
  entries[cur].label = labelOf(placeOfStore(), titleOf(placeOfStore()));
  restoreScroll(entries[cur].scroll);
  save();
  emit();
}

/** Back, as the buttons, `Alt+Left` and the mouse's button 3 do it: an open dialog, menu or sheet closes first. */
export function goBack() {
  if (closeTopOverlay()) return;
  if (cur > 0) history.back();
}

export function goForward() {
  if (closeTopOverlay()) return;
  if (cur < entries.length - 1) history.forward();
}

function installInput() {
  window.addEventListener("keydown", (e) => {
    if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
    e.preventDefault();
    if (e.key === "ArrowLeft") goBack();
    else goForward();
  });
  // The side buttons act on `mouseup` (and on `pointerup`, where a browser sends no mouse events for them; one press acts once). The
  // default action of the buttons is cancelled on `mousedown`, `mouseup` and `auxclick`, so the browser does not also navigate.
  const side = (e: MouseEvent) => e.button === 3 || e.button === 4;
  const lastAt: Record<number, number> = {};
  const act = (e: MouseEvent) => {
    if (!side(e)) return;
    e.preventDefault();
    if (e.timeStamp - (lastAt[e.button] ?? -1000) < 60) return;
    lastAt[e.button] = e.timeStamp;
    if (e.button === 3) goBack();
    else goForward();
  };
  // The press of a side button is not a click outside a menu (which would close it before Back could): nothing else sees it.
  for (const type of ["mousedown", "pointerdown", "auxclick"] as const) {
    window.addEventListener(
      type,
      (e) => {
        if (!side(e)) return;
        e.preventDefault();
        e.stopPropagation();
      },
      true,
    );
  }
  window.addEventListener("mouseup", act);
  window.addEventListener("pointerup", act);
  document.addEventListener(
    "scroll",
    (e) => {
      const el = e.target;
      if (!(el instanceof HTMLElement) || !el.dataset.scrollRoot || !entries[cur]) return;
      entries[cur].scroll[el.dataset.scrollRoot] = el.scrollTop;
      save();
    },
    true,
  );
}

/**
 * Start the router: the address (or an old fragment, `legacy`) is the place the window opens at. Runs after the token was
 * taken out of the address and before the store loads anything, so the first conversation read is the one named.
 */
export function startRouter(legacy: { view: string | null; section: string | null; session: string | null }) {
  if (started) return;
  let restored: { cur: number; entries: Entry[] } | null = null;
  try {
    restored = JSON.parse(sessionStorage.getItem(KEY) ?? "null");
  } catch {
    restored = null;
  }
  const fromAddress = parsePath(location.pathname);
  const old = fromLegacy(legacy.view, legacy.section, legacy.session);
  // An old link says where to go and wins; else the address; an address that is not one becomes the chat.
  const first = old ?? fromAddress ?? place("chat");
  const known = restored && Array.isArray(restored.entries) ? restored.entries.findIndex((x) => x.key === history.state?.key) : -1;
  if (restored && known >= 0 && !old) {
    entries = restored.entries;
    cur = known;
  } else {
    entries = [];
    cur = 0;
  }
  apply(first);
  const path = buildPath(placeOfStore());
  if (entries.length === 0 || entries[cur].path !== path || old || !fromAddress) {
    commit(path, "replace");
  } else {
    emit();
  }
  started = true;
  subscribe(onStore);
  window.addEventListener("popstate", onPop);
  installInput();
  emit();
  if (restored && known >= 0 && !old) restoreScroll(entries[cur].scroll);
}
