// Minimal store with useSyncExternalStore (no Redux/Zustand): state is replaced
// wholesale by the reducer; views subscribe via useStore().

import { useSyncExternalStore } from "react";
import { api, initialSession, initialView, type SessionMeta, type Message, type ApprovalRequest, type StateView } from "./api";
import { events, type AgentEvent } from "./events";

export interface Store {
  sessions: SessionMeta[];
  activeSession: string | null;
  messages: Record<string, Message[]>;
  streaming: Record<string, string>; // sessionId -> partial assistant text
  running: Set<string>;
  approvals: ApprovalRequest[];
  state: StateView | null;
  notice: string | null;
  view: "chat" | "voice" | "tasks" | "alerts" | "changes" | "security" | "system" | "providers" | "tools" | "settings";
  reasoning: Record<string, string>; // sessionId -> thinking text of the running turn
  alertsTick: number; // bumps on alert events so views refetch
  changesTick: number;
  tasksTick: number; // bumps when the task list changes
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
  reasoning: {},
  alertsTick: 0,
  changesTick: 0,
  tasksTick: 0,
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

export async function refreshSessions() {
  const { sessions } = await api.get<{ sessions: SessionMeta[] }>("/api/sessions");
  set({ sessions });
  if (!store.activeSession && sessions.length > 0) {
    void openSession(sessions[0].id);
  }
}

export async function openSession(id: string) {
  set({ activeSession: id });
  const s = await api.get<{ meta: SessionMeta; messages: Message[] }>(`/api/sessions/${id}`);
  set({ messages: { ...store.messages, [id]: s.messages } });
}

export async function newSession(): Promise<string> {
  const meta = await api.post<SessionMeta>("/api/sessions", {});
  set({ sessions: [meta, ...store.sessions], activeSession: meta.id, messages: { ...store.messages, [meta.id]: [] } });
  return meta.id;
}

export async function deleteSession(id: string) {
  await api.del(`/api/sessions/${id}`);
  const sessions = store.sessions.filter((s) => s.id !== id);
  set({ sessions, activeSession: store.activeSession === id ? (sessions[0]?.id ?? null) : store.activeSession });
  if (store.activeSession) void openSession(store.activeSession);
}

export async function sendMessage(text: string) {
  const id = store.activeSession;
  if (!id || !text.trim()) return;
  await api.post(`/api/sessions/${id}/messages`, { text });
}

export async function cancelSession() {
  const id = store.activeSession;
  if (id) await api.post(`/api/sessions/${id}/cancel`);
}

export async function decideApproval(requestId: string, decision: "allowOnce" | "allowSession" | "allowAlways" | "deny") {
  await api.post(`/api/approvals/${requestId}`, { decision });
  set({ approvals: store.approvals.filter((a) => a.requestId !== requestId) });
}

export function setView(view: Store["view"]) {
  set({ view });
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
      void openSession(ev.sessionId); // reload the finished history
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
    case "settingsChanged":
      void refreshState();
      break;
    case "notice":
      if (ev.level === "tasks") set({ tasksTick: store.tasksTick + 1 });
      else set({ notice: ev.text });
      break;
    default:
      break;
  }
}

export function init() {
  if (initialView) set({ view: initialView as Store["view"] });
  events.on(handleEvent);
  events.start(api.wsUrl);
  void refreshState();
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
