// The events of the daemon, applied to the store, and the start of the window. Split from
// getStore().ts, which holds the state and its actions.
import { api, initialSection, initialSession, initialView, type StateView } from "./api";
import { events, type AgentEvent } from "./events";
import { notify, pushNotice } from "./components/toasts";
import { refreshWatch } from "./screenWatch";
import { readUsage } from "./usageLine";
import { onBackgroundTask } from "./components/work/state";
import { applyMeta, applySettings, getStore, openSession, patch, refreshModels, refreshSessions, refreshSkills, refreshState, showView } from "./store";

const set = patch;

export function handleEvent(ev: AgentEvent) {
  switch (ev.type) {
    case "messageAppended": {
      const list = getStore().messages[ev.sessionId] ?? [];
      set({
        messages: { ...getStore().messages, [ev.sessionId]: [...list, ev.message] },
        streaming: { ...getStore().streaming, [ev.sessionId]: "" },
      });
      break;
    }
    case "reasoningDelta": {
      set({ reasoning: { ...getStore().reasoning, [ev.sessionId]: (getStore().reasoning[ev.sessionId] ?? "") + ev.text } });
      break;
    }
    case "alertRaised":
    case "alertUpdated":
      set({ alertsTick: getStore().alertsTick + 1 });
      void refreshState();
      break;
    case "ticketChanged":
      set({ ticketsTick: getStore().ticketsTick + 1 });
      break;
    case "runUpdated":
      set({ runsTick: getStore().runsTick + 1 });
      break;
    case "changeRecorded":
      set({ changesTick: getStore().changesTick + 1 });
      break;
    case "textDelta": {
      set({ streaming: { ...getStore().streaming, [ev.sessionId]: (getStore().streaming[ev.sessionId] ?? "") + ev.text } });
      break;
    }
    case "turnStarted": {
      const running = new Set(getStore().running);
      running.add(ev.sessionId);
      set({ running });
      break;
    }
    case "turnFinished": {
      const running = new Set(getStore().running);
      running.delete(ev.sessionId);
      set({ running, streaming: { ...getStore().streaming, [ev.sessionId]: "" }, reasoning: { ...getStore().reasoning, [ev.sessionId]: "" } });
      // A failed turn otherwise leaves the chat silent.
      if (ev.status === "failed" && ev.error) notify(`Turn failed: ${ev.error.slice(0, 300)}`, "error", ev.sessionId);
      // Reload the finished history of the open chat. Another session's turn (a subagent's,
      // a task's) ends without opening that session here.
      if (ev.sessionId === getStore().activeSession) void openSession(ev.sessionId);
      void refreshSessions();
      break;
    }
    case "approvalRequested": {
      void refreshState();
      break;
    }
    case "approvalResolved": {
      set({ approvals: getStore().approvals.filter((a) => a.requestId !== ev.requestId) });
      void refreshState();
      break;
    }
    case "sessionUpdated":
      void refreshSessions();
      break;
    case "todoUpdated":
      // The agent wrote the list, or the daemon stored what the panel sent.
      set({ todos: { ...getStore().todos, [ev.sessionId]: ev.items } });
      break;
    case "planUpdated":
      // The session entered or left plan mode (the agent, an approval, "Cancel plan").
      set({ planning: { ...getStore().planning, [ev.sessionId]: ev.active } });
      break;
    case "sessionMetaChanged":
      applyMeta(ev.sessionId, ev.meta);
      break;
    case "subagentUpdated": {
      // A subagent of the session started, got a slot, ended or was stopped (agents.md section 3).
      const mine = getStore().children[ev.sessionId] ?? [];
      const child = { ...ev.child, status: ev.status };
      set({ children: { ...getStore().children, [ev.sessionId]: mine.some((c) => c.id === child.id) ? mine.map((c) => (c.id === child.id ? child : c)) : [...mine, child] } });
      break;
    }
    case "backgroundTask":
      onBackgroundTask(ev.sessionId);
      break;
    case "settingsChanged":
      // The event carries the whole settings (the answer of PUT /api/settings): no
      // second GET /api/state.
      if (getStore().state && ev.settings && typeof ev.settings === "object") applySettings(ev.settings as StateView["settings"]);
      else void refreshState();
      break;
    case "screenWatchChanged":
      // Only the daemon says whether a watch runs: any state but `watching` is the end.
      set({ watch: ev.watch && ev.watch.state === "watching" ? ev.watch : null });
      break;
    case "notice":
      pushNotice(ev);
      break;
    case "tasksChanged":
    case "taskRun":
      // A task was added, changed, claimed or finished (automations.md section 3): Tasks reads the list again.
      set({ tasksTick: getStore().tasksTick + 1 });
      break;
    case "usageChanged":
      // The limits or the counters of a provider moved (usage.md section 4): the rings and the cards read again.
      void readUsage();
      break;
    case "skillsChanged":
      // A skill was written, removed or switched: Skills reads the list again.
      set({ skillsTick: getStore().skillsTick + 1 });
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
  void refreshWatch();
  void refreshSkills();
  void refreshModels(true);
  void readUsage();
  void refreshSessions().then(() => {
    if (initialSession) {
      void openSession(initialSession);
    } else if (!getStore().activeSession && getStore().sessions.length > 0) {
      // Continue the most recent conversation (voice too) instead of starting a new one.
      const latest = [...getStore().sessions].sort((a, b) => String(b.updatedAt).localeCompare(String(a.updatedAt)))[0];
      void openSession(latest.id);
    }
  });
}
