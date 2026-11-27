// Appearance: the theme. It lives in this browser (localStorage), not in the daemon.
// Auto follows the system's light or dark setting (theme.ts).
import { useSyncExternalStore } from "react";
import type { SectionProps } from "../registry";
import { GroupCard, Row, SectionPage } from "../ui";
import { isPref, type ThemePref } from "../../theme";

const CHOICES: { pref: ThemePref; label: string }[] = [
  { pref: "auto", label: "Auto" },
  { pref: "dark", label: "Dark" },
  { pref: "light", label: "Light" },
];

const readPref = (): ThemePref => {
  const v = document.documentElement.dataset.themePref;
  return isPref(v) ? v : "auto";
};

// App owns the theme: it writes `data-theme` and `data-theme-pref` when its state changes.
// The header button, the `ui` tool and this page all go through its `kagent-theme` event.
function subscribeTheme(notify: () => void) {
  const observer = new MutationObserver(() => notify());
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "data-theme-pref"] });
  return () => {
    observer.disconnect();
  };
}

export default function Appearance({ title, description }: SectionProps) {
  const pref = useSyncExternalStore(subscribeTheme, readPref, readPref);
  const pick = (p: ThemePref) => window.dispatchEvent(new CustomEvent("kagent-theme", { detail: p }));
  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Theme" description="The colours of every KAgent window. The button in the header switches between dark and light.">
        <Row label="Theme" hint="Auto follows the light or dark setting of your system. Dark for low light, light for bright rooms.">
          <div className="row" role="group" aria-label="Theme">
            {CHOICES.map((c) => (
              <button key={c.pref} className={c.pref === pref ? "primary" : ""} aria-pressed={c.pref === pref} onClick={() => pick(c.pref)}>
                {c.label}
              </button>
            ))}
          </div>
        </Row>
      </GroupCard>
    </SectionPage>
  );
}
