// Listen on start (voice.md section 14): the daemon opens the window with `listen=<marker>` in its
// address. The marker is the daemon's own, held in its memory, good for one use and two minutes: this page
// takes it out of the address at once (a reload never shows it again) and the daemon says whether it was
// the one it made. Only then does the session start. An address that merely holds a flag does nothing.
import { api } from "../api";
import { getStore } from "../store";
import { notify } from "../components/toasts";
import { voice } from "./controller";

const PARAM = "listen";

/** The marker in the address, taken out of it; null when there is none. */
export function takeMarker(): string | null {
  const q = new URLSearchParams(location.search);
  const marker = q.get(PARAM);
  if (marker === null) return null;
  q.delete(PARAM);
  const qs = q.toString().replace(/=(&|$)/g, "$1");
  history.replaceState(null, "", `${location.pathname}${qs ? `?${qs}` : ""}${location.hash}`);
  return marker;
}

/** Say once why a session that started by itself did not come up; never starts it again. */
function reportFailure() {
  const off = voice.subscribe(() => {
    if (voice.status === "error") {
      notify(`Listen on start: ${voice.detail || "the voice session could not start"}`);
    }
    if (voice.status === "live" || voice.status === "idle" || voice.status === "error") off();
  });
}

/** On load: present the marker of the address, and start the voice session when the daemon accepts it. */
export async function startIfAsked(): Promise<void> {
  const marker = takeMarker();
  if (!marker) return;
  try {
    const r = await api.post<{ start: boolean }>("/api/voice/listen-start", { marker });
    if (!r.start) return;
  } catch {
    return;
  }
  reportFailure();
  voice.start(getStore().activeSession, undefined);
}
