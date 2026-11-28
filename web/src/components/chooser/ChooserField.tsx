// A form field that opens the chooser (chooser.md section 1): the Defaults card and the subagent profile form.
// A button with the chosen model; the chooser opens under it (above it when there is no room).
import { useEffect, useId, useRef, useState } from "react";
import ModelChooser, { type ChooserNone } from "./ModelChooser";
import { modelLabel, type ChooserModel } from "./data";
import "../popup.css";
import "./chooser.css";

interface Props {
  ariaLabel: string;
  models: ChooserModel[];
  /** A ref, or "" for none. */
  value: string;
  onChange: (value: string) => void;
  none: ChooserNone;
  /** What to say about a value the list does not hold (shown as the current row, and on the button). */
  keptHint?: (ref: string) => string;
  disabled?: boolean;
  id?: string;
}

export default function ChooserField({ ariaLabel, models, value, onChange, none, keptHint, disabled = false, id }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popId = useId();
  const [open, setOpen] = useState(false);
  const [above, setAbove] = useState(false);
  const current = models.find((m) => m.ref === value);
  const gone = !!value && !current;
  const goneLabel = value.split("::").slice(1).join("::").replace(/^models\//, "") || value;

  const toggle = (next: boolean) => {
    if (next) {
      const rect = trigger.current?.getBoundingClientRect();
      setAbove(!!rect && window.innerHeight - rect.bottom < 420 && rect.top > window.innerHeight - rect.bottom);
    }
    setOpen(next);
  };
  const close = (refocus: boolean) => {
    setOpen(false);
    if (refocus) trigger.current?.focus();
  };

  useEffect(() => {
    if (!open) return;
    const outside = (e: Event) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const resize = () => setOpen(false);
    document.addEventListener("mousedown", outside);
    window.addEventListener("resize", resize, true);
    return () => {
      document.removeEventListener("mousedown", outside);
      window.removeEventListener("resize", resize, true);
    };
  }, [open]);

  return (
    <div ref={root} className={`mc-field${open ? " open" : ""}`}>
      <button
        ref={trigger}
        type="button"
        id={id}
        className={`dd-trigger mc-trigger${gone ? " gone" : ""}`}
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? popId : undefined}
        disabled={disabled}
        onClick={() => toggle(!open)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" && !open) {
            e.preventDefault();
            toggle(true);
          }
        }}
      >
        <span className={`dd-value${!value ? " dd-placeholder" : ""}`}>
          {current ? (
            <>
              {modelLabel(current)} <span className="pop-hint">{current.providerName}</span>
            </>
          ) : gone ? (
            goneLabel
          ) : (
            none.label
          )}
        </span>
        <svg className="dd-chevron" width={14} height={14} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      {open && (
        <div id={popId} className={`pop mc-pop${above ? " above" : ""}`}>
          <ModelChooser
            models={models}
            value={value || null}
            none={none}
            kept={gone ? { label: goneLabel, hint: keptHint?.(value) ?? "not in the list" } : undefined}
            onEscape={() => close(true)}
            onPick={(ref) => {
              onChange(ref ?? "");
              close(true);
            }}
          />
        </div>
      )}
    </div>
  );
}
