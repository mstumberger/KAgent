// The settings screen (ui-settings.md section 2): a menu of pages on the left, the
// open page on the right. Below 900 px the menu is a select at the top.
import { useEffect } from "react";
import Icon from "../components/Icon";
import { setSection, useStore } from "../store";
import { GROUPS, REMEMBERED, SECTIONS, currentSection } from "./registry";
import "./controls.css";
import "./settings.css";

export default function SettingsPage() {
  const s = useStore();
  const section = currentSection(s.settingsSection);
  const Page = section.component;

  // The page used last is the one a plain `#view=settings` opens next time.
  useEffect(() => {
    localStorage.setItem(REMEMBERED, section.id);
  }, [section.id]);
  // A plain `/settings` is the page used last: the address is filled in (a replace, not a new entry).
  useEffect(() => {
    if (!s.settingsSection) setSection(section.id);
  }, [s.settingsSection, section.id]);

  const groups = GROUPS.filter((g) => SECTIONS.some((x) => x.group === g));
  const item = (x: (typeof SECTIONS)[number]) => (
    <button
      key={x.id}
      className={`settings-nav-item${x.id === section.id ? " active" : ""}`}
      aria-current={x.id === section.id ? "page" : undefined}
      title={x.keywords.join(", ")}
      onClick={() => setSection(x.id)}
    >
      <Icon name={x.icon} size={15} />
      <span>{x.title}</span>
    </button>
  );

  return (
    <div className="settings" data-section={section.id}>
      <select className="settings-jump" aria-label="Settings section" value={section.id} onChange={(e) => setSection(e.target.value)}>
        {SECTIONS.map((x) => (
          <option key={x.id} value={x.id}>
            {x.title}
          </option>
        ))}
      </select>
      <nav className="settings-nav" aria-label="Settings sections">
        {groups.map((g) => (
          <div className="settings-nav-group" key={g}>
            <div className="settings-nav-heading">{g}</div>
            {SECTIONS.filter((x) => x.group === g).map(item)}
          </div>
        ))}
      </nav>
      <section className="settings-detail">
        {s.state ? <Page key={section.id} title={section.title} description={section.description} /> : <div className="page">loading...</div>}
      </section>
    </div>
  );
}
