// The actions of the sidebar (sidebar.md section 5.6): the folder and the pin of a session,
// the projects it offers, and its own state. It writes through the store's `patch` and is
// read through `useStore`; what the browser remembers goes to `prefs.ts`.
import { getProjects, putPin, putProject, type SessionMeta } from "../../sessionApi";
import { applyMeta, deleteSession, getStore, newSession, openSession, patch, setView } from "../../store";
import type { View } from "../../nav";
import { notify } from "../toasts";
import { DRAWER_BELOW, writePrefs, type SidebarView } from "./prefs";

const narrow = () => typeof window.matchMedia === "function" && window.matchMedia(`(max-width: ${DRAWER_BELOW - 1}px)`).matches;
export const isNarrow = narrow;

function remember() {
  const s = getStore();
  writePrefs({ view: s.sidebarView, collapsed: s.sidebarCollapsed, closed: s.sidebarClosed });
}

/** The toggle of the title bar: the drawer of a narrow window, else the rail and back. */
export function toggleSidebar() {
  if (narrow()) patch({ sidebarDrawer: !getStore().sidebarDrawer });
  else {
    patch({ sidebarCollapsed: !getStore().sidebarCollapsed });
    remember();
  }
}

export const closeDrawer = () => patch({ sidebarDrawer: false });

export function setSidebarView(view: SidebarView) {
  patch({ sidebarView: view });
  remember();
}

/** Open or close the section of a folder; the closed ones are remembered. */
export function toggleFolder(path: string) {
  const closed = getStore().sidebarClosed;
  patch({ sidebarClosed: closed.includes(path) ? closed.filter((p) => p !== path) : [...closed, path] });
  remember();
}

/** "New session" (in a folder when `project` is given): it opens in the chat and the drawer closes. */
export async function createSession(project?: string) {
  try {
    await newSession(project);
    patch({ view: "chat", sidebarDrawer: false, sheet: null });
  } catch (e) {
    notify(`Session not created: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/** A row was picked: the session opens in the chat. */
export function openFromSidebar(id: string) {
  setView("chat");
  patch({ sidebarDrawer: false, sheet: null });
  void openSession(id);
}

/** A view of the list at the bottom was picked. */
export function showFromSidebar(view: View) {
  setView(view);
  patch({ sidebarDrawer: false });
}

/** Attach a folder to a session, or detach it (`null`). A refusal is thrown with the daemon's message, for the popup that asked. */
export async function setProject(id: string, path: string | null): Promise<SessionMeta> {
  const meta = await putProject(id, path);
  applyMeta(id, meta);
  return meta;
}

/** Pin or unpin; a refusal is a notice. */
export async function setPinned(id: string, pinned: boolean) {
  try {
    applyMeta(id, await putPin(id, pinned));
  } catch (e) {
    notify(`${pinned ? "Not pinned" : "Not unpinned"}: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/** The projects "Attach folder" lists, read each time it opens. A daemon that cannot answer leaves the last list. */
export async function refreshProjects() {
  try {
    const r = await getProjects();
    patch({ projects: r.projects, projectRoots: r.roots });
  } catch {
    // keep the last list
  }
}

export const removeSession = (id: string) => void deleteSession(id);
