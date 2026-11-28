// A themed dropdown (models.md section 7.8): a button that opens a list of options.
// The popup of a native select is drawn by the browser and stays light in Brave on
// Linux, also in the dark theme; this one carries the colours of the page. The focus
// stays on the trigger while the list is open; the cursor is its `aria-activedescendant`.
import { useEffect, useId, useLayoutEffect, isValidElement, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import "../components/popup.css";
import "./dropdown.css";
import { useOverlay } from "../router/overlays";

export interface DropdownOption {
  value: string;
  label: ReactNode;
  /** A dim second line. */
  hint?: ReactNode;
  disabled?: boolean;
}

interface Props {
  /** Names the trigger and the list, unless a `<label htmlFor>` does. */
  ariaLabel?: string;
  value: string;
  options: DropdownOption[];
  onChange: (value: string) => void;
  /** Shown in the dim colour when the value is empty or names no option. */
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  /** So that a `<label htmlFor>` can point at the trigger. */
  id?: string;
}

/** The text of a label, for the letters that are typed. */
function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement(node)) return textOf((node.props as { children?: ReactNode }).children);
  return "";
}

const ROW = 34;
const LIST = 280;

export default function Dropdown({ ariaLabel, value, options, onChange, placeholder, disabled = false, className, id }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const listId = useId();
  const optionId = (i: number) => `${listId}-${i}`;
  const [open, setOpen] = useState(false);
  const [cursor, setCursor] = useState(0);
  // Where the list opens: under the trigger, or above it when there is no room below.
  const [above, setAbove] = useState(false);
  const [alignRight, setAlignRight] = useState(false);
  const [space, setSpace] = useState(LIST);
  // The letters typed so far jump to the first label that starts with them.
  const typed = useRef("");
  const typedTimer = useRef<number | undefined>(undefined);

  const enabled = (from: number, step: 1 | -1 = 1) => {
    for (let n = 0; n < options.length; n++) {
      const i = (from + n * step + options.length * 2) % options.length;
      if (!options[i].disabled) return i;
    }
    return from;
  };
  const current = options.findIndex((o) => o.value === value);

  const openList = (fromEnd = false) => {
    const rect = trigger.current?.getBoundingClientRect();
    const below = rect ? window.innerHeight - rect.bottom - 8 : window.innerHeight;
    const over = rect ? rect.top - 8 : 0;
    // Above when the list does not fit below and there is more room above.
    const up = Math.min(LIST, options.length * ROW + 8) > below && over > below;
    setAbove(up);
    setSpace(up ? over : below);
    setAlignRight(!!rect && rect.left + 420 > window.innerWidth - 16);
    setCursor(current >= 0 && !options[current].disabled ? current : enabled(fromEnd ? options.length - 1 : 0, fromEnd ? -1 : 1));
    typed.current = "";
    setOpen(true);
  };
  const close = () => setOpen(false);
  useOverlay(open, close);
  const pick = (i: number) => {
    const option = options[i];
    if (!option || option.disabled) return;
    setOpen(false);
    onChange(option.value);
  };
  /** The first enabled label after `from` that starts with the letters typed. */
  const jump = (letter: string, from: number) => {
    typed.current += letter.toLowerCase();
    window.clearTimeout(typedTimer.current);
    typedTimer.current = window.setTimeout(() => {
      typed.current = "";
    }, 700);
    for (let n = 1; n <= options.length; n++) {
      const i = (from + n + options.length) % options.length;
      if (!options[i].disabled && textOf(options[i].label).toLowerCase().startsWith(typed.current)) {
        setCursor(i);
        return;
      }
    }
  };

  // The cursor's row is scrolled into view: only the list itself is scrolled.
  useLayoutEffect(() => {
    if (!open) return;
    const box = list.current;
    const row = box?.children[cursor] as HTMLElement | undefined;
    if (!box || !row) return;
    if (row.offsetTop < box.scrollTop) box.scrollTop = row.offsetTop;
    else if (row.offsetTop + row.offsetHeight > box.scrollTop + box.clientHeight) box.scrollTop = row.offsetTop + row.offsetHeight - box.clientHeight;
  }, [open, cursor]);

  // A click outside, a resize or a scroll of the page closes the list. The list's own
  // scroll does not.
  useEffect(() => {
    if (!open) return;
    const outside = (e: Event) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", outside);
    window.addEventListener("resize", close, true);
    window.addEventListener("scroll", outside, true);
    return () => {
      document.removeEventListener("mousedown", outside);
      window.removeEventListener("resize", close, true);
      window.removeEventListener("scroll", outside, true);
    };
  }, [open]);

  useEffect(() => () => window.clearTimeout(typedTimer.current), []);

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>) => {
    const letter = e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey;
    if (!open) {
      if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openList();
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        openList(true);
      } else if (letter) {
        openList();
        jump(e.key, current);
      }
      return;
    }
    if (e.key === "Escape") {
      // The focus is on the trigger and stays there.
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "Tab") {
      close();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor(enabled(cursor + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor(enabled(cursor - 1, -1));
    } else if (e.key === "Home") {
      e.preventDefault();
      setCursor(enabled(0));
    } else if (e.key === "End") {
      e.preventDefault();
      setCursor(enabled(options.length - 1, -1));
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      pick(cursor);
    } else if (letter) {
      jump(e.key, cursor);
    }
  };

  return (
    <div ref={root} className={`dd${className ? ` ${className}` : ""}${open ? " open" : ""}${disabled ? " off" : ""}`}>
      <button
        ref={trigger}
        type="button"
        id={id}
        className="dd-trigger"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? optionId(cursor) : undefined}
        disabled={disabled}
        onKeyDown={onKeyDown}
        // Enter and Space are keys above (a click they cause has no pointer: `detail` 0).
        onClick={(e) => {
          if (e.detail === 0) return;
          if (open) close();
          else openList();
        }}
      >
        <span className={`dd-value${current >= 0 ? "" : " dd-placeholder"}`}>{current >= 0 ? options[current].label : (placeholder ?? "\u00a0")}</span>
        <svg className="dd-chevron" width={14} height={14} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
          <path d="M6 9l6 6 6-6" />
        </svg>
      </button>
      {open && (
        <ul
          ref={list}
          id={listId}
          role="listbox"
          aria-label={ariaLabel}
          className={`pop dd-list${above ? " above" : ""}${alignRight ? " right" : ""}`}
          style={{ "--dd-space": `${Math.max(space, ROW * 2)}px` } as CSSProperties}
          // A press on a row must not take the focus from the trigger.
          onMouseDown={(e) => e.preventDefault()}
        >
          {options.map((o, i) => (
            <li
              key={optionId(i)}
              id={optionId(i)}
              role="option"
              aria-selected={o.value === value}
              aria-disabled={o.disabled || undefined}
              className={`pop-item dd-option${i === cursor ? " cursor" : ""}${o.disabled ? " off" : ""}`}
              onMouseEnter={() => {
                if (!o.disabled) setCursor(i);
              }}
              onClick={() => pick(i)}
            >
              <span className="dd-option-text">
                {o.label}
                {o.hint !== undefined && <span className="pop-hint dd-hint">{o.hint}</span>}
              </span>
              {o.value === value && (
                <svg className="pop-check dd-check" width={16} height={16} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M20 6 9 17l-5-5" />
                </svg>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
