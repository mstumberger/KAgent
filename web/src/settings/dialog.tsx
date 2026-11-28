// A dialog over the page: a title, a body that scrolls, a footer. `Confirm` (ui.tsx) is
// the special case of a question; this is the general one, for a form or a list. Escape
// and a click on the backdrop close it, the focus stays inside while it is open, and it
// goes back to where it was.
import { useEffect, useId, useRef, type ReactNode } from "react";
import "./dialog.css";
import { useOverlay } from "../router/overlays";

const FOCUSABLE = "input:not([disabled]), button:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href]";

interface Props {
  title: string;
  children: ReactNode;
  /** Buttons under the body. */
  footer?: ReactNode;
  /** Called for Escape, the backdrop and the Close button. The caller may wait for a running action. */
  onClose: () => void;
  wide?: boolean;
}

export default function Dialog({ title, children, footer, onClose, wide = false }: Props) {
  useOverlay(true, onClose);
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  // The latest `onClose`, so that the effect below starts once and not on every render.
  const close = useRef(onClose);
  useEffect(() => {
    close.current = onClose;
  }, [onClose]);

  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panel.current?.querySelector<HTMLElement>(FOCUSABLE)?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close.current();
        return;
      }
      const box = panel.current;
      if (e.key !== "Tab" || !box) return;
      const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
      const first = items[0];
      const last = items[items.length - 1];
      if (!first || !last) return;
      const inside = box.contains(document.activeElement);
      if (e.shiftKey && (document.activeElement === first || !inside)) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (document.activeElement === last || !inside)) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      before?.focus();
    };
  }, []);

  return (
    <div
      className="ui-confirm-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div ref={panel} className={`ui-dialog${wide ? " wide" : ""}`} role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <div className="ui-dialog-head">
          <h3 id={titleId}>{title}</h3>
          <button type="button" onClick={onClose}>
            Close
          </button>
        </div>
        <div className="ui-dialog-body">{children}</div>
        {footer && <div className="ui-dialog-foot">{footer}</div>}
      </div>
    </div>
  );
}
