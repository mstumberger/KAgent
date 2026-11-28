// "Attach folder..." and "Change folder..." (sidebar.md section 5.4): a popup with a field for
// a path and the projects of `GET /api/projects`. A pick or Enter sends one PUT; a refusal
// stays under the field.
import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import type { SessionRow } from "../../settings/subagents/api";
import { showView, useStore } from "../../store";
import Icon from "../Icon";
import "../popup.css";
import "./attach.css";
import { closeDrawer, refreshProjects, setProject } from "./store";
import { useOverlay } from "../../router/overlays";

interface Props {
  session: SessionRow;
  anchor: { left: number; top: number; right: number; bottom: number };
  onClose: (restore: boolean) => void;
}

const WIDTH = 300;

export default function AttachMenu({ session, anchor, onClose }: Props) {
  const s = useStore();
  useOverlay(true, () => onClose(true));
  const box = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const id = useId();
  const [text, setText] = useState("");
  const [cursor, setCursor] = useState(-1);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [pos, setPos] = useState<CSSProperties>({ left: anchor.right - WIDTH, top: anchor.bottom + 4, opacity: 0 });

  useEffect(() => {
    void refreshProjects();
  }, []);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const { height } = el.getBoundingClientRect();
    const left = Math.min(Math.max(8, anchor.right - WIDTH), Math.max(8, window.innerWidth - WIDTH - 8));
    let top = anchor.bottom + 4;
    if (top + height > window.innerHeight - 8) top = Math.max(8, Math.min(anchor.top - height - 4, window.innerHeight - height - 8));
    setPos({ left, top });
    field.current?.focus({ preventScroll: true });
  }, [anchor, s.projects]);
  useEffect(() => {
    const outside = (e: Event) => {
      if (!box.current?.contains(e.target as Node)) onClose(false);
    };
    document.addEventListener("mousedown", outside);
    window.addEventListener("resize", outside, true);
    return () => {
      document.removeEventListener("mousedown", outside);
      window.removeEventListener("resize", outside, true);
    };
  }, [onClose]);

  const q = text.trim().toLowerCase();
  const all = s.projects ?? [];
  const shown = all.filter((p) => !q || p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q));

  const send = async (path: string) => {
    if (busy || !path) return;
    setBusy(true);
    try {
      await setProject(session.id, path);
      onClose(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose(true);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor(Math.min(cursor + 1, shown.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor(Math.max(cursor - 1, -1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      void send(cursor >= 0 && shown[cursor] ? shown[cursor].path : text.trim());
    } else if (e.key === "Tab") onClose(true);
  };

  return createPortal(
    <div ref={box} className="pop attach" role="dialog" aria-label={session.project ? "Change folder" : "Attach folder"} style={pos} onKeyDown={onKeyDown}>
      <input
        ref={field}
        className="attach-field"
        placeholder="Type a path, or pick a project"
        aria-label="Folder path"
        aria-invalid={error ? true : undefined}
        aria-describedby={error ? `${id}-err` : undefined}
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          setCursor(-1);
          setError("");
        }}
      />
      {error && (
        <div id={`${id}-err`} className="attach-error" role="alert">
          {error}
        </div>
      )}
      <div className="attach-list" role="listbox" aria-label="Projects">
        {shown.map((p, i) => (
          <div key={p.path} role="option" aria-selected={p.path === session.project} className={`pop-item attach-row${i === cursor ? " cursor" : ""}`} title={p.path} onMouseEnter={() => setCursor(i)} onClick={() => void send(p.path)}>
            <Icon name="folder" size={14} />
            <span className="pop-text">
              {p.name}
              <span className="pop-hint attach-path">{p.path}</span>
            </span>
            {p.agentsMd && <span className="attach-tag">AGENTS.md</span>}
            {p.path === session.project && (
              <span className="pop-check">
                <Icon name="check" size={14} />
              </span>
            )}
          </div>
        ))}
        {s.projects && all.length === 0 && (
          <>
            <div className="attach-none">No projects under the project folders</div>
            <button
              type="button"
              className="pop-item attach-row attach-settings"
              onClick={() => {
                onClose(false);
                closeDrawer();
                showView("settings", "desktop");
              }}
            >
              <Icon name="gear" size={14} />
              <span className="pop-text">Project folders in Settings</span>
            </button>
          </>
        )}
        {s.projects && all.length > 0 && shown.length === 0 && <div className="attach-none">No project matches</div>}
      </div>
    </div>,
    document.body,
  );
}
