import { useSyncExternalStore } from "react";
import { DRAWER_BELOW } from "./prefs";
import { isNarrow } from "./store";

function subscribeNarrow(notify: () => void) {
  if (typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(`(max-width: ${DRAWER_BELOW - 1}px)`);
  media.addEventListener("change", notify);
  return () => media.removeEventListener("change", notify);
}

/** Under 900 px of viewport width: the sidebar is a drawer (sidebar.md section 5.1). */
export function useNarrow(): boolean {
  return useSyncExternalStore(subscribeNarrow, isNarrow, () => false);
}
