// The model chooser (chooser.md): providers on the left, the models of one on the right, a search box and
// filter chips; one component for every place a model is picked. The search box keeps the focus, so typing
// searches; `aria-activedescendant` names the row under the keyboard cursor.
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import Icon from "../Icon";
import Chips from "./Chips";
import ModelColumn from "./ModelColumn";
import ProviderColumn from "./ProviderColumn";
import UsageRingFor from "../UsageRing";
import { RECENT, groupsOf, type ChipKey, type ChooserModel, type Group } from "./data";
import { pushRecent, readRecent } from "./recent";
import "../popup.css";
import "./chooser.css";

export interface ChooserNone {
  label: string;
  hint?: string;
}

/** A current value that the list does not hold: shown as a row that cannot be picked, with why. */
export interface ChooserKept {
  label: string;
  hint: string;
}

interface Props {
  /** The models this place offers (each place filters its own list). */
  models: ChooserModel[];
  /** The ref that is chosen now, or `null` for the default or none. */
  value: string | null;
  onPick: (ref: string | null) => void;
  /** A row above the columns that picks `null` ("Default", "none", "The chat model"). */
  none?: ChooserNone;
  kept?: ChooserKept;
  /** The ref whose provider is shown first when it opens (the answering model, when `value` is the default); `value` by default. */
  focusRef?: string | null;
  /** The limits ring of a provider: the ring and its card by default. */
  ring?: (providerId: string) => ReactNode;
  onEscape?: () => void;
  /** The focus goes to the search box when it opens. */
  autoFocus?: boolean;
}

type Col = "none" | "p" | "m";

export default function ModelChooser({ models, value, focusRef, onPick, none, kept, ring = (providerId) => <UsageRingFor providerId={providerId} />, onEscape, autoFocus = true }: Props) {
  const uid = useId();
  const input = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [chips, setChips] = useState<ReadonlySet<ChipKey>>(new Set());
  const [recent] = useState(readRecent);
  const groups = useMemo(() => groupsOf(models, query, chips, recent), [models, query, chips, recent]);
  const start = focusRef ?? value;
  const [activeId, setActive] = useState(() => models.find((m) => m.ref === start)?.providerId ?? "");
  const active: Group | undefined = groups.find((g) => g.id === activeId) ?? groups.find((g) => g.id !== RECENT) ?? groups[0];
  const [col, setCol] = useState<Col>("p");
  const [mi, setMi] = useState(() => {
    const m = models.find((x) => x.ref === start);
    return m ? models.filter((x) => x.providerId === m.providerId).findIndex((x) => x.ref === start) : 0;
  });
  const at = Math.min(mi, Math.max((active?.models.length ?? 1) - 1, 0));

  useEffect(() => {
    if (autoFocus) input.current?.focus();
  }, [autoFocus]);

  const providerRow = (g: Group) => `${uid}-p-${g.id}`;
  const modelRow = (m: ChooserModel) => `${uid}-m-${m.ref}`;
  const noneRow = `${uid}-none`;
  const activeRow = col === "none" ? noneRow : col === "p" ? (active ? providerRow(active) : undefined) : active?.models[at] ? modelRow(active.models[at]) : undefined;

  const select = (id: string) => {
    setActive(id);
    const g = groups.find((x) => x.id === id);
    const i = g?.models.findIndex((m) => m.ref === value) ?? -1;
    setMi(i >= 0 ? i : 0);
  };
  const pick = (ref: string | null) => {
    if (ref) pushRecent(ref);
    onPick(ref);
  };
  const moveProvider = (dir: 1 | -1) => {
    const i = groups.findIndex((g) => g.id === active?.id);
    const next = groups[i + dir];
    if (next) select(next.id);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    const el = e.currentTarget;
    const atEnd = el.selectionStart === el.value.length && el.selectionEnd === el.value.length;
    const count = active?.models.length ?? 0;
    let handled = true;
    switch (e.key) {
      case "ArrowDown":
        if (col === "none") setCol("p");
        else if (col === "p") moveProvider(1);
        else setMi(Math.min(at + 1, count - 1));
        break;
      case "ArrowUp":
        if (col === "p") {
          if (groups.findIndex((g) => g.id === active?.id) === 0) {
            if (none) setCol("none");
          } else moveProvider(-1);
        } else if (col === "m") {
          if (at === 0) {
            if (none) setCol("none");
          } else setMi(at - 1);
        }
        break;
      case "ArrowRight":
        if (col !== "m" && atEnd && count > 0) {
          setCol("m");
          if (active?.models.every((m) => m.ref !== active.models[at]?.ref)) setMi(0);
        } else handled = false;
        break;
      case "ArrowLeft":
        if (col === "m") setCol("p");
        else handled = false;
        break;
      case "Home":
        if (col === "m") setMi(0);
        else if (col === "p" && groups[0]) select(groups[0].id);
        else handled = false;
        break;
      case "End":
        if (col === "m") setMi(Math.max(count - 1, 0));
        else if (col === "p" && groups.length) select(groups[groups.length - 1].id);
        else handled = false;
        break;
      case "Enter":
        if (col === "none") pick(null);
        else if (col === "p") {
          if (count > 0) setCol("m");
        } else if (active?.models[at]) pick(active.models[at].ref);
        break;
      case "Escape":
        if (onEscape) {
          e.preventDefault();
          e.stopPropagation();
          onEscape();
        }
        return;
      default:
        handled = false;
    }
    if (handled) e.preventDefault();
  };

  // The row under the cursor is scrolled into view inside its column.
  useEffect(() => {
    if (activeRow) document.getElementById(activeRow)?.scrollIntoView?.({ block: "nearest" });
  }, [activeRow]);

  return (
    <div className="mc" role="group" aria-label="Choose a model">
      <div className="mc-search">
        <Icon name="search" size={14} />
        <input
          ref={input}
          type="search"
          role="combobox"
          aria-label="Search models"
          aria-expanded="true"
          aria-controls={`${uid}-cols`}
          aria-activedescendant={activeRow}
          placeholder="Search models, providers, free, vision"
          value={query}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => {
            setQuery(e.target.value);
            setMi(0);
          }}
          onKeyDown={onKeyDown}
        />
      </div>
      <Chips
        on={chips}
        onToggle={(chip) =>
          setChips((prev) => {
            const next = new Set(prev);
            if (!next.delete(chip)) next.add(chip);
            return next;
          })
        }
      />
      {kept && (
        <div className="pop-item off mc-kept" role="note">
          <span className="mm-label">
            <span className="pop-text">{kept.label}</span>
            <span className="pop-hint pop-text">{kept.hint}</span>
          </span>
          <span className="pop-check">
            <Icon name="check" size={16} />
          </span>
        </div>
      )}
      {none && (
        <div
          id={noneRow}
          role="option"
          aria-selected={value === null && !kept}
          data-none="true"
          className={`pop-item mc-none${col === "none" ? " cursor" : ""}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => pick(null)}
        >
          <span className="mm-label">
            <span className="pop-text">{none.label}</span>
            {none.hint && <span className="pop-hint pop-text">{none.hint}</span>}
          </span>
          {value === null && !kept && (
            <span className="pop-check">
              <Icon name="check" size={16} />
            </span>
          )}
        </div>
      )}
      <div className="mc-cols" id={`${uid}-cols`}>
        <ProviderColumn
          groups={groups}
          active={active?.id ?? ""}
          cursor={col === "p"}
          rowId={providerRow}
          ring={ring}
          onSelect={(id) => {
            select(id);
            setCol("p");
          }}
        />
        <ModelColumn
          models={active?.models ?? []}
          value={value}
          cursor={at}
          focused={col === "m"}
          withProvider={active?.id === RECENT}
          rowId={modelRow}
          onHover={(i) => {
            setMi(i);
            setCol("m");
          }}
          onPick={pick}
        />
      </div>
    </div>
  );
}
