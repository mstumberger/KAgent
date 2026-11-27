// What this browser remembers of the sidebar (sidebar.md section 2), under one key: the
// list it shows, whether it is a rail, and the folders whose sections are closed.
export type SidebarView = "projects" | "recent";
export interface SidebarPrefs {
  view: SidebarView;
  collapsed: boolean;
  closed: string[];
}

export const SIDEBAR_KEY = "kagent.sidebar";
export const DEFAULTS: SidebarPrefs = { view: "projects", collapsed: false, closed: [] };

/** The stored preferences; the defaults when there are none or storage cannot be read. */
export function readPrefs(): SidebarPrefs {
  try {
    const raw = JSON.parse(localStorage.getItem(SIDEBAR_KEY) ?? "null") as Partial<SidebarPrefs> | null;
    return {
      view: raw?.view === "recent" ? "recent" : "projects",
      collapsed: raw?.collapsed === true,
      closed: Array.isArray(raw?.closed) ? raw.closed.filter((p): p is string => typeof p === "string") : [],
    };
  } catch {
    return { ...DEFAULTS };
  }
}

export function writePrefs(prefs: SidebarPrefs) {
  try {
    localStorage.setItem(SIDEBAR_KEY, JSON.stringify(prefs));
  } catch {
    // A private window: the choice lasts for this page only.
  }
}

/** Below this width the sidebar is a drawer, never inline (sidebar.md section 5.1). */
export const DRAWER_BELOW = 900;
