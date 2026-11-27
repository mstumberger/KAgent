// Appearance: the theme. It lives in this browser (localStorage), not in the daemon.
// Auto follows the system's light or dark setting (theme.ts).
import { useSyncExternalStore } from "react";
import type { SectionProps } from "../registry";
import { GroupCard, Row, SectionPage } from "../ui";
import { readApplied, requestPref, subscribeApplied, type ThemePref } from "../../theme";

const CHOICES: { pref: ThemePref; label: string }[] = [
  { pref: "auto", label: "Auto" },
  { pref: "dark", label: "Dark" },
  { pref: "light", label: "Light" },
];

export default function Appearance({ title, description }: SectionProps) {
  // App owns the theme: the sidebar's switch, the `ui` tool and this page go through its `kagent-theme` event.
  const pref = useSyncExternalStore(subscribeApplied, readApplied, readApplied);
  const pick = requestPref;
  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Theme" description="The colours of every KAgent window. The button at the foot of the sidebar goes through light, dark and auto.">
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
