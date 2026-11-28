// One session of the sidebar's list (sidebar.md section 5.3, shell.md section 5): a 32 px button, and the menu of its
// actions on a `...` button, on a right click, on the menu key and on Shift+F10. F2 renames the focused row, Delete
// deletes it (with ten seconds of undo), and a row can be dragged onto a project.
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { SessionRow as Row } from "../../settings/subagents/api";
import { useStore } from "../../store";
import Icon from "../Icon";
import Menu, { type MenuItem } from "../Menu";
import AttachMenu from "./AttachMenu";
import { archiveSession, deleteWithUndo, moveSession, renameSession } from "./convo";
import { folderName } from "./sections";
import { openFromSidebar, refreshProjects, setPinned, setProject } from "./store";

type Anchor = { left: number; top: number; right: number; bottom: number };

/** What a dragged row carries: the id of its session. */
export const DRAG_TYPE = "application/x-kagent-session";
const NONE = "__none";

interface Props {
  session: Row;
  active: boolean;
  running: boolean;
  /** The row says its folder first (the Recent view and the search). */
  showFolder: boolean;
}

export default function SessionRow({ session, active, running, showFolder }: Props) {
  const s = useStore();
  const main = useRef<HTMLButtonElement>(null);
  const more = useRef<HTMLButtonElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [menu, setMenu] = useState<Anchor | null>(null);
  // What the focus goes back to when a menu closes: the `...` button, or the row after a right click.
  const viaButton = useRef(true);
  const [expanded, setExpanded] = useState(false);
  const [attach, setAttach] = useState<Anchor | null>(null);
  const [move, setMove] = useState<Anchor | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const has = !!session.project;
  const archived = !!session.archived;

  const items: MenuItem[] = [
    { id: "rename", label: "Rename", icon: "pencil", hint: "F2" },
    ...(archived ? [] : [{ id: "pin", label: session.pinned ? "Unpin" : "Pin", icon: "pin" }]),
    ...(archived ? [] : [{ id: "move", label: "Move to project...", icon: "folder" }]),
    ...(archived ? [] : [{ id: "attach", label: has ? "Change folder..." : "Attach folder...", icon: "folder" }]),
    ...(has && !archived ? [{ id: "detach", label: "Detach folder", icon: "close" }] : []),
    { id: "archive", label: archived ? "Unarchive" : "Archive", icon: "box", separated: true },
    { id: "delete", label: "Delete", icon: "trash", danger: true, hint: "Del" },
  ];
  const openMenu = (anchor: Anchor, button: boolean) => {
    viaButton.current = button;
    setExpanded(button);
    setMenu(anchor);
  };
  const back = () => (viaButton.current ? more.current : main.current)?.focus();
  const rect = (el: HTMLElement | null): Anchor => {
    const r = el?.getBoundingClientRect();
    return r ? { left: r.left, top: r.top, right: r.right, bottom: r.bottom } : { left: 0, top: 0, right: 0, bottom: 0 };
  };

  const startRename = () => {
    setDraft(session.title || "");
    setEditing(true);
  };
  useEffect(() => {
    if (editing) field.current?.select();
  }, [editing]);
  const finishRename = (save: boolean) => {
    if (!editing) return;
    setEditing(false);
    if (save) void renameSession(session.id, draft);
    requestAnimationFrame(() => main.current?.focus());
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (editing || e.target !== main.current || e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === "F2") {
      e.preventDefault();
      startRename();
    } else if (e.key === "Delete") {
      e.preventDefault();
      // The row goes: the focus moves to its neighbour, not to the page.
      const rows = [...document.querySelectorAll<HTMLElement>(".sb-list [data-sb-nav]")];
      const at = rows.indexOf(main.current as HTMLElement);
      const neighbour = rows[at + 1] ?? rows[at - 1];
      deleteWithUndo(session.id);
      requestAnimationFrame(() => neighbour?.focus());
    } else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      openMenu(rect(main.current), false);
    }
  };

  const projects = () => {
    const known = new Map<string, string>();
    for (const p of s.projects ?? []) known.set(p.path, p.name);
    for (const x of s.sessions) if (x.project && !known.has(x.project)) known.set(x.project, folderName(x.project));
    return [...known].sort((a, b) => a[1].localeCompare(b[1]));
  };

  return (
    <div
      className={`sb-item${active ? " active" : ""}${running ? " running" : ""}${archived ? " archived" : ""}`}
      draggable={!editing}
      onDragStart={(e) => {
        e.dataTransfer.setData(DRAG_TYPE, session.id);
        e.dataTransfer.effectAllowed = "move";
      }}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => {
        e.preventDefault();
        openMenu({ left: e.clientX, top: e.clientY, right: e.clientX, bottom: e.clientY }, false);
      }}
    >
      {editing ? (
        <input
          ref={field}
          className="sb-rename"
          aria-label="Session title"
          maxLength={120}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => finishRename(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              finishRename(true);
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              finishRename(false);
            }
          }}
        />
      ) : (
        <button ref={main} type="button" className="sb-row-main" data-sb-nav aria-current={active ? "true" : undefined} onClick={() => openFromSidebar(session.id)}>
          {session.pinned && (
            <span className="sb-pin" aria-label="Pinned">
              <Icon name="pin" size={12} />
            </span>
          )}
          {showFolder && session.project && <span className="sb-folder-word">{folderName(session.project)}</span>}
          <span className="sb-title">{session.title || "New session"}</span>
          {session.agents ? <span className="sb-count">{session.agents} agent{session.agents === 1 ? "" : "s"}</span> : null}
          {running && <span className="sb-run" role="img" aria-label="running" />}
        </button>
      )}
      <button
        ref={more}
        type="button"
        className="sb-more"
        aria-label="More"
        aria-haspopup="menu"
        aria-expanded={menu && expanded ? true : undefined}
        onClick={() => (menu ? setMenu(null) : openMenu(rect(more.current), true))}
      >
        <Icon name="more" size={14} />
      </button>
      {menu && (
        <Menu
          items={items}
          anchor={menu}
          label={`Actions for ${session.title || "New session"}`}
          onClose={(restore) => {
            setMenu(null);
            if (restore) back();
          }}
          onPick={(id) => {
            if (id === "rename") startRename();
            else if (id === "pin") void setPinned(session.id, !session.pinned);
            else if (id === "move") {
              void refreshProjects();
              setMove(menu);
            } else if (id === "attach") setAttach(menu);
            else if (id === "detach") void setProject(session.id, null).catch(() => {});
            else if (id === "archive") void archiveSession(session.id, !archived);
            else if (id === "delete") deleteWithUndo(session.id);
          }}
        />
      )}
      {move && (
        <Menu
          items={[
            ...projects().map(([path, name]): MenuItem => ({ id: path, label: name, icon: "folder", checked: path === session.project })),
            { id: NONE, label: "No project", icon: "close", checked: !has, separated: true },
          ]}
          anchor={move}
          label={`Move ${session.title || "New session"} to a project`}
          onClose={(restore) => {
            setMove(null);
            if (restore) back();
          }}
          onPick={(id) => void moveSession(session.id, id === NONE ? null : id)}
        />
      )}
      {attach && (
        <AttachMenu
          session={session}
          anchor={attach}
          onClose={(restore) => {
            setAttach(null);
            if (restore) back();
          }}
        />
      )}
    </div>
  );
}
