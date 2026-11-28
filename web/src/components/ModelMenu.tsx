// The model picker of the composer (picker.md section 5): a trigger with the model and the
// effort, and the menu above it. The body of the menu is also the content of the widget's sheet.
import { useEffect, useId, useRef, useState, type CSSProperties } from "react";
import { api, type ProviderView } from "../api";
import { label } from "../picker";
import { providerOfRef } from "../usage";
import { refreshModels, setSessionModel, showView } from "../store";
import { unusableWhy } from "../settings/sections/models/present";
import Icon from "./Icon";
import ModelChooser from "./chooser/ModelChooser";
import UsageRingFor from "./UsageRing";
import { useChoice } from "./useChoice";
import "./popup.css";
import "./model-menu.css";
import { useOverlay } from "../router/overlays";

/** Why a chosen model that is not in the list cannot answer (the providers say). */
function useWhy(ref: string | null, missing: boolean): string {
  const [why, setWhy] = useState("not available");
  useEffect(() => {
    if (!ref || !missing) return;
    api.get<{ providers: ProviderView[] }>("/api/providers").then((r) => setWhy(unusableWhy(ref, r.providers) ?? "not available"), () => {});
  }, [ref, missing]);
  return why;
}

interface BodyProps {
  sessionId: string | null;
  /** The menu closes after a pick; the sheet stays open. */
  onPicked?: () => void;
  /** Closes the menu (Manage models). */
  onClose?: () => void;
  onEscape?: () => void;
  /** The widget's sheet: the ring of a provider without its card. */
  ringOnly?: boolean;
}

/** The chooser, the effort and "Manage models" of the menu (picker.md section 5.2, chooser.md). */
export function ModelBody({ sessionId, onPicked, onClose, onEscape, ringOnly = false }: BodyProps) {
  const c = useChoice(sessionId);
  const why = useWhy(c.chosen, c.missing);
  const listId = useId();
  const [draft, setDraft] = useState<number | null>(null);

  const pick = async (ref: string | null) => {
    if (!sessionId) return;
    try {
      await setSessionModel(sessionId, ref === null ? { model: null, reasoningLevel: null } : { model: ref });
    } catch {
      // the store shows the refusal as a notice
    }
    onPicked?.();
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
      <ModelChooser
        models={c.models ?? []}
        value={c.chosen}
        focusRef={c.answering}
        none={{ label: "Default", hint: c.chatModel ? label(c.chatModel) : "none: pick a model" }}
        kept={c.missing && c.chosen ? { label: label(c.chosen), hint: why } : undefined}
        ring={ringOnly ? (id) => <UsageRingFor providerId={id} card={false} /> : undefined}
        onPick={(ref) => void pick(ref)}
        onEscape={onEscape}
      />
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
  useOverlay(open, () => setOpen(false));
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
        <UsageRingFor providerId={providerOfRef(c.answering)} up />
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
