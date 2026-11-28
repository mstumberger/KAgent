// Main navigation (ui-settings.md section 1): the tabs of the header, in order. The
// daemon's allow-list (`crates/kagentd/src/ui.rs`, `VIEWS`) holds the same ids.

export const VIEWS = ["chat", "voice", "tasks", "tickets", "runs", "alerts", "changes", "security", "system", "settings"] as const;
export type View = (typeof VIEWS)[number];

/** Tabs that are settings pages since S19: `providers` is Models, `tools` is Tools and permissions. */
const FORMER_TABS: Record<string, string> = { providers: "models", tools: "tools" };

function isView(name: unknown): name is View {
  return typeof name === "string" && (VIEWS as readonly string[]).includes(name);
}

/**
 * Where a view name opens the UI: a tab, or the settings page of a former tab (`section`
 * wins there). `section` is kept for `settings` only. An unknown name gives `null`.
 */
export function resolveView(name: string, section?: string | null): { view: View; section: string | null } | null {
  const former = FORMER_TABS[name];
  if (former) return { view: "settings", section: section || former };
  if (!isView(name)) return null;
  return { view: name, section: name === "settings" && section ? section : null };
}
