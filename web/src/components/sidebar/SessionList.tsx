// The list of the sidebar (sidebar.md section 5.3): Projects or Recent, or the flat result of
// a search. A heading is a button; Up and Down move the focus among the headings and rows.
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import { useStore } from "../../store";
import Icon from "../Icon";
import SessionRow from "./SessionRow";
import { projectSections, recentSections, searchSessions, type Section } from "./sections";
import { createSession, toggleFolder } from "./store";

function Heading({ section }: { section: Section }) {
  const count = section.kind === "pinned" || section.kind === "date" ? null : section.sessions.length;
  // Only the section of a folder opens and closes: "Pinned", "This machine" and the dates are labels.
  if (section.kind !== "folder" || !section.path) {
    return (
      <div className="sb-head">
        <div className="sb-section plain">
          <span className="sb-section-name">{section.title}</span>
          {count !== null && <span className="sb-section-count">({count})</span>}
        </div>
      </div>
    );
  }
  const path = section.path;
  return (
    <div className="sb-head">
      <button type="button" className="sb-section" data-sb-nav title={path} aria-expanded={!section.closed} onClick={() => toggleFolder(path)}>
        <Icon name="folder" size={14} />
        <span className="sb-section-name">{section.title}</span>
        <span className="sb-section-count">({count})</span>
        <span className={`sb-chevron${section.closed ? " closed" : ""}`}>
          <Icon name="chevron" size={12} />
        </span>
      </button>
      <button type="button" className="sb-add" aria-label="New session in this project" title="New session in this project" onClick={() => void createSession(path)}>
        <Icon name="plus" size={14} />
      </button>
    </div>
  );
}

/** Up, Down, Home and End among the focusable headings and rows of the list. */
function navigate(e: KeyboardEvent<HTMLDivElement>) {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const nodes = [...e.currentTarget.querySelectorAll<HTMLElement>("[data-sb-nav]:not(:disabled)")];
  const at = nodes.indexOf(document.activeElement as HTMLElement);
  if (at < 0 && e.key !== "Home" && e.key !== "End") return;
  e.preventDefault();
  const next = e.key === "Home" ? 0 : e.key === "End" ? nodes.length - 1 : Math.min(nodes.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)));
  nodes[next]?.focus();
}

export default function SessionList({ query }: { query: string }) {
  const s = useStore();
  // The date headings follow the clock: read again each minute.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);
  const searching = query.trim() !== "";
  const sessions = s.sessions;
  const sections = useMemo(
    () => (s.sidebarView === "projects" ? projectSections(sessions, s.sidebarClosed) : recentSections(sessions, now)),
    [sessions, s.sidebarView, s.sidebarClosed, now],
  );
  const found = useMemo(() => (searching ? searchSessions(sessions, query) : []), [sessions, query, searching]);
  const row = (x: (typeof sessions)[number], showFolder: boolean) => <SessionRow key={x.id} session={x} active={x.id === s.activeSession} running={s.running.has(x.id)} showFolder={showFolder} />;

  return (
    <div className="sb-list" role="region" aria-label="Sessions" onKeyDown={navigate}>
      {searching ? (
        found.length ? (
          found.map((x) => row(x, true))
        ) : (
          <div className="sb-empty">No session matches</div>
        )
      ) : sessions.length === 0 ? (
        <div className="sb-empty">No sessions yet</div>
      ) : (
        sections.map((section) => (
          <div className="sb-group" key={section.key}>
            <Heading section={section} />
            {!section.closed && section.sessions.map((x) => row(x, s.sidebarView === "recent"))}
          </div>
        ))
      )}
    </div>
  );
}
