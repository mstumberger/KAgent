// One session of the sidebar's list (sidebar.md section 5.3): a 32 px button, and the
// menu of its actions on a `...` button and on a right click.
import { useRef, useState } from "react";
import type { SessionRow as Row } from "../../settings/subagents/api";
import Icon from "../Icon";
import Menu, { type MenuItem } from "../Menu";
import AttachMenu from "./AttachMenu";
import { folderName } from "./sections";
import { openFromSidebar, removeSession, setPinned, setProject } from "./store";

type Anchor = { left: number; top: number; right: number; bottom: number };

interface Props {
  session: Row;
  active: boolean;
  running: boolean;
  /** The row says its folder first (the Recent view and the search). */
  showFolder: boolean;
}

export default function SessionRow({ session, active, running, showFolder }: Props) {
  const main = useRef<HTMLButtonElement>(null);
  const more = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState<Anchor | null>(null);
  // What the focus goes back to when a menu closes: the `...` button, or the row after a right click.
  const viaButton = useRef(true);
  const [expanded, setExpanded] = useState(false);
  const [attach, setAttach] = useState<Anchor | null>(null);
  const has = !!session.project;

  const items: MenuItem[] = [
    { id: "pin", label: session.pinned ? "Unpin" : "Pin", icon: "pin" },
    { id: "attach", label: has ? "Change folder..." : "Attach folder...", icon: "folder" },
    ...(has ? [{ id: "detach", label: "Detach folder", icon: "close" }] : []),
    { id: "delete", label: "Delete", icon: "trash", danger: true, separated: true },
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

  return (
    <div className={`sb-item${active ? " active" : ""}${running ? " running" : ""}`} onContextMenu={(e) => {
      e.preventDefault();
      openMenu({ left: e.clientX, top: e.clientY, right: e.clientX, bottom: e.clientY }, false);
    }}>
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
            if (id === "pin") void setPinned(session.id, !session.pinned);
            else if (id === "attach") setAttach(menu);
            else if (id === "detach") void setProject(session.id, null).catch(() => {});
            else if (id === "delete") removeSession(session.id);
          }}
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
