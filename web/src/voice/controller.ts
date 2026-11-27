// One voice session for the whole UI: the Voice tab, the overlay, the tray and the
// agent's `ui` tool all drive this, so switching tabs no longer ends the call.
import { LiveClient, type LiveEvent, type LiveStatus } from "./live";

export interface Line {
  who: "user" | "assistant" | "tool";
  text: string;
}

type Listener = () => void;
const listeners = new Set<Listener>();
let client: LiveClient | null = null;
let sessionHook: ((id: string) => void) | null = null;

function readNum(key: string, dflt: number): number {
  const v = Number(localStorage.getItem(key));
  return Number.isFinite(v) && localStorage.getItem(key) !== null ? v : dflt;
}

export const voice = {
  status: "idle" as LiveStatus,
  detail: "",
  muted: false,
  volume: readNum("kagent.volume", 0.6),
  /** settings.voice.volume, 0..100: set by the owner or by the agent's `settings` tool (voice-control.md section 2). */
  settingsVolume: 100,
  lines: [] as Line[],
  /** What was said in the current turn, not yet stored in the session (shown live in chat). */
  pending: [] as Line[],
  /** The chat session this voice conversation belongs to. */
  sessionId: null as string | null,
  version: 0,
  /** A brain turn runs for what was just said (the `think` call of the live model); the status line says so. */
  thinking: false,

  get active(): boolean {
    return client?.active ?? false;
  },
  get micAnalyser(): AnalyserNode | null {
    return client?.micAnalyser ?? null;
  },
  get playAnalyser(): AnalyserNode | null {
    return client?.playAnalyser ?? null;
  },

  subscribe(l: Listener): () => void {
    listeners.add(l);
    return () => {
      listeners.delete(l);
    };
  },
  onSession(h: (id: string) => void) {
    sessionHook = h;
  },

  start(sessionId: string | null, voiceName?: string) {
    if (client?.active) return;
    // Same conversation: keep its transcript; another one: start clean.
    if (sessionId !== voice.sessionId) voice.lines = [];
    voice.sessionId = sessionId;
    voice.pending = [];
    voice.thinking = false;
    client = new LiveClient(handle);
    client.setVolume(voice.volume);
    client.setSettingsVolume(voice.settingsVolume);
    client.setMuted(voice.muted);
    void client.start(sessionId, voiceName);
    changed();
  },
  stop() {
    client?.stop();
  },
  toggle(sessionId: string | null, voiceName?: string) {
    if (client?.active) voice.stop();
    else voice.start(sessionId, voiceName);
  },
  sendText(text: string) {
    client?.sendText(text);
  },
  setMuted(m: boolean) {
    voice.muted = m;
    client?.setMuted(m);
    changed();
  },
  setSettingsVolume(pct: number) {
    voice.settingsVolume = Math.max(0, Math.min(100, Number.isFinite(pct) ? pct : 100));
    client?.setSettingsVolume(voice.settingsVolume);
    changed();
  },
  setVolume(v: number) {
    voice.volume = Math.max(0, Math.min(1, v));
    localStorage.setItem("kagent.volume", String(voice.volume));
    client?.setVolume(voice.volume);
    changed();
  },
};

function changed() {
  voice.version++;
  listeners.forEach((l) => l());
}

function handle(e: LiveEvent) {
  switch (e.type) {
    case "status":
      voice.status = e.status;
      voice.detail = e.detail ?? "";
      // A roll or a barge-in does not end the thinking; the end of the session does.
      if (e.status === "idle" || e.status === "error" || e.status === "connecting") voice.thinking = false;
      break;
    case "session":
      voice.sessionId = e.sessionId;
      sessionHook?.(e.sessionId);
      break;
    case "turnComplete":
      // The daemon stores the turn in the session now; chat shows it from there.
      voice.pending = [];
      break;
    case "transcript": {
      const last = voice.lines[voice.lines.length - 1];
      voice.lines =
        last && last.who === e.who ? [...voice.lines.slice(0, -1), { who: e.who, text: last.text + e.text }] : [...voice.lines, { who: e.who, text: e.text }];
      const lastP = voice.pending[voice.pending.length - 1];
      voice.pending =
        lastP && lastP.who === e.who ? [...voice.pending.slice(0, -1), { who: e.who, text: lastP.text + e.text }] : [...voice.pending, { who: e.who, text: e.text }];
      break;
    }
    case "tool":
      if (e.name === "think") {
        // From the moment the brain is asked until it has answered.
        voice.thinking = e.state === "started";
      } else {
        voice.lines = [...voice.lines, { who: "tool", text: `${e.name}: ${e.state}` }];
      }
      break;
  }
  changed();
}
