// The stack of notices (notices.md section 2): top right under the title bar, at the top of
// the widget. An error stays until closed; an info or a recovery goes after 6 s; the same
// key is one toast with a count. Escape closes the focused one.
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import Icon from "./Icon";
import { api } from "../api";
import { dismiss, dismissAll, INFO_MS, notify, SHOWN, useToasts, type Toast } from "./toasts";
import { openSession, setView, useStore } from "../store";
import "./toasts.css";

const time = (at: number) => new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
const ICON: Record<Toast["kind"], string> = { error: "alert", recovery: "check", info: "info" };

function ToastItem({ toast, open }: { toast: Toast; open: boolean }) {
  const [held, setHeld] = useState(false);
  const [more, setMore] = useState(false);
  const [clipped, setClipped] = useState(false);
  const text = useRef<HTMLSpanElement>(null);
  // Hover or focus holds the timer; a new count or text starts it again.
  useEffect(() => {
    if (toast.kind === "error" || held) return;
    const timer = setTimeout(() => dismiss(toast.id), toast.ms ?? INFO_MS);
    return () => clearTimeout(timer);
  }, [toast.id, toast.kind, toast.at, toast.ms, held]);
  useLayoutEffect(() => {
    const el = text.current;
    if (el && !more) setClipped(el.scrollHeight > el.clientHeight + 1);
  }, [toast.text, more]);
  return (
    <div
      className={`toast ${toast.kind}`}
      role={toast.kind === "error" ? "alert" : "status"}
      data-key={toast.key ?? undefined}
      onMouseEnter={() => setHeld(true)}
      onMouseLeave={() => setHeld(false)}
      onFocus={() => setHeld(true)}
      onBlur={() => setHeld(false)}
      onKeyDown={(e) => {
        if (e.key !== "Escape") return;
        e.stopPropagation();
        dismiss(toast.id);
      }}
    >
      <span className="toast-icon" aria-hidden="true">
        <Icon name={ICON[toast.kind]} size={14} />
      </span>
      <div className="toast-body">
        <span ref={text} className={`toast-text${more ? " all" : ""}`}>{toast.text}</span>
        <div className="toast-meta">
          {toast.count > 1 && <span className="toast-count" title={`${toast.count} times`}>x{toast.count}</span>}
          <time dateTime={new Date(toast.at).toISOString()}>{time(toast.at)}</time>
          {(clipped || more) && (
            <button type="button" className="toast-link" aria-expanded={more} onClick={() => setMore(!more)}>
              {more ? "less" : "more"}
            </button>
          )}
          {toast.local && (
            <button
              type="button"
              className="toast-link toast-action"
              onClick={() => {
                toast.local?.run();
                dismiss(toast.id, false);
              }}
            >
              {toast.local.label}
            </button>
          )}
          {toast.action && (
            <button
              type="button"
              className="toast-link toast-action"
              onClick={() => {
                const action = toast.action;
                if (!action) return;
                api.post(action.path, action.body).then(
                  () => dismiss(toast.id),
                  (e) => notify(`${action.label} failed: ${e instanceof Error ? e.message : String(e)}`),
                );
              }}
            >
              {toast.action.label}
            </button>
          )}
          {open && (
            <button
              type="button"
              className="toast-link"
              onClick={() => {
                setView("chat");
                void openSession(toast.sessionId as string);
                dismiss(toast.id);
              }}
            >
              Open conversation
            </button>
          )}
        </div>
      </div>
      <button type="button" className="toast-close" aria-label="Dismiss" onClick={() => dismiss(toast.id)}>
        <Icon name="close" size={12} />
      </button>
    </div>
  );
}

export default function Toasts({ compact = false }: { compact?: boolean }) {
  const toasts = useToasts();
  const s = useStore();
  const [all, setAll] = useState(false);
  const [top, setTop] = useState(8);
  // Under the title bar of the full window; at the top of the widget (which has none).
  useLayoutEffect(() => {
    const place = () => setTop(Math.round((document.querySelector("header")?.getBoundingClientRect().bottom ?? 0) + 8));
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [compact]);
  if (toasts.length === 0) return null;
  const folded = all ? 0 : Math.max(0, toasts.length - SHOWN);
  const shown = toasts.slice(0, toasts.length - folded);
  return (
    <section className={`toasts${compact ? " compact" : ""}`} role="region" aria-label="Notifications" style={{ top }}>
      {toasts.length >= 2 && (
        <div className="toasts-bar">
          <button type="button" className="toast-link" onClick={() => { setAll(false); dismissAll(); }}>
            Dismiss all
          </button>
        </div>
      )}
      {shown.map((t) => (
        <ToastItem key={t.id} toast={t} open={!!t.sessionId && t.sessionId !== s.activeSession} />
      ))}
      {toasts.length > SHOWN && (
        <button type="button" className="toast-fold" aria-expanded={all} onClick={() => setAll(!all)}>
          {all ? "Show fewer" : `${folded} more`}
        </button>
      )}
    </section>
  );
}
