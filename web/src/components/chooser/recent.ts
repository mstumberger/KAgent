// The last models picked, kept in this browser (chooser.md section 3).
const KEY = "kagent.chooser.recent";
export const RECENT_MAX = 5;

export function readRecent(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "[]") as unknown;
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === "string").slice(0, RECENT_MAX) : [];
  } catch {
    return [];
  }
}

/** A model was picked: it goes first, the list keeps the last five. */
export function pushRecent(ref: string) {
  try {
    localStorage.setItem(KEY, JSON.stringify([ref, ...readRecent().filter((r) => r !== ref)].slice(0, RECENT_MAX)));
  } catch {
    // private window: the list is not kept
  }
}
