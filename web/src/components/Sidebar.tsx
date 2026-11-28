// The left sidebar (sidebar.md section 5): the navigation of the whole window. New session,
// the search, the switch between Projects and Recent, the list, and the views at the bottom.
// Expanded (240 px) or a rail (48 px) from 900 px of width; below it a drawer over the content.
import { useEffect, useRef, useState } from "react";
import { setSidebarView as setView, closeDrawer, createSession, showFromSidebar } from "./sidebar/store";
import { useNarrow } from "./sidebar/useNarrow";
import { VIEWS, type View } from "../nav";
import { useStore } from "../store";
import SessionList from "./sidebar/SessionList";
import Icon from "./Icon";
import { PermissionRow } from "./PermissionPicker";
import ThemeSwitch from "./ThemeSwitch";
import "./sidebar.css";
import "./sidebar-foot.css";
import { useOverlay } from "../router/overlays";

/** The views under the list, with their icons (`chat` is the list itself). */
const ICONS: Record<Exclude<View, "chat">, string> = { voice: "mic", tasks: "sliders", tickets: "list", runs: "bot", alerts: "bell", changes: "box", security: "shield", system: "monitor", settings: "gear" };
const LABEL = (view: string) => view[0].toUpperCase() + view.slice(1);

interface BodyProps {
  rail?: boolean;
  /** What a click on a view does; the widget's sheet expands the window first. */
  onView?: (view: View) => void;
  /** The permission mode and the theme above Settings (the sidebar, not the widget's sheet: its picker is in the widget). */
  foot?: boolean;
}

/** The content of the sidebar: the frame below and the widget's sheet both draw it. */
export function SidebarBody({ rail = false, onView = showFromSidebar, foot = false }: BodyProps) {
  const s = useStore();
  const [query, setQuery] = useState("");
  // The search leaves archived conversations out unless this filter is pressed (shell.md section 5).
  const [archived, setArchived] = useState(false);
  const views = VIEWS.filter((v): v is Exclude<View, "chat"> => v !== "chat");
  const alerts = s.state?.alertsOpen ?? 0;
  const item = (v: Exclude<View, "chat">) => (
    <button key={v} type="button" className={`sb-nav-item${s.view === v ? " active" : ""}`} aria-current={s.view === v ? "page" : undefined} title={rail ? LABEL(v) : undefined} aria-label={rail ? LABEL(v) : undefined} onClick={() => onView(v)}>
      <Icon name={ICONS[v]} size={15} />
      {!rail && <span className="sb-nav-label">{LABEL(v)}</span>}
      {v === "alerts" && alerts > 0 && <span className="sb-badge">{alerts}</span>}
    </button>
  );

  return (
    <>
      <div className="sb-top">
        {rail ? (
          <button type="button" className="sb-icon-btn" aria-label="New session" title="New session" onClick={() => void createSession()}>
            <Icon name="plus" size={16} />
          </button>
        ) : (
          <button type="button" className="primary sb-new" onClick={() => void createSession()}>
            <Icon name="plus" size={14} />
            New session
          </button>
        )}
      </div>
      {!rail && (
        <>
          <div className="sb-search">
            <Icon name="search" size={14} />
            <input
              type="search"
              placeholder="Search sessions"
              aria-label="Search sessions"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                // Escape clears the text; with none it goes on (it closes a drawer).
                if (e.key === "Escape" && query) {
                  e.preventDefault();
                  e.stopPropagation();
                  setQuery("");
                }
              }}
            />
            <button type="button" className="sb-filter" aria-pressed={archived} title="Include archived sessions in the search" onClick={() => setArchived(!archived)}>
              Archived
            </button>
          </div>
          <div className="sb-switch" role="group" aria-label="List">
            {(["projects", "recent"] as const).map((v) => (
              <button key={v} type="button" aria-pressed={s.sidebarView === v} onClick={() => setView(v)}>
                {LABEL(v)}
              </button>
            ))}
          </div>
          <SessionList query={query} archived={archived} />
        </>
      )}
      {rail && <div className="sb-fill" />}
      <nav className="sb-nav" aria-label="Views">
        {views.filter((v) => v !== "settings").map(item)}
        {foot && (
          <div className="sb-foot" role="group" aria-label="Permission and theme">
            <PermissionRow rail={rail} />
            <ThemeSwitch />
          </div>
        )}
        {item("settings")}
      </nav>
    </>
  );
}

function Drawer() {
  const box = useRef<HTMLElement>(null);
  useOverlay(true, closeDrawer);
  useEffect(() => {
    // The focus moves into the drawer, and comes back to the toggle when it closes.
    box.current?.querySelector<HTMLElement>("button, input")?.focus();
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      closeDrawer();
    };
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("keydown", key);
      document.getElementById("sidebar-toggle")?.focus();
    };
  }, []);
  return (
    <>
      <div className="sidebar-scrim" onClick={closeDrawer} />
      <aside ref={box} id="sidebar" className="sidebar drawer" aria-label="Sidebar">
        <SidebarBody foot />
      </aside>
    </>
  );
}

export default function Sidebar() {
  const s = useStore();
  const narrow = useNarrow();
  useEffect(() => {
    // A window that grew past 900 px has no drawer to leave open.
    if (!narrow && s.sidebarDrawer) closeDrawer();
  }, [narrow, s.sidebarDrawer]);
  if (narrow) return s.sidebarDrawer ? <Drawer /> : null;
  return (
    <aside id="sidebar" className={`sidebar inline${s.sidebarCollapsed ? " rail" : ""}`} aria-label="Sidebar">
      <SidebarBody rail={s.sidebarCollapsed} foot />
    </aside>
  );
}
