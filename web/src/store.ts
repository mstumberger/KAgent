// Minimal store with useSyncExternalStore (no Redux/Zustand): state is replaced
// wholesale by the reducer; views subscribe via useStore().

import { useSyncExternalStore } from "react";
import { api, postPlan, putProject, putTodo, requestCompaction, type ProjectView, type SessionMeta, type Message, type ApprovalRequest, type StateView, type TodoItem } from "./api";
import { skillsApi, type SkillView } from "./skillsApi";
import { builtinOf, projectArg } from "./commands";
import { readPrefs, type SidebarView } from "./components/sidebar/prefs";
import { voice } from "./voice/controller";
import { resolveView, type View } from "./nav";
import * as picker from "./picker";
import { agentsApi, type ChildView, type SessionInfo, type SessionRow } from "./settings/subagents/api";

export interface Store {
  /** With `agents`, how many subagents each session started (agents.md section 6). */
  sessions: SessionRow[];
  activeSession: string | null;
  messages: Record<string, Message[]>;
  streaming: Record<string, string>; // sessionId -> partial assistant text
  running: Set<string>;
  approvals: ApprovalRequest[];
  state: StateView | null;
  notice: string | null;
  view: View;
  /** The open settings page (`settings/registry.ts`); "" until the settings page decides. */
  settingsSection: string;
  reasoning: Record<string, string>; // sessionId -> thinking text of the running turn
  alertsTick: number; // bumps on alert events so views refetch
  changesTick: number;
  tasksTick: number; // bumps when the task list changes
  skills: SkillView[]; // the library, for the slash picker of the composer
  skillsTick: number; // bumps when the library changed, so Skills reads it again
  /** Text waiting in the chat composer ("New skill with the agent" in Settings > Skills). */
  composerDraft: string;
  /** Each session's todo list (plan-todo.md section 1): from the session's meta, then from `todoUpdated` and the answers of the panel. */
  todos: Record<string, TodoItem[]>;
  /** Each session's plan-mode flag (plan-todo.md section 2): from the session's meta, then from `planUpdated` and the answers of the plan route. */
  planning: Record<string, boolean>;
  /** Each session's subagents (agents.md section 6): from `subagentUpdated`, in the order they were started. */
  children: Record<string, ChildView[]>;
  /** The meta of each session opened here: `origin` and `parent` tell the session of a subagent (read-only). */
  metas: Record<string, SessionInfo>;
  /** The models a conversation may choose (picker.md section 5.6); `null` until the first read. */
  models: picker.PickerModel[] | null;
  /** The widget's one open sheet (picker.md section 5.5, sidebar.md section 5.8); S43 adds its own. */
  sheet: "models" | "sessions" | null;
  /** The sidebar (sidebar.md section 5): a rail or expanded, the drawer of a narrow window, the list it shows and the folders it closed. */
  sidebarCollapsed: boolean;
  sidebarDrawer: boolean;
  sidebarView: SidebarView;
  sidebarClosed: string[];
  /** The projects of `GET /api/projects` (read when "Attach folder" opens), with the roots they come from. */
  projects: ProjectView[] | null;
  projectRoots: string[];
}

const prefs = readPrefs();

let store: Store = {
  sessions: [],
  activeSession: null,
  messages: {},
  streaming: {},
  running: new Set(),
  approvals: [],
  state: null,
  notice: null,
  view: "chat",
  settingsSection: "",
  reasoning: {},
  alertsTick: 0,
  changesTick: 0,
  tasksTick: 0,
  skills: [],
  skillsTick: 0,
  composerDraft: "",
  todos: {},
  planning: {},
  children: {},
  metas: {},
  models: null,
  sheet: null,
  sidebarCollapsed: prefs.collapsed,
  sidebarDrawer: false,
  sidebarView: prefs.view,
  sidebarClosed: prefs.closed,
  projects: null,
  projectRoots: [],
};

const listeners = new Set<() => void>();

/** Replace part of the state; the other stores (components/sidebar/store.ts) write through it. */
export function patch(next: Partial<Store>) {
  store = { ...store, ...next };
  listeners.forEach((l) => l());
}

export function getStore(): Store {
  return store;
}

export function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useStore(): Store {
  return useSyncExternalStore(subscribe, getStore);
}

// ---------------------------------------------------------------- actions

export async function refreshState() {
  try {
    const state = await api.get<StateView>("/api/state");
    patch({ state, approvals: state.pendingApprovals, running: new Set(state.running) });
    voice.setSettingsVolume(state.settings.voice?.volume ?? 100);
  } catch (e) {
    patch({ notice: `state failed: ${String(e)}` });
  }
}

/** Put settings into the state: what `settingsChanged` carries, or the answer of a save. */
export function applySettings(settings: StateView["settings"]) {
  if (store.state) patch({ state: { ...store.state, settings } });
  voice.setSettingsVolume(settings.voice?.volume ?? 100);
}

export async function refreshSessions() {
  const { sessions } = await api.get<{ sessions: SessionRow[] }>("/api/sessions");
  patch({ sessions });
  if (!store.activeSession && sessions.length > 0) {
    void openSession(sessions[0].id);
  }
}

export async function openSession(id: string) {
  patch({ activeSession: id });
  const s = await api.get<{ meta: SessionInfo; messages: Message[] }>(`/api/sessions/${id}`);
  // The session's todo list comes with its meta (plan-todo.md section 1), as does what
  // tells the session of a subagent (agents.md section 6).
  patch({ messages: { ...store.messages, [id]: s.messages }, todos: { ...store.todos, [id]: s.meta.todo ?? [] }, planning: { ...store.planning, [id]: s.meta.planning ?? false }, metas: { ...store.metas, [id]: s.meta } });
}

/** A new session, in folder `project` when given (sidebar.md section 5.2); it opens in the chat. */
export async function newSession(project?: string): Promise<string> {
  // The last choice starts it (picker.md section 5.6); a model that is gone is forgotten, not sent.
  await refreshModels();
  const last = picker.last();
  const kept = last.model && store.models?.some((m) => m.ref === last.model);
  if (last.model && !kept) picker.remember({ model: null, reasoningLevel: null });
  const body = { ...(kept ? last : {}), ...(project ? { project } : {}) };
  const meta = await api.post<SessionMeta>("/api/sessions", body);
  patch({ sessions: [meta, ...store.sessions], activeSession: meta.id, messages: { ...store.messages, [meta.id]: [] }, todos: { ...store.todos, [meta.id]: [] }, planning: { ...store.planning, [meta.id]: false }, metas: { ...store.metas, [meta.id]: meta } });
  return meta.id;
}

/** The picker's models, read again when older than 10 s; a daemon that cannot answer leaves the last list. */
export async function refreshModels(force = false) {
  const models = await picker.list(force).catch(() => null);
  if (models) patch({ models });
}

/** A session's meta as the daemon has it: the open one and its row (`sessionMetaChanged`, the answer of the model route). */
export function applyMeta(id: string, meta: SessionMeta) {
  patch({ metas: id in store.metas ? { ...store.metas, [id]: { ...store.metas[id], ...meta } } : store.metas, sessions: store.sessions.map((row) => (row.id === id ? { ...row, ...meta } : row)) });
}

/** Choose a conversation's model and effort: the answer replaces its meta and is the last choice; a refusal is a notice, and thrown. */
export async function setSessionModel(id: string, body: Partial<picker.Choice>) {
  try {
    const meta = await picker.putModel(id, body);
    applyMeta(id, meta);
    picker.remember({ model: meta.model ?? null, reasoningLevel: meta.reasoningLevel ?? null });
  } catch (e) {
    patch({ notice: `Model not changed: ${e instanceof Error ? e.message : String(e)}` });
    throw e;
  }
}

export const openSheet = (sheet: NonNullable<Store["sheet"]>) => patch({ sheet });
export const closeSheet = () => patch({ sheet: null });

/** Send a session's todo list as the panel changed it (a tick, Clear): one PUT with the whole list; the answer is the list the daemon stored. A failure is a notice. */
export async function saveTodo(id: string, items: TodoItem[]) {
  try {
    const r = await putTodo(id, items);
    patch({ todos: { ...store.todos, [id]: r.items } });
  } catch (e) {
    patch({ notice: `Todo list not saved: ${e instanceof Error ? e.message : String(e)}` });
    throw e;
  }
}

export async function deleteSession(id: string) {
  await api.del(`/api/sessions/${id}`);
  const sessions = store.sessions.filter((s) => s.id !== id);
  // The daemon deleted the subagents of the session with it (agents.md section 2).
  const without = <T,>(map: Record<string, T>) => Object.fromEntries(Object.entries(map).filter(([key]) => key !== id));
  patch({ sessions, children: without(store.children), metas: without(store.metas), activeSession: store.activeSession === id ? (sessions[0]?.id ?? null) : store.activeSession });
  if (store.activeSession) void openSession(store.activeSession);
}

/** Put a session in or out of plan mode ("Cancel plan", plan-todo.md section 2): the answer is its flag. A failure is a notice. */
export async function setPlan(id: string, active: boolean) {
  try {
    const r = await postPlan(id, active);
    patch({ planning: { ...store.planning, [id]: r.planning } });
  } catch (e) {
    patch({ notice: `Plan not ${active ? "started" : "cancelled"}: ${e instanceof Error ? e.message : String(e)}` });
    throw e;
  }
}

/** Send a line of the composer. `/new` and `/compact` are commands of the UI (commands.ts): neither sends a message; `/plan` alone arms the Plan toggle (Chat). `/plan <request>` (plan-todo.md section 2) and a line sent with `plan` start the turn in plan mode: the prefix is not part of the message. */
export async function sendMessage(text: string, plan = false) {
  const line = text.trim();
  const builtin = builtinOf(line)?.name;
  if (builtin === "new") return newSession();
  if (builtin === "compact") return compactSession();
  if (builtin === "plan") return;
  const id = store.activeSession;
  if (!id || !line) return;
  // The session of a subagent is read-only (agents.md section 6); the daemon refuses too.
  if (store.metas[id]?.origin === "subagent") {
    patch({ notice: "The session of a subagent is read-only: its answer arrives in the session that started it." });
    return;
  }
  const folder = projectArg(line);
  if (folder !== null) return attachByCommand(id, folder);
  const prefixed = /^\/plan\s/.test(line);
  const body = prefixed ? { text: line.replace(/^\/plan\s+/, ""), plan: true } : plan ? { text, plan: true } : { text };
  await api.post(`/api/sessions/${id}/messages`, body);
}

/** `/project <path>` attaches a folder to the session, `/project` alone detaches it (sidebar.md section 5.5). The daemon canonicalizes the path; a refusal is a notice with its message. */
async function attachByCommand(id: string, path: string) {
  try {
    const meta = await putProject(id, path || null);
    applyMeta(id, meta);
    patch({ notice: meta.project ? `Project: ${meta.project}` : "Project detached" });
  } catch (e) {
    patch({ notice: e instanceof Error ? e.message : String(e) });
  }
}

/** `/compact` (compaction.md section 4): the daemon summarizes everything before the last user message of the active session. The result shows as a notice; nothing is sent as a message. */
export async function compactSession() {
  const id = store.activeSession;
  if (!id) return;
  try {
    const result = await requestCompaction(id);
    patch({ notice: `Compacted: ${result.keptMessages} recent messages kept, the rest is a summary.` });
  } catch (e) {
    patch({ notice: `Compaction failed: ${e instanceof Error ? e.message : String(e)}` });
  }
}

/** The skills library. A daemon that cannot answer leaves the last list in place. */
export async function refreshSkills() {
  try {
    patch({ skills: await skillsApi.list() });
  } catch {
    // keep the last list
  }
}

/** Put text into the chat composer and open the chat, in a new session when there is none.
 * An open chat hears it as a `kagent-composer` event; one that is opened by this takes
 * the text from the store when it mounts. */
export async function prefillComposer(text: string) {
  if (!store.activeSession) await newSession();
  patch({ view: "chat", composerDraft: text });
  window.dispatchEvent(new CustomEvent("kagent-composer", { detail: text }));
}

export function takeComposerDraft() {
  patch({ composerDraft: "" });
}

export async function cancelSession() {
  const id = store.activeSession;
  if (id) await api.post(`/api/sessions/${id}/cancel`);
}

/** Stop a subagent of `sessionId` (the Stop button of its Agents card): a running one is cancelled and a queued one never starts; the card follows the event. A failure is a notice. */
export async function stopSubagent(sessionId: string, childId: string) {
  try {
    await agentsApi.stop(sessionId, childId);
  } catch (e) {
    patch({ notice: `Subagent not stopped: ${e instanceof Error ? e.message : String(e)}` });
  }
}

/** Answer a card. `reason` is the text of the reason field of a plan card (plan-todo.md section 2). */
export async function decideApproval(requestId: string, decision: "allowOnce" | "allowSession" | "allowAlways" | "deny", reason?: string) {
  await api.post(`/api/approvals/${requestId}`, reason ? { decision, reason } : { decision });
  patch({ approvals: store.approvals.filter((a) => a.requestId !== requestId) });
}

export function setView(view: View) {
  patch({ view });
}

/** Open a view as a deep link or `ui show` names it (nav.ts `resolveView`). */
export function showView(name: string, section: string | null = null) {
  const target = resolveView(name, section);
  if (!target) return;
  patch(target.section ? { view: target.view, settingsSection: target.section } : { view: target.view });
}

/** Open a settings page. The page remembers it (localStorage `kagent.settings.section`). */
export function setSection(id: string) {
  patch({ settingsSection: id });
}
