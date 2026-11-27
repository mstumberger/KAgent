// A list of actions in a portal (sidebar.md section 5.3), in the house style of popup.css:
// `position: fixed` at its anchor, the focus on the menu with the cursor as its
// `aria-activedescendant`. Up, Down, Home and End move the cursor, Enter and Space pick,
// Escape closes and gives the focus back, Tab closes and lets the focus move on, a click
// outside closes, and letters jump to the next label that starts with them.
import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import Icon from "./Icon";
import "./popup.css";
import "./menu.css";

export interface MenuItem {
  id: string;
  label: string;
  /** The icon at the left. */
  icon?: string;
  /** A dim end of the row (the current mode's hint). */
  hint?: ReactNode;
  /** The check mark of the current choice. */
  checked?: boolean;
  /** In the colour of a destructive action. */
  danger?: boolean;
  /** A line above the item. */
  separated?: boolean;
}

interface Props {
  items: MenuItem[];
  /** Where the menu opens: the rectangle of its button, or a point (a right click). */
  anchor: { left: number; top: number; right: number; bottom: number };
  /** `below` (aligned to the right edge of the anchor, as the menu of a row) or `right` (beside it, as the permission dot of the rail). */
  placement?: "below" | "right";
  label: string;
  onPick: (id: string) => void;
  /** `restore`: Escape, Tab and a pick give the focus back to what opened the menu. */
  onClose: (restore: boolean) => void;
}

const MARGIN = 8;

export default function Menu({ items, anchor, placement = "below", label, onPick, onClose }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const id = useId();
  // The cursor starts on the current choice (the checked item), else on the first.
  const [cursor, setCursor] = useState(() => Math.max(0, items.findIndex((i) => i.checked)));
  const [pos, setPos] = useState<CSSProperties>({ left: anchor.left, top: anchor.bottom + 4, opacity: 0 });
  const typed = useRef({ text: "", timer: 0 });
  // The focus that comes back to a button scrolls the list a moment later: a scroll that early is not the owner's.
  const [born] = useState(() => performance.now());

  // Placed after it was measured: kept inside the window, above the anchor when there is no room below.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    let left = placement === "right" ? anchor.right + 4 : anchor.right - width;
    let top = placement === "right" ? anchor.bottom - height : anchor.bottom + 4;
    if (placement === "below" && top + height > window.innerHeight - MARGIN) top = Math.max(MARGIN, anchor.top - height - 4);
    left = Math.min(Math.max(MARGIN, left), Math.max(MARGIN, window.innerWidth - width - MARGIN));
    top = Math.min(Math.max(MARGIN, top), Math.max(MARGIN, window.innerHeight - height - MARGIN));
    setPos({ left, top });
    el.focus({ preventScroll: true });
  }, [anchor, placement]);

  useEffect(() => {
    const outside = (e: Event) => {
      // A press on the button that opened the menu is its own click: that click closes the menu.
      if ((e.target as Element).closest?.('[aria-haspopup="menu"][aria-expanded="true"]')) return;
      if (!box.current?.contains(e.target as Node)) onClose(false);
    };
    const away = (e: Event) => {
      if (e.type === "scroll" && performance.now() - born < 250) return;
      if (!box.current?.contains(e.target as Node)) onClose(false);
    };
    document.addEventListener("mousedown", outside);
    window.addEventListener("resize", away, true);
    window.addEventListener("scroll", away, true);
    return () => {
      document.removeEventListener("mousedown", outside);
      window.removeEventListener("resize", away, true);
      window.removeEventListener("scroll", away, true);
    };
  }, [onClose, born]);
  useEffect(() => () => window.clearTimeout(typed.current.timer), []);

  const step = (from: number, dir: 1 | -1) => (from + dir + items.length) % items.length;
  const pick = (i: number) => {
    onClose(true);
    onPick(items[i].id);
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const letter = e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey && !e.altKey;
    if (e.key === "ArrowDown") setCursor(step(cursor, 1));
    else if (e.key === "ArrowUp") setCursor(step(cursor, -1));
    else if (e.key === "Home") setCursor(0);
    else if (e.key === "End") setCursor(items.length - 1);
    else if (e.key === "Enter" || e.key === " ") pick(cursor);
    else if (e.key === "Escape") onClose(true);
    else if (e.key === "Tab") {
      // The focus goes back first, so that the Tab moves on from the button that opened the menu.
      onClose(true);
      return;
    } else if (letter) {
      typed.current.text += e.key.toLowerCase();
      window.clearTimeout(typed.current.timer);
      typed.current.timer = window.setTimeout(() => {
        typed.current.text = "";
      }, 700);
      for (let n = 1; n <= items.length; n++) {
        const i = (cursor + n) % items.length;
        if (items[i].label.toLowerCase().startsWith(typed.current.text)) {
          setCursor(i);
          break;
        }
      }
    } else return;
    e.preventDefault();
    e.stopPropagation();
  };

  return createPortal(
    <div ref={box} className="pop menu" role="menu" aria-label={label} tabIndex={-1} aria-activedescendant={`${id}-${cursor}`} style={pos} onKeyDown={onKeyDown} onContextMenu={(e) => e.preventDefault()}>
      {items.map((item, i) => (
        <div key={item.id} className="menu-entry">
          {item.separated && <div className="pop-sep" role="separator" />}
          <div
            id={`${id}-${i}`}
            role="menuitem"
            className={`pop-item menu-item${i === cursor ? " cursor" : ""}${item.danger ? " danger" : ""}`}
            onMouseEnter={() => setCursor(i)}
            onClick={() => pick(i)}
          >
            {item.icon && <Icon name={item.icon} size={14} />}
            <span className="pop-text">{item.label}</span>
            {item.hint !== undefined && <span className="pop-hint">{item.hint}</span>}
            {item.checked && (
              <span className="pop-check">
                <Icon name="check" size={14} />
              </span>
            )}
          </div>
        </div>
      ))}
    </div>,
    document.body,
  );
}

