// The widget's one sheet (picker.md section 5.5): a panel over the widget for the model
// picker (S41) and, later, the session list and the work panel. Escape closes it; the
// keys of the widget do nothing while it is open (Overlay checks `getStore().sheet`).
import { useEffect, useRef, useState, type ReactNode } from "react";
import Icon from "./Icon";
import "./popup.css";
import "./sheet.css";
import { useOverlay } from "../router/overlays";

export default function Sheet({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  useOverlay(true, onClose);
  const close = useRef(onClose);
  useEffect(() => {
    close.current = onClose;
  });
  // Where the focus was, read before the body takes it (the chip that opened the sheet).
  const [before] = useState(() => document.activeElement as HTMLElement | null);
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // A menu or a popup over the sheet takes the key first (sidebar.md section 5.8).
      if (document.querySelector(".menu, .attach")) return;
      e.preventDefault();
      e.stopPropagation();
      close.current();
    };
    window.addEventListener("keydown", key, true);
    return () => {
      window.removeEventListener("keydown", key, true);
      before?.focus?.();
    };
  }, [before]);
  return (
    <div className="sheet pop" role="dialog" aria-modal="true" aria-label={title}>
      <div className="sheet-head">
        <span className="sheet-title">{title}</span>
        <button type="button" className="sheet-close" aria-label="Close" onClick={onClose}>
          <Icon name="close" size={14} />
        </button>
      </div>
      <div className="sheet-body">{children}</div>
    </div>
  );
}
