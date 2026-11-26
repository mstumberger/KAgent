// The shared parts of a settings page (ui-settings.md section 4). This file exports
// components only, so fast refresh keeps working; hooks live in the .ts files next to it.
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { useSettingPatch } from "./useSettingPatch";

/** The frame of a page: a fixed header (title, description, actions, save result) over the scrolling body. */
export function SectionPage({ title, description, actions, children }: { title: string; description: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="ui-section-page">
      <header className="ui-section-head">
        <div className="ui-section-head-inner">
          <div className="ui-section-text">
            <h2>{title}</h2>
            <p className="hint">{description}</p>
          </div>
          <div className="ui-section-actions">
            <SaveStatus />
            {actions}
          </div>
        </div>
      </header>
      <div className="ui-section-scroll">
        <div className="ui-section">{children}</div>
      </div>
    </div>
  );
}

/** The result of the last save or page action, in the header of every page. */
function SaveStatus() {
  const { busy, error, note, savedAt } = useSettingPatch();
  const [now, setNow] = useState(0);
  // "Saved" shows for 2.5 s.
  useEffect(() => {
    if (!savedAt) return;
    const t = setTimeout(() => {
      setNow(Date.now());
    }, 2500);
    return () => {
      clearTimeout(t);
    };
  }, [savedAt]);
  const saved = !!savedAt && now < savedAt;
  // `error` is null until a save is refused, even when the reason is empty.
  const failed = error !== null;
  const text = busy ? "Saving..." : failed ? `Not saved: ${error}` : (note?.text ?? (saved ? "Saved" : ""));
  const tone = busy ? "neutral" : failed || note?.failed ? "bad" : saved ? "ok" : "accent";
  return text ? (
    <span className={`ui-status ui-${tone}`} role="status">
      {text}
    </span>
  ) : null;
}

/** One card of related rows. */
export function GroupCard({ title, description, children }: { title?: string; description?: string; children: ReactNode }) {
  return (
    <section className="ui-card">
      {(title || description) && (
        <header className="ui-card-head">
          {title && <h3>{title}</h3>}
          {description && <p className="hint">{description}</p>}
        </header>
      )}
      {children}
    </section>
  );
}

/** A label and a hint on the left, the control on the right. `stacked` puts the control under the text. */
export function Row({ label, hint, children, stacked }: { label: string; hint?: string; children: ReactNode; stacked?: boolean }) {
  return (
    <div className={`ui-row${stacked ? " stacked" : ""}`}>
      <div className="ui-row-text">
        <span className="ui-row-label">{label}</span>
        {hint && <span className="hint">{hint}</span>}
      </div>
      <div className="ui-row-control">{children}</div>
    </div>
  );
}

/** A label above one input (the label wraps it, so the input is named). */
export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="ui-field">
      <span className="ui-field-label">{label}</span>
      {children}
      {hint && <span className="hint">{hint}</span>}
    </label>
  );
}

/** A switch. With `label` its text names it; without, `title` does. */
export function Toggle({ checked, onChange, label, disabled, title }: { checked: boolean; onChange: (checked: boolean) => void; label?: string; disabled?: boolean; title?: string }) {
  const name = label ?? title ?? (checked ? "On" : "Off");
  return (
    <label className={`ui-toggle${label ? "" : " bare"}`}>
      <input type="checkbox" checked={checked} disabled={disabled} aria-label={label ? undefined : name} onChange={(e) => onChange(e.target.checked)} />
      <span className="ui-toggle-track" aria-hidden="true" />
      {label && <span className="ui-toggle-label">{label}</span>}
    </label>
  );
}

const TONES = ["neutral", "ok", "warn", "bad", "accent"] as const;
export type BadgeTone = (typeof TONES)[number];

/** A small pill: a state, a source, a role. */
export function Badge({ tone = "neutral", children }: { tone: BadgeTone; children: ReactNode }) {
  return <span className={`ui-badge ui-${tone}`}>{children}</span>;
}

/** An empty list. */
export function EmptyState({ title, hint, actions }: { title: string; hint?: string; actions?: ReactNode }) {
  return (
    <div className="ui-empty">
      <div className="ui-empty-title">{title}</div>
      {hint && <p className="hint">{hint}</p>}
      {actions && <div className="row">{actions}</div>}
    </div>
  );
}

/** A dialog that asks first (it replaces the native `confirm`). Escape and a click outside cancel. */
export function Confirm({
  title,
  body,
  confirmLabel,
  danger,
  onConfirm,
  onCancel,
}: {
  title: string;
  body: string;
  confirmLabel: string;
  danger?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const titleId = useId();
  const cancel = useRef<HTMLButtonElement>(null);
  const confirm = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // A dangerous dialog opens on Cancel, so Enter alone does nothing.
    (danger ? cancel : confirm).current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [onCancel, danger]);
  return (
    <div
      className="ui-confirm-backdrop"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="ui-confirm" role="alertdialog" aria-modal="true" aria-labelledby={titleId}>
        <h3 id={titleId}>{title}</h3>
        <p>{body}</p>
        <div className="row">
          <button ref={cancel} onClick={onCancel}>
            Cancel
          </button>
          <button ref={confirm} className={danger ? "danger" : "primary"} onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/** A list on the left, the selected item on the right. It keeps the `.pv-*` rules of index.css. */
export function ListDetail<T>({
  items,
  keyOf,
  selectedKey,
  onSelect,
  renderRow,
  detail,
  actions,
  label,
  empty,
}: {
  items: T[];
  keyOf: (item: T) => string;
  selectedKey: string | null;
  onSelect: (item: T) => void;
  renderRow: (item: T) => ReactNode;
  detail: ReactNode;
  /** Buttons under the list, an add button. */
  actions?: ReactNode;
  /** What the list holds, for screen readers. */
  label: string;
  /** Shown instead of an empty list. */
  empty: string;
}) {
  return (
    <div className="pv-layout ld">
      <nav className="pv-nav" aria-label={label}>
        {items.map((item) => {
          const key = keyOf(item);
          return (
            <button
              key={key}
              className={`pv-nav-item${key === selectedKey ? " active" : ""}`}
              aria-current={key === selectedKey ? "page" : undefined}
              onClick={() => onSelect(item)}
            >
              {renderRow(item)}
            </button>
          );
        })}
        {items.length === 0 && <div className="hint pv-nav-empty">{empty}</div>}
        {actions}
      </nav>
      <section className="pv-detail">{detail}</section>
    </div>
  );
}
