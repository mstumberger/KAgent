// The list of the sidebar (sidebar.md section 5.3): Projects or Recent, or the flat result of
// a search. A heading is a button; Up and Down move the focus among the headings and rows.
import { useEffect, useMemo, useState, type DragEvent, type KeyboardEvent, type ReactNode } from "react";
import type { SessionRow as Row } from "../../settings/subagents/api";
import { useStore } from "../../store";
import Icon from "../Icon";
import SessionRow, { DRAG_TYPE } from "./SessionRow";
import { moveSession } from "./convo";
import { archivedOf, projectSections, recentSections, searchSessions, type Section } from "./sections";
import { createSession, toggleFolder } from "./store";
import "./convo.css";

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

/** The conversations the owner put away: a collapsed group at the foot of the list. */
function ArchivedGroup({ rows, open, onToggle, row }: { rows: Row[]; open: boolean; onToggle: () => void; row: (x: Row) => ReactNode }) {
  if (rows.length === 0) return null;
  return (
    <div className="sb-group sb-archived">
      <div className="sb-head">
        <button type="button" className="sb-section" data-sb-nav aria-expanded={open} onClick={onToggle}>
          <Icon name="box" size={14} />
          <span className="sb-section-name">Archived</span>
          <span className="sb-section-count">({rows.length})</span>
          <span className={`sb-chevron${open ? "" : " closed"}`}>
            <Icon name="chevron" size={12} />
          </span>
        </button>
      </div>
      {open && rows.map(row)}
    </div>
  );
}

/** A group that takes a dragged row: the folder's section moves it there, "This machine" takes it out of its project. */
function dropProps(section: Section, over: string | null, setOver: (key: string | null) => void) {
  if (section.kind !== "folder" && section.kind !== "machine") return {};
  const takes = (e: DragEvent<HTMLElement>) => e.dataTransfer.types.includes(DRAG_TYPE);
  return {
    onDragOver: (e: DragEvent<HTMLElement>) => {
      if (!takes(e)) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      if (over !== section.key) setOver(section.key);
    },
    onDragLeave: (e: DragEvent<HTMLElement>) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(null);
    },
    onDrop: (e: DragEvent<HTMLElement>) => {
      if (!takes(e)) return;
      e.preventDefault();
      setOver(null);
      void moveSession(e.dataTransfer.getData(DRAG_TYPE), section.kind === "folder" ? (section.path ?? null) : null);
    },
  };
}

export default function SessionList({ query, archived: inSearch = false }: { query: string; archived?: boolean }) {
  const s = useStore();
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [over, setOver] = useState<string | null>(null);
  // The date headings follow the clock: read again each minute.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, []);
  const searching = query.trim() !== "";
  // A conversation whose delete waits for its undo is not listed (shell.md section 5).
  const sessions = useMemo(() => s.sessions.filter((x) => !s.hiddenSessions.includes(x.id)), [s.sessions, s.hiddenSessions]);
  const sections = useMemo(
    () => (s.sidebarView === "projects" ? projectSections(sessions, s.sidebarClosed) : recentSections(sessions, now)),
    [sessions, s.sidebarView, s.sidebarClosed, now],
  );
  const found = useMemo(() => (searching ? searchSessions(sessions, query, inSearch) : []), [sessions, query, searching, inSearch]);
  const away = useMemo(() => archivedOf(sessions), [sessions]);
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
        <>
          {sections.map((section) => (
            <div className={`sb-group${over === section.key ? " drop" : ""}`} key={section.key} {...(s.sidebarView === "projects" ? dropProps(section, over, setOver) : {})}>
              <Heading section={section} />
              {!section.closed && section.sessions.map((x) => row(x, s.sidebarView === "recent"))}
            </div>
          ))}
          <ArchivedGroup rows={away} open={archiveOpen} onToggle={() => setArchiveOpen(!archiveOpen)} row={(x) => row(x, true)} />
        </>
      )}
    </div>
  );
}
