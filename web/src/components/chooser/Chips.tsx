// The filter chips of the chooser: on or off, and combined (chooser.md section 3).
import { CHIPS, type ChipKey } from "./data";

export default function Chips({ on, onToggle }: { on: ReadonlySet<ChipKey>; onToggle: (chip: ChipKey) => void }) {
  return (
    <div className="mc-chips" role="group" aria-label="Filters">
      {CHIPS.map((c) => (
        <button
          key={c.key}
          type="button"
          className={`mc-chip${on.has(c.key) ? " on" : ""}`}
          data-chip={c.key}
          aria-pressed={on.has(c.key)}
          // The focus stays in the search box, so typing goes on.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onToggle(c.key)}
        >
          {c.label}
        </button>
      ))}
    </div>
  );
}
