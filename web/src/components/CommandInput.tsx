// The composer's input with the slash picker (skills.md section 6): while the text is one
// word that starts with `/`, the matching skills and built-in commands open above it.
import { useId, useState, type KeyboardEvent } from "react";
import type { SkillView } from "../api";
import { matchSlash, slashItems, slashWord, type SlashItem } from "../commands";
import "./command.css";

interface Props {
  value: string;
  skills: SkillView[];
  onChange: (text: string) => void;
  /** A picked item, as `/<name>` and a space: ready for the request. */
  onPick: (text: string) => void;
  placeholder?: string;
}

export default function CommandInput({ value, skills, onChange, onPick, placeholder }: Props) {
  const listId = useId();
  // The highlighted item of the word it was moved in: a new word starts at the top.
  const [cursor, setCursor] = useState<{ word: string; index: number }>({ word: "", index: 0 });
  // The word Escape closed the picker for; it stays closed until the word changes.
  const [dismissed, setDismissed] = useState<string | null>(null);
  const query = slashWord(value);
  const items = query === null ? [] : matchSlash(slashItems(skills), query);
  const active = cursor.word === query ? Math.min(cursor.index, Math.max(items.length - 1, 0)) : 0;
  const setActive = (index: number) => setCursor({ word: query ?? "", index });
  const open = query !== null && dismissed !== query && items.length > 0;

  const pick = (item: SlashItem | undefined) => {
    if (item && !item.comesWith) onPick(`/${item.name} `);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (!open) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      setActive((active + (e.key === "ArrowDown" ? 1 : -1) + items.length) % items.length);
    } else if (e.key === "Enter" || e.key === "Tab") {
      // Enter picks instead of sending; a second Enter sends.
      e.preventDefault();
      pick(items[active]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setDismissed(query);
    }
  };

  return (
    <div className="cmd">
      <input
        value={value}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        aria-label="Message"
        role="combobox"
        aria-autocomplete="list"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-activedescendant={open ? `${listId}-${active}` : undefined}
      />
      {open && (
        <ul id={listId} className="cmd-list" role="listbox" aria-label="Skills and commands">
          {items.map((item, i) => (
            <li
              key={`${item.skill}-${item.name}`}
              id={`${listId}-${i}`}
              role="option"
              aria-selected={i === active}
              className={`cmd-item${item.comesWith ? " later" : ""}`}
              title={item.comesWith ? `Comes with stage ${item.comesWith}` : undefined}
              onMouseEnter={() => setActive(i)}
              onMouseDown={(e) => {
                // The input keeps the focus.
                e.preventDefault();
                pick(item);
              }}
            >
              <span className="cmd-name">
                <code>/{item.name}</code>
                <span className="cmd-kind">{item.skill ? "skill" : "command"}</span>
                {item.comesWith && <span className="cmd-kind">from {item.comesWith}</span>}
              </span>
              <span className="cmd-desc">{item.description}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
