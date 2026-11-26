// The agent's eyes on its own UI: console errors, crashes and UI state go to the daemon
// (`POST /api/ui/report`), and `uiCommand` events from the `ui` tool or the tray are
// executed here (show a view, voice start/stop/mute/volume, theme, reload, snapshot).
// Approvals are deliberately not reachable from here.
import { api } from "./api";
import { events } from "./events";
import { getStore, openSession, showView, subscribe, type Store } from "./store";
import { currentSection } from "./settings/registry";
import { voice } from "./voice/controller";

interface LogEntry {
  level: "error" | "warn" | "crash" | "log";
  message: string;
  stack?: string;
  at: string;
}

const queue: LogEntry[] = [];
let installed = false;
let stateDirty = true;
let lastSent = 0;
const TAB = Math.random().toString(36).slice(2, 10);

function text(x: unknown): string {
  if (x instanceof Error) return `${x.name}: ${x.message}`;
  if (typeof x === "string") return x;
  try {
    return JSON.stringify(x);
  } catch {
    return String(x);
  }
}

export function logUi(level: LogEntry["level"], message: string, stack?: string) {
  queue.push({ level, message: message.slice(0, 4000), stack: stack?.slice(0, 4000), at: new Date().toISOString() });
  if (queue.length > 200) queue.splice(0, queue.length - 200);
}

function uiState() {
  const s: Store = getStore();
  return {
    tab: TAB,
    view: s.view,
    // The open settings page (ui-settings.md section 2); only while settings is open.
    section: s.view === "settings" ? currentSection(s.settingsSection).id : undefined,
    activeSession: s.activeSession,
    running: [...s.running],
    approvalsPending: s.state?.pendingApprovals?.length ?? s.approvals.length,
    voice: { status: voice.status, detail: voice.detail, muted: voice.muted, volume: voice.volume, transcriptTail: voice.lines.slice(-6) },
    theme: document.documentElement.dataset.theme,
    // compact = the small widget layout of KAgent's own window (app).
    compact: new URLSearchParams(location.search).has("compact") || new URLSearchParams(location.search).has("overlay"),
    app: new URLSearchParams(location.search).has("app"),
    window: { width: innerWidth, height: innerHeight, visible: document.visibilityState },
    errorsSeen: errorsSeen,
  };
}

let errorsSeen = 0;

async function flush() {
  // Heartbeat every 10 s so the daemon (tray, ui tool) knows the tab is open.
  if (!queue.length && !stateDirty && Date.now() - lastSent < 10_000) return;
  lastSent = Date.now();
  const logs = queue.splice(0, queue.length);
  stateDirty = false;
  try {
    await api.post("/api/ui/report", { logs, state: uiState() });
  } catch {
    queue.unshift(...logs.slice(-50)); // daemon down: keep the latest for later
  }
}

function snapshot(requestId: string) {
  const top = document.querySelector(".widget-top, header");
  const body = {
    view: getStore().view,
    url: location.pathname + location.search,
    text: document.body.innerText.slice(0, 12000),
    // Layout of the title bar for diagnosing missing controls.
    titlebar: top ? [...top.children].map((c) => `${c.tagName.toLowerCase()}.${c.className} ${Math.round(c.getBoundingClientRect().x)}+${Math.round(c.getBoundingClientRect().width)}`).join(" | ") : "",
    viewport: `${innerWidth}x${innerHeight} dpr ${devicePixelRatio}`,
  };
  void api.post(`/api/ui/snapshot/${encodeURIComponent(requestId)}`, body).catch(() => {});
}

function run(command: string, args: Record<string, unknown>) {
  const isOverlay = new URLSearchParams(location.search).has("overlay");
  const liveVoice = (getStore().state?.settings.liveVoice as string) || undefined;
  switch (command) {
    case "snapshot":
      snapshot(String(args.requestId ?? ""));
      break;
    case "show": {
      // `providers` and `tools` open settings pages; `section` names the page (S19).
      showView(String(args.view ?? ""), typeof args.section === "string" ? args.section : null);
      if (args.session) void openSession(String(args.session));
      window.focus();
      break;
    }
    case "voice_start": {
      // Only one tab may own the microphone: KAgent's own window when targeted (even
      // minimized to the tray), else a visible tab.
      const isApp = new URLSearchParams(location.search).has("app");
      if (args.target === "app" ? isApp : isApp || isOverlay || !document.hidden) voice.start(getStore().activeSession, liveVoice);
      break;
    }
    case "layout":
      // Only KAgent's own window changes layout, never a normal browser tab.
      if (new URLSearchParams(location.search).has("app")) window.dispatchEvent(new CustomEvent("kagent-layout", { detail: args.mode }));
      break;
    case "voice_stop":
      voice.stop();
      break;
    case "mute":
      voice.setMuted(true);
      break;
    case "unmute":
      voice.setMuted(false);
      break;
    case "mute_toggle":
      voice.setMuted(!voice.muted);
      break;
    case "volume":
      voice.setVolume(Number(args.value ?? voice.volume));
      break;
    case "theme":
      window.dispatchEvent(new CustomEvent("kagent-theme", { detail: args.value }));
      break;
    case "orb":
      localStorage.setItem("kagent.orb", String(args.value));
      window.dispatchEvent(new Event("kagent-orb"));
      break;
    case "reload":
      location.reload();
      break;
  }
  stateDirty = true;
}

export function installDiagnostics() {
  if (installed) return;
  installed = true;
  const orig = { error: console.error.bind(console), warn: console.warn.bind(console) };
  console.error = (...a: unknown[]) => {
    errorsSeen++;
    const err = a.find((x) => x instanceof Error) as Error | undefined;
    logUi("error", a.map(text).join(" "), err?.stack);
    orig.error(...a);
  };
  console.warn = (...a: unknown[]) => {
    logUi("warn", a.map(text).join(" "));
    orig.warn(...a);
  };
  window.addEventListener("error", (e) => {
    errorsSeen++;
    logUi("error", e.message || text(e.error), (e.error as Error | undefined)?.stack);
  });
  window.addEventListener("unhandledrejection", (e) => {
    errorsSeen++;
    logUi("error", `unhandled promise rejection: ${text(e.reason)}`, (e.reason as Error | undefined)?.stack);
  });
  subscribe(() => (stateDirty = true));
  voice.subscribe(() => (stateDirty = true));
  events.on((ev) => {
    if (ev.type === "uiCommand") run(ev.command, (ev.args ?? {}) as Record<string, unknown>);
  });
  setInterval(() => void flush(), 2000);
  addEventListener("pagehide", () => void flush());
}
