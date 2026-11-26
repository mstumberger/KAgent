// Minimal store with useSyncExternalStore (no Redux/Zustand): state is replaced
// wholesale by the reducer; views subscribe via useStore().

import { useSyncExternalStore } from "react";
import { api, initialSection, initialSession, initialView, postPlan, putTodo, requestCompaction, skillsApi, type SessionMeta, type Message, type ApprovalRequest, type SkillView, type StateView, type TodoItem } from "./api";
import { builtinOf } from "./commands";
import { events, type AgentEvent } from "./events";
import { resolveView, type View } from "./nav";
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
}

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
};

const listeners = new Set<() => void>();

function set(patch: Partial<Store>) {
  store = { ...store, ...patch };
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
    set({ state, approvals: state.pendingApprovals, running: new Set(state.running) });
  } catch (e) {
    set({ notice: `state failed: ${String(e)}` });
  }
}

/** Put settings into the state: what `settingsChanged` carries, or the answer of a save. */
export function applySettings(settings: StateView["settings"]) {
  if (store.state) set({ state: { ...store.state, settings } });
}

export async function refreshSessions() {
  const { sessions } = await api.get<{ sessions: SessionRow[] }>("/api/sessions");
  set({ sessions });
  if (!store.activeSession && sessions.length > 0) {
    void openSession(sessions[0].id);
  }
}

export async function openSession(id: string) {
  set({ activeSession: id });
  const s = await api.get<{ meta: SessionInfo; messages: Message[] }>(`/api/sessions/${id}`);
  // The session's todo list comes with its meta (plan-todo.md section 1), as does what
  // tells the session of a subagent (agents.md section 6).
  set({ messages: { ...store.messages, [id]: s.messages }, todos: { ...store.todos, [id]: s.meta.todo ?? [] }, planning: { ...store.planning, [id]: s.meta.planning ?? false }, metas: { ...store.metas, [id]: s.meta } });
}

export async function newSession(): Promise<string> {
  const meta = await api.post<SessionMeta>("/api/sessions", {});
  set({ sessions: [meta, ...store.sessions], activeSession: meta.id, messages: { ...store.messages, [meta.id]: [] }, todos: { ...store.todos, [meta.id]: [] }, planning: { ...store.planning, [meta.id]: false }, metas: { ...store.metas, [meta.id]: meta } });
  return meta.id;
}

/** Send a session's todo list as the panel changed it (a tick, Clear): one PUT with the whole list; the answer is the list the daemon stored. A failure is a notice. */
export async function saveTodo(id: string, items: TodoItem[]) {
  try {
    const r = await putTodo(id, items);
    set({ todos: { ...store.todos, [id]: r.items } });
  } catch (e) {
    set({ notice: `Todo list not saved: ${e instanceof Error ? e.message : String(e)}` });
    throw e;
  }
}

export async function deleteSession(id: string) {
  await api.del(`/api/sessions/${id}`);
  const sessions = store.sessions.filter((s) => s.id !== id);
  // The daemon deleted the subagents of the session with it (agents.md section 2).
  const without = <T,>(map: Record<string, T>) => Object.fromEntries(Object.entries(map).filter(([key]) => key !== id));
  set({ sessions, children: without(store.children), metas: without(store.metas), activeSession: store.activeSession === id ? (sessions[0]?.id ?? null) : store.activeSession });
  if (store.activeSession) void openSession(store.activeSession);
}

/** Put a session in or out of plan mode ("Cancel plan", plan-todo.md section 2): the answer is its flag. A failure is a notice. */
export async function setPlan(id: string, active: boolean) {
  try {
    const r = await postPlan(id, active);
    set({ planning: { ...store.planning, [id]: r.planning } });
  } catch (e) {
    set({ notice: `Plan not ${active ? "started" : "cancelled"}: ${e instanceof Error ? e.message : String(e)}` });
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
    set({ notice: "The session of a subagent is read-only: its answer arrives in the session that started it." });
    return;
  }
  const prefixed = /^\/plan\s/.test(line);
  const body = prefixed ? { text: line.replace(/^\/plan\s+/, ""), plan: true } : plan ? { text, plan: true } : { text };
  await api.post(`/api/sessions/${id}/messages`, body);
}

/** `/compact` (compaction.md section 4): the daemon summarizes everything before the last user message of the active session. The result shows as a notice; nothing is sent as a message. */
export async function compactSession() {
  const id = store.activeSession;
  if (!id) return;
  try {
    const result = await requestCompaction(id);
    set({ notice: `Compacted: ${result.keptMessages} recent messages kept, the rest is a summary.` });
  } catch (e) {
    set({ notice: `Compaction failed: ${e instanceof Error ? e.message : String(e)}` });
  }
}

/** The skills library. A daemon that cannot answer leaves the last list in place. */
export async function refreshSkills() {
  try {
    set({ skills: await skillsApi.list() });
  } catch {
    // keep the last list
  }
}

/** Put text into the chat composer and open the chat, in a new session when there is none.
 * An open chat hears it as a `kagent-composer` event; one that is opened by this takes
 * the text from the store when it mounts. */
export async function prefillComposer(text: string) {
  if (!store.activeSession) await newSession();
  set({ view: "chat", composerDraft: text });
  window.dispatchEvent(new CustomEvent("kagent-composer", { detail: text }));
}

export function takeComposerDraft() {
  set({ composerDraft: "" });
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
    set({ notice: `Subagent not stopped: ${e instanceof Error ? e.message : String(e)}` });
  }
}

/** Answer a card. `reason` is the text of the reason field of a plan card (plan-todo.md section 2). */
export async function decideApproval(requestId: string, decision: "allowOnce" | "allowSession" | "allowAlways" | "deny", reason?: string) {
  await api.post(`/api/approvals/${requestId}`, reason ? { decision, reason } : { decision });
  set({ approvals: store.approvals.filter((a) => a.requestId !== requestId) });
}

export function setView(view: View) {
  set({ view });
}

/** Open a view as a deep link or `ui show` names it (nav.ts `resolveView`). */
export function showView(name: string, section: string | null = null) {
  const target = resolveView(name, section);
  if (!target) return;
  set(target.section ? { view: target.view, settingsSection: target.section } : { view: target.view });
}

/** Open a settings page. The page remembers it (localStorage `kagent.settings.section`). */
export function setSection(id: string) {
  set({ settingsSection: id });
}

// ---------------------------------------------------------------- event wiring

export function handleEvent(ev: AgentEvent) {
  switch (ev.type) {
    case "messageAppended": {
      const list = store.messages[ev.sessionId] ?? [];
      set({
        messages: { ...store.messages, [ev.sessionId]: [...list, ev.message] },
        streaming: { ...store.streaming, [ev.sessionId]: "" },
      });
      break;
    }
    case "reasoningDelta": {
      set({ reasoning: { ...store.reasoning, [ev.sessionId]: (store.reasoning[ev.sessionId] ?? "") + ev.text } });
      break;
    }
    case "alertRaised":
    case "alertUpdated":
      set({ alertsTick: store.alertsTick + 1 });
      void refreshState();
      break;
    case "changeRecorded":
      set({ changesTick: store.changesTick + 1 });
      break;
    case "textDelta": {
      set({ streaming: { ...store.streaming, [ev.sessionId]: (store.streaming[ev.sessionId] ?? "") + ev.text } });
      break;
    }
    case "turnStarted": {
      const running = new Set(store.running);
      running.add(ev.sessionId);
      set({ running });
      break;
    }
    case "turnFinished": {
      const running = new Set(store.running);
      running.delete(ev.sessionId);
      set({ running, streaming: { ...store.streaming, [ev.sessionId]: "" }, reasoning: { ...store.reasoning, [ev.sessionId]: "" } });
      // A failed turn otherwise leaves the chat silent.
      if (ev.status === "failed" && ev.error) set({ notice: `Turn failed: ${ev.error.slice(0, 300)}` });
      // Reload the finished history of the open chat. Another session's turn (a subagent's,
      // a task's) ends without opening that session here.
      if (ev.sessionId === store.activeSession) void openSession(ev.sessionId);
      void refreshSessions();
      break;
    }
    case "approvalRequested": {
      void refreshState();
      break;
    }
    case "approvalResolved": {
      set({ approvals: store.approvals.filter((a) => a.requestId !== ev.requestId) });
      void refreshState();
      break;
    }
    case "sessionUpdated":
      void refreshSessions();
      break;
    case "todoUpdated":
      // The agent wrote the list, or the daemon stored what the panel sent.
      set({ todos: { ...store.todos, [ev.sessionId]: ev.items } });
      break;
    case "planUpdated":
      // The session entered or left plan mode (the agent, an approval, "Cancel plan").
      set({ planning: { ...store.planning, [ev.sessionId]: ev.active } });
      break;
    case "subagentUpdated": {
      // A subagent of the session started, got a slot, ended or was stopped (agents.md section 3).
      const mine = store.children[ev.sessionId] ?? [];
      const child = { ...ev.child, status: ev.status };
      set({ children: { ...store.children, [ev.sessionId]: mine.some((c) => c.id === child.id) ? mine.map((c) => (c.id === child.id ? child : c)) : [...mine, child] } });
      break;
    }
    case "settingsChanged":
      // The event carries the whole settings (the answer of PUT /api/settings): no
      // second GET /api/state.
      if (store.state && ev.settings && typeof ev.settings === "object") applySettings(ev.settings as StateView["settings"]);
      else void refreshState();
      break;
    case "notice":
      set({ notice: ev.text });
      break;
    case "tasksChanged":
    case "taskRun":
      // A task was added, changed, claimed or finished (automations.md section 3): Tasks reads the list again.
      set({ tasksTick: store.tasksTick + 1 });
      break;
    case "skillsChanged":
      // A skill was written, removed or switched: Skills reads the list again.
      set({ skillsTick: store.skillsTick + 1 });
      void refreshSkills();
      break;
    default:
      break;
  }
}

export function init() {
  if (initialView) showView(initialView, initialSection);
  events.on(handleEvent);
  events.start(api.wsUrl);
  void refreshState();
  void refreshSkills();
  void refreshSessions().then(() => {
    if (initialSession) {
      void openSession(initialSession);
    } else if (!store.activeSession && store.sessions.length > 0) {
      // Continue the most recent conversation (voice too) instead of starting a new one.
      const latest = [...store.sessions].sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))[0];
      void openSession(latest.id);
    }
  });
}
