// The right column of the chooser: the models of the active provider (chooser.md section 3).
import Icon from "../Icon";
import { contextLabel, modelLabel, type ChooserModel } from "./data";

interface Props {
  models: ChooserModel[];
  /** The ref that is chosen now. */
  value: string | null;
  cursor: number;
  /** The keyboard cursor is in this column. */
  focused: boolean;
  /** Shows the provider on each row (the Recent list mixes providers). */
  withProvider: boolean;
  rowId: (m: ChooserModel) => string;
  onHover: (index: number) => void;
  onPick: (ref: string) => void;
}

export default function ModelColumn({ models, value, cursor, focused, withProvider, rowId, onHover, onPick }: Props) {
  if (models.length === 0) return <div className="mc-models mc-empty">No model matches.</div>;
  return (
    <div className="mc-models" role="listbox" aria-label="Models">
      {models.map((m, i) => {
        const levels = m.config.reasoningLevels;
        return (
          <div
            key={m.ref}
            id={rowId(m)}
            role="option"
            aria-selected={m.ref === value}
            data-ref={m.ref}
            className={`pop-item mc-model${i === cursor && focused ? " cursor" : ""}${i === cursor ? " at" : ""}`}
            onMouseDown={(e) => e.preventDefault()}
            onMouseEnter={() => onHover(i)}
            onClick={() => onPick(m.ref)}
          >
            <span className="mc-mname pop-text">
              {modelLabel(m)}
              {withProvider && <span className="pop-hint mc-from"> {m.providerName}</span>}
            </span>
            {m.free && <span className="mc-tag free">free</span>}
            {m.config.supportsImage && (
              <span className="mm-image" title="Reads images">
                <Icon name="image" size={13} />
              </span>
            )}
            {levels.length > 0 && (
              <span className="mc-levels" title={`Effort: ${levels.join(", ")}`}>
                {levels.length === 1 ? levels[0] : `${levels.length} levels`}
              </span>
            )}
            <span className="mc-ctx" title="Context window">
              {contextLabel(m.config.contextWindow)}
            </span>
            {m.ref === value && (
              <span className="pop-check">
                <Icon name="check" size={16} />
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
}
