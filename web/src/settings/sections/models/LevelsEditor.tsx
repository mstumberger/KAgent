// The reasoning levels of the model dialog (models.md section 7.5): an ordered list
// with add, remove, up and down.
import { useState } from "react";
import Icon from "../../../components/Icon";

export default function LevelsEditor({ levels, onChange }: { levels: string[]; onChange: (levels: string[]) => void }) {
  const [adding, setAdding] = useState("");
  const swap = (i: number, j: number) => onChange(levels.map((l, k) => (k === i ? levels[j] : k === j ? levels[i] : l)));
  const add = () => {
    if (adding.trim()) {
      onChange([...levels, adding.trim()]);
      setAdding("");
    }
  };
  return (
    <>
      <ol className="md-levels">
        {levels.map((name, i) => (
          <li key={i} className="md-level">
            <input aria-label={`Level ${i + 1}`} value={name} onChange={(e) => onChange(levels.map((l, j) => (j === i ? e.target.value : l)))} />
            <button type="button" className="pv-mini" aria-label={`Move ${name} up`} disabled={i === 0} onClick={() => swap(i, i - 1)}>
              Up
            </button>
            <button type="button" className="pv-mini" aria-label={`Move ${name} down`} disabled={i === levels.length - 1} onClick={() => swap(i, i + 1)}>
              Down
            </button>
            <button type="button" className="pv-mini danger" aria-label={`Remove ${name}`} onClick={() => onChange(levels.filter((_, j) => j !== i))}>
              <Icon name="trash" size={13} />
            </button>
          </li>
        ))}
      </ol>
      <div className="md-add">
        <input
          placeholder="Add a level"
          aria-label="New level"
          value={adding}
          onChange={(e) => setAdding(e.target.value)}
          onKeyDown={(e) => {
            // Enter adds the level; it does not save the dialog.
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
        />
        <button type="button" onClick={add}>
          Add
        </button>
      </div>
    </>
  );
}
