// The left column of the chooser: the providers (chooser.md section 3).
import type { ReactNode } from "react";
import type { Group } from "./data";

interface Props {
  groups: Group[];
  active: string;
  /** The keyboard cursor is in this column. */
  cursor: boolean;
  rowId: (group: Group) => string;
  onSelect: (id: string) => void;
  /** The limits ring of a provider (`usage.md` section 7): nothing for "Recent" or a provider without a limit. */
  ring?: (providerId: string) => ReactNode;
}

export default function ProviderColumn({ groups, active, cursor, rowId, onSelect, ring }: Props) {
  return (
    <div className="mc-providers" role="listbox" aria-label="Providers">
      {groups.map((g) => (
        <div
          key={g.id}
          id={rowId(g)}
          role="option"
          aria-selected={g.id === active}
          data-provider={g.id}
          className={`pop-item mc-provider${g.id === active ? " active" : ""}${g.id === active && cursor ? " cursor" : ""}`}
          // The search box keeps the focus.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onSelect(g.id)}
        >
          <span className="mc-pname pop-text">{g.name}</span>
          {ring?.(g.id)}
          {g.local && <span className="mc-tag local">local</span>}
          {g.free && g.id !== "__recent" && <span className="mc-tag free">free</span>}
          <span className="mc-count" title="Models">
            {g.models.length}
          </span>
        </div>
      ))}
    </div>
  );
}
