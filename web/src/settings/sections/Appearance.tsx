// Appearance: the theme. It lives in this browser (localStorage), not in the daemon.
import { useSyncExternalStore } from "react";
import type { SectionProps } from "../registry";
import { GroupCard, Row, SectionPage } from "../ui";

type Theme = "dark" | "light";
const THEMES: Theme[] = ["dark", "light"];

const readTheme = (): Theme => (document.documentElement.dataset.theme === "light" ? "light" : "dark");

// App owns the theme: it writes `data-theme` when its state changes. The header button,
// the `ui` tool and this page all go through its `kagent-theme` event.
function subscribeTheme(notify: () => void) {
  const observer = new MutationObserver(() => notify());
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return () => {
    observer.disconnect();
  };
}

export default function Appearance({ title, description }: SectionProps) {
  const theme = useSyncExternalStore(subscribeTheme, readTheme, readTheme);
  const pick = (t: Theme) => window.dispatchEvent(new CustomEvent("kagent-theme", { detail: t }));
  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Theme" description="The colours of every KAgent window. The button in the header switches too.">
        <Row label="Theme" hint="Dark for low light, light for bright rooms.">
          <div className="row" role="group" aria-label="Theme">
            {THEMES.map((t) => (
              <button key={t} className={t === theme ? "primary" : ""} aria-pressed={t === theme} disabled={t === theme} onClick={() => pick(t)}>
                {t === "dark" ? "Dark" : "Light"}
              </button>
            ))}
          </div>
        </Row>
      </GroupCard>
    </SectionPage>
  );
}
