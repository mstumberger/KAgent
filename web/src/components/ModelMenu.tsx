// The model picker of the composer (picker.md section 5): a trigger with the model and the
// effort, and the menu above it. The body of the menu is also the content of the widget's sheet.
import { useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";
import { api, type ProviderView } from "../api";
import { label } from "../picker";
import { refreshModels, setSessionModel, showView } from "../store";
import { unusableWhy } from "../settings/sections/models/present";
import Icon from "./Icon";
import { useChoice } from "./useChoice";
import "./popup.css";
import "./model-menu.css";

/** Why a chosen model that is not in the list cannot answer (the providers say). */
function useWhy(ref: string | null, missing: boolean): string {
  const [why, setWhy] = useState("not available");
  useEffect(() => {
    if (!ref || !missing) return;
    api.get<{ providers: ProviderView[] }>("/api/providers").then((r) => setWhy(unusableWhy(ref, r.providers) ?? "not available"), () => {});
  }, [ref, missing]);
  return why;
}

interface Row {
  key: string;
  head?: string;
  /** `null`: the Default row. */
  ref?: string | null;
  text?: string;
  hint?: string;
  image?: boolean;
  off?: boolean;
  current?: boolean;
}

interface BodyProps {
  sessionId: string | null;
  /** The menu closes after a pick; the sheet stays open. */
  onPicked?: () => void;
  /** Closes the menu (Manage models). */
  onClose?: () => void;
  onEscape?: () => void;
}

/** The rows, effort and "Manage models" of the menu (picker.md section 5.2). */
export function ModelBody({ sessionId, onPicked, onClose, onEscape }: BodyProps) {
  const c = useChoice(sessionId);
  const why = useWhy(c.chosen, c.missing);
  const listId = useId();
  const list = useRef<HTMLDivElement>(null);
  const typed = useRef({ text: "", timer: 0 });
  const [draft, setDraft] = useState<number | null>(null);

  const rows: Row[] = [{ key: "default", ref: null, text: "Default", hint: c.chatModel ? label(c.chatModel) : "none: pick a model", current: !c.chosen }];
  if (c.missing && c.chosen) rows.unshift({ key: "gone", ref: c.chosen, text: label(c.chosen), hint: why, off: true, current: true });
  for (const m of c.models ?? []) {
    if (!rows.some((r) => r.head === m.providerName)) rows.push({ key: `h-${m.providerId}`, head: m.providerName });
    rows.push({ key: m.ref, ref: m.ref, text: label(m.ref), image: m.config.supportsImage, current: m.ref === c.chosen });
  }
  const pickable = (i: number) => !rows[i].head && !rows[i].off;
  const [cursor, setCursor] = useState(() => {
    const at = rows.findIndex((r, i) => r.current && pickable(i));
    return at >= 0 ? at : rows.findIndex((_, i) => pickable(i));
  });
  const step = (from: number, dir: 1 | -1) => {
    for (let n = 1; n <= rows.length; n++) {
      const i = (from + dir * n + rows.length * 2) % rows.length;
      if (pickable(i)) return i;
    }
    return from;
  };

  // The focus goes to the cursor row when the body opens, then follows the cursor while it is in the list:
  // a hover over a row does not take it from the slider.
  const opened = useRef(false);
  useEffect(() => {
    const at = document.activeElement;
    if (!opened.current || list.current?.contains(at)) (list.current?.children[cursor] as HTMLElement | undefined)?.focus();
    opened.current = true;
  }, [cursor]);
  useEffect(() => () => window.clearTimeout(typed.current.timer), []);

  const pick = async (i: number) => {
    const row = rows[i];
    if (!sessionId || !row || !pickable(i)) return;
    try {
      await setSessionModel(sessionId, row.ref === null ? { model: null, reasoningLevel: null } : { model: row.ref });
    } catch {
      // the store shows the refusal as a notice
    }
    onPicked?.();
  };
  const onKeys = (e: KeyboardEvent) => {
    const letter = e.key.length === 1 && e.key !== " " && !e.ctrlKey && !e.metaKey && !e.altKey;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") setCursor(step(cursor, e.key === "ArrowDown" ? 1 : -1));
    else if (e.key === "Home") setCursor(step(-1, 1));
    else if (e.key === "End") setCursor(step(rows.length, -1));
    else if (e.key === "Enter" || e.key === " ") void pick(cursor);
    else if (letter) {
      typed.current.text += e.key.toLowerCase();
      window.clearTimeout(typed.current.timer);
      typed.current.timer = window.setTimeout(() => {
        typed.current.text = "";
      }, 700);
      const hit = Array.from({ length: rows.length }, (_, n) => (cursor + n + 1) % rows.length).find((i) => pickable(i) && (rows[i].text ?? "").toLowerCase().startsWith(typed.current.text));
      if (hit !== undefined) setCursor(hit);
      return;
    } else return;
    e.preventDefault();
  };

  const stops = ["default", ...c.levels];
  const at = draft ?? c.pos;
  const setLevel = (n: number) => {
    setDraft(n);
    if (sessionId) setSessionModel(sessionId, { reasoningLevel: n === 0 ? null : c.levels[n - 1] }).catch(() => {}).finally(() => setDraft(null));
  };

  return (
    <div
      className="mm-body"
      onKeyDown={(e) => {
        if (e.key !== "Escape" || !onEscape) return;
        e.preventDefault();
        e.stopPropagation();
        onEscape();
      }}
    >
      <div ref={list} className="mm-list" role="listbox" id={listId} aria-label="Model" onKeyDown={onKeys}>
        {rows.map((r, i) =>
          r.head ? (
            <div key={r.key} role="presentation" className="pop-head">
              {r.head}
            </div>
          ) : (
            <div
              key={r.key}
              role="option"
              aria-selected={!!r.current}
              aria-disabled={r.off || undefined}
              tabIndex={i === cursor ? 0 : -1}
              className={`pop-item${i === cursor && !r.off ? " cursor" : ""}${r.off ? " off" : ""}`}
              onMouseEnter={() => pickable(i) && setCursor(i)}
              onClick={() => void pick(i)}
            >
              <span className="mm-label">
                <span className="pop-text">{r.text}</span>
                {r.hint && <span className="pop-hint pop-text">{r.hint}</span>}
              </span>
              {r.image && (
                <span className="mm-image" title="Reads images">
                  <Icon name="image" size={13} />
                </span>
              )}
              {r.current && (
                <span className="pop-check">
                  <Icon name="check" size={16} />
                </span>
              )}
            </div>
          ),
        )}
      </div>
      {stops.length > 1 && (
        <>
          <div className="pop-sep" />
          <div className="mm-effort">
            <label htmlFor={`${listId}-effort`}>Effort</label>
            <div className="mm-range" style={{ "--n": stops.length - 1, "--at": at } as CSSProperties}>
              {stops.map((stop, i) => (
                <i key={stop} className={i <= at ? "on" : ""} style={{ "--i": i } as CSSProperties} />
              ))}
              <input id={`${listId}-effort`} type="range" min={0} max={stops.length - 1} step={1} value={at} disabled={!sessionId} aria-valuetext={at === 0 ? "Provider default" : stops[at]} onChange={(e) => setLevel(Number(e.target.value))} />
            </div>
            <span className="mm-stop">{at === 0 ? "Provider default" : stops[at]}</span>
          </div>
        </>
      )}
      <div className="pop-sep" />
      <button type="button" className="pop-item mm-manage" onClick={() => {
          onClose?.();
          showView("settings", "models");
        }}>
        <Icon name="gear" size={14} />
        <span className="pop-text">Manage models</span>
      </button>
    </div>
  );
}

/** The trigger in the composer's bar, and the menu above it. */
export default function ModelMenu({ sessionId, onOpenChange }: { sessionId: string | null; onOpenChange?: (open: boolean) => void }) {
  const c = useChoice(sessionId);
  const why = useWhy(c.chosen, c.missing);
  const [open, setOpenState] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popId = useId();
  const setOpen = (next: boolean, refocus = false) => {
    setOpenState(next);
    onOpenChange?.(next);
    if (next) void refreshModels();
    if (refocus) trigger.current?.focus();
  };
  const enabled = !!sessionId;

  // Ctrl+M toggles the menu (picker.md section 5.4).
  useEffect(() => {
    const key = (e: globalThis.KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.code !== "KeyM" || !enabled) return;
      e.preventDefault();
      setOpen(!open, open);
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  });

  // A click outside, a resize or a scroll of the page closes the menu; the list's own scroll does not.
  useEffect(() => {
    if (!open) return;
    const outside = (e: Event) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    const close = () => setOpen(false);
    document.addEventListener("mousedown", outside);
    window.addEventListener("scroll", outside, true);
    window.addEventListener("resize", close, true);
    return () => {
      document.removeEventListener("mousedown", outside);
      window.removeEventListener("scroll", outside, true);
      window.removeEventListener("resize", close, true);
    };
  });

  const name = c.none ? "No model" : label(c.chosen ?? c.chatModel);
  // The tag is the level the answering model offers: a stored level it does not list is not shown (picker.md section 5.1).
  const effort = c.level && c.pos > 0 && !c.none ? c.level : "";
  const tip = `Model and effort (Ctrl+M): ${name}${effort ? ` · ${effort}` : ""}${c.missing ? ` - ${why}` : ""}`;
  return (
    <div ref={root} className="mm">
      <button
        ref={trigger}
        type="button"
        className={`mm-trigger${open ? " open" : ""}${c.none ? " none" : ""}${c.missing ? " gone" : ""}`}
        title={tip}
        aria-label={tip}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? popId : undefined}
        disabled={!enabled}
        onClick={() => setOpen(!open)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" && !open) {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        <span className="mm-chevron">
          <Icon name="chevron" size={14} />
        </span>
        <span className="mm-name">{name}</span>
        {effort && <span className="mm-effort-tag">· {effort}</span>}
      </button>
      {open && (
        <div id={popId} className="pop mm-pop">
          <ModelBody sessionId={sessionId} onPicked={() => setOpen(false, true)} onClose={() => setOpen(false)} onEscape={() => setOpen(false, true)} />
        </div>
      )}
    </div>
  );
}
