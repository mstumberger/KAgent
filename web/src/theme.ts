// The theme (ui-settings.md section 3, Appearance): the owner's choice, kept in this
// browser, is `auto` (follow the system's light or dark setting; the default of a new
// install), `dark` or `light`. The page always carries the theme in use as `data-theme`
// (dark or light), so the styles and the widget frames need nothing else; the choice is
// `data-theme-pref`.
export type ThemePref = "auto" | "dark" | "light";
export type Theme = "dark" | "light";

export const THEME_KEY = "kagent.theme";
const QUERY = "(prefers-color-scheme: light)";

export const isPref = (v: unknown): v is ThemePref => v === "auto" || v === "dark" || v === "light";

/** The stored choice; `auto` when there is none or storage cannot be read. */
export function readPref(): ThemePref {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return isPref(v) ? v : "auto";
  } catch {
    return "auto";
  }
}

export function storePref(pref: ThemePref) {
  try {
    localStorage.setItem(THEME_KEY, pref);
  } catch {
    // A private window: the choice lasts for this page only.
  }
}

/** The system's setting; dark when the browser cannot say. */
export function systemTheme(): Theme {
  return typeof window.matchMedia === "function" && window.matchMedia(QUERY).matches ? "light" : "dark";
}

/** For `useSyncExternalStore`: a change of the system's setting. */
export function subscribeSystemTheme(notify: () => void) {
  if (typeof window.matchMedia !== "function") return () => {};
  const media = window.matchMedia(QUERY);
  media.addEventListener("change", notify);
  return () => {
    media.removeEventListener("change", notify);
  };
}

export const themeOf = (pref: ThemePref, system: Theme): Theme => (pref === "auto" ? system : pref);
