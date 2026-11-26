// One way to save a setting (ui-settings.md section 5): PUT /api/settings with
// `{[sectionKey]: patch}`. The daemon answers with the whole settings and tells every
// UI in `settingsChanged`. Every caller of the hook shares one status, which
// `SectionPage` shows in the header of the open section.
import { useSyncExternalStore } from "react";
import { api, type Settings } from "../api";
import { applySettings } from "../store";

interface Note {
  text: string;
  failed: boolean;
}

interface SaveState {
  /** Saves in flight. */
  pending: number;
  /** Why the daemon refused the last save. */
  error: string | null;
  /** The result of the last provider action (`report`). */
  note: Note | null;
  /** When the last save was applied (0 = none). */
  savedAt: number;
}

let state: SaveState = { pending: 0, error: null, note: null, savedAt: 0 };
const listeners = new Set<() => void>();
/** Counts saves, so that a slow answer cannot overwrite a newer one in the store. */
let started = 0;

function set(patch: Partial<SaveState>) {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

/**
 * Save `patch` under the settings key `sectionKey`. Resolves with the daemon's answer
 * (the whole settings), or `undefined` when it refused: `error` then says why.
 */
export async function saveSettings(sectionKey: string, patch: unknown): Promise<Settings | undefined> {
  const mine = ++started;
  set({ pending: state.pending + 1, error: null });
  try {
    const settings = await api.put<Settings>("/api/settings", { [sectionKey]: patch });
    // The daemon also sends `settingsChanged`. This keeps the UI right while the event
    // stream is down, and only for the save started last.
    if (mine === started) applySettings(settings);
    set({ pending: state.pending - 1, note: null, savedAt: Date.now() });
    return settings;
  } catch (e) {
    set({ pending: state.pending - 1, error: e instanceof Error ? e.message : String(e) });
    return undefined;
  }
}

export function useSettingPatch() {
  const s = useSyncExternalStore(subscribe, () => state);
  return {
    save: saveSettings,
    busy: s.pending > 0,
    error: s.error,
    note: s.note,
    savedAt: s.savedAt,
    /** Show what a section did outside `save` (a provider added, a connection tested). */
    report(text: string, failed = false) {
      set({ note: { text, failed }, error: null });
    },
  };
}
