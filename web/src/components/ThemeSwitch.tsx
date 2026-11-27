// The theme switch at the foot of the sidebar (sidebar.md, amendment of 2026-11-27): one icon
// button that goes through light, dark and auto. The icon is the choice in use.
import { useSyncExternalStore } from "react";
import { readApplied, requestPref, subscribeApplied, type ThemePref } from "../theme";
import Icon from "./Icon";

const ORDER: ThemePref[] = ["light", "dark", "auto"];
const ICON: Record<ThemePref, string> = { light: "sun", dark: "moon", auto: "contrast" };

export default function ThemeSwitch() {
  const pref = useSyncExternalStore(subscribeApplied, readApplied, readApplied);
  const next = ORDER[(ORDER.indexOf(pref) + 1) % ORDER.length];
  return (
    <button type="button" className="sb-icon-btn sb-theme" title={`Theme: ${pref}. Click for ${next}`} aria-label={`Theme: ${pref}. Switch to ${next}`} onClick={() => requestPref(next)}>
      <Icon name={ICON[pref]} size={15} />
    </button>
  );
}
