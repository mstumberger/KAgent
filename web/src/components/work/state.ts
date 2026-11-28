// The state of the work panel and of the side pane (work-windows.md sections 2 and 5.3): the tabs
// of the pane, the background tasks of the sessions that were asked, and the sections the owner
// folded. A small store of its own; the events reach it through `onBackgroundTask`.
import { useSyncExternalStore } from "react";
import { api } from "../../api";
import type { Message } from "../../api";
import { getStore, patch, useStore } from "../../store";
import type { SessionInfo } from "../../settings/subagents/api";
import { notify } from "../toasts";

export type SideKind = "subagent" | "plan" | "output";

/** A tab of the side pane. `owner` is the session whose work it shows; `sessionId` the child, the plan's session or the session that started the task. */
export interface SideTab {
  key: string;
  kind: SideKind;
  owner: string;
  sessionId: string;
  taskId?: string;
  title: string;
}

export type TaskState = "running" | "done" | "failed" | "killed";

/** A background shell task as `GET /api/sessions/:id/background` lists it. */
export interface BackgroundTaskView {
  id: string;
  sessionId: string;
  command: string;
  status: TaskState;
  exitCode: number | null;
  startedAt: string;
  seconds: number;
  outputFile: string;
}

const CLOSED_KEY = "kagent.work.closed";

interface Work {
  tabs: SideTab[];
  /** The key of the tab shown, per owner. */
  active: Record<string, string>;
  tasks: Record<string, BackgroundTaskView[]>;
  /** The sections the owner folded (stored), and those the owner opened though they were empty (this page only). */
  closed: string[];
  opened: string[];
}

function readClosed(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(CLOSED_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

let work: Work = { tabs: [], active: {}, tasks: {}, closed: readClosed(), opened: [] };
const listeners = new Set<() => void>();

function set(next: Partial<Work>) {
  work = { ...work, ...next };
  listeners.forEach((l) => l());
}

export function useWork(): Work {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => {
        listeners.delete(l);
      };
    },
    () => work,
  );
}

export const subagentTab = (owner: string, child: { id: string; title: string }): SideTab => ({ key: `subagent:${child.id}`, kind: "subagent", owner, sessionId: child.id, title: child.title || "Subagent" });
export const planTab = (owner: string): SideTab => ({ key: `plan:${owner}`, kind: "plan", owner, sessionId: owner, title: "Plan" });
export const outputTab = (owner: string, task: BackgroundTaskView): SideTab => ({ key: `output:${task.id}`, kind: "output", owner, sessionId: task.sessionId, taskId: task.id, title: task.command });

/** Open a tab, or make it the active one when it is open already. */
export function openTab(tab: SideTab) {
  set({ tabs: work.tabs.some((t) => t.key === tab.key) ? work.tabs : [...work.tabs, tab], active: { ...work.active, [tab.owner]: tab.key } });
}

export function closeTab(key: string) {
  const tab = work.tabs.find((t) => t.key === key);
  if (!tab) return;
  const tabs = work.tabs.filter((t) => t.key !== key);
  const mine = tabs.filter((t) => t.owner === tab.owner);
  const active = { ...work.active };
  if (active[tab.owner] === key) {
    if (mine.length > 0) active[tab.owner] = mine[mine.length - 1].key;
    else delete active[tab.owner];
  }
  set({ tabs, active });
}

export function activate(owner: string, key: string) {
  set({ active: { ...work.active, [owner]: key } });
}

/** The tab shown for `owner`, and its tabs. */
export function tabsOf(w: Work, owner: string | null): { tabs: SideTab[]; active: SideTab | null } {
  const tabs = owner ? w.tabs.filter((t) => t.owner === owner) : [];
  return { tabs, active: tabs.find((t) => t.key === w.active[owner ?? ""]) ?? null };
}

/** A session was deleted: its tabs and its tasks go. */
export function forgetSession(id: string) {
  const tasks = Object.fromEntries(Object.entries(work.tasks).filter(([key]) => key !== id));
  const tabs = work.tabs.filter((t) => t.owner !== id && t.sessionId !== id);
  const active = Object.fromEntries(Object.entries(work.active).filter(([owner]) => owner !== id && tabs.some((t) => t.key === work.active[owner])));
  set({ tabs, active, tasks });
}

/** The tasks of a session, read again (`GET /api/sessions/:id/background`). A refusal leaves the list as it was. */
export async function loadTasks(sessionId: string) {
  try {
    const r = await api.get<{ tasks: BackgroundTaskView[] }>(`/api/sessions/${sessionId}/background`);
    set({ tasks: { ...work.tasks, [sessionId]: r.tasks } });
  } catch {
    // keep the last list
  }
}

/** `backgroundTask` over `/ws`: the open session reads its tasks again, another one forgets them until they are needed. */
export function onBackgroundTask(sessionId: string) {
  if (getStore().activeSession === sessionId) {
    void loadTasks(sessionId);
  } else if (sessionId in work.tasks) {
    set({ tasks: Object.fromEntries(Object.entries(work.tasks).filter(([key]) => key !== sessionId)) });
  }
}

/** Stop a task (`POST /api/background/:taskId/stop`); a refusal is a notice. */
export async function stopTask(task: BackgroundTaskView) {
  try {
    await api.post(`/api/background/${task.id}/stop`, {});
  } catch (e) {
    notify(`Command not stopped: ${e instanceof Error ? e.message : String(e)}`);
  }
  await loadTasks(task.sessionId);
}

/** The first read of a child session for its tab: its messages and meta into the store, without opening it. */
export async function readSession(id: string) {
  try {
    const s = await api.get<{ meta: SessionInfo; messages: Message[] }>(`/api/sessions/${id}`);
    const store = getStore();
    patch({ messages: { ...store.messages, [id]: s.messages }, metas: { ...store.metas, [id]: s.meta } });
  } catch (e) {
    notify(`Subagent not read: ${e instanceof Error ? e.message : String(e)}`);
  }
}

/** A section is open when the owner opened it, or has something and was not folded by the owner. */
export function sectionOpen(w: Work, id: string, hasSomething: boolean): boolean {
  if (w.closed.includes(id)) return false;
  return hasSomething || w.opened.includes(id);
}

export function toggleSection(id: string, open: boolean, hasSomething: boolean) {
  const closed = open ? [...new Set([...work.closed, id])] : work.closed.filter((x) => x !== id);
  const opened = open || hasSomething ? work.opened.filter((x) => x !== id) : [...new Set([...work.opened, id])];
  try {
    localStorage.setItem(CLOSED_KEY, JSON.stringify(closed));
  } catch {
    // the page works without it
  }
  set({ closed, opened });
}

/** The tab the pane shows for the open session, in the chat; `null`: no pane. */
export function usePane(): SideTab | null {
  const s = useStore();
  const w = useWork();
  return s.view === "chat" ? tabsOf(w, s.activeSession).active : null;
}
