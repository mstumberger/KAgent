// The list grouped by state (tickets.md section 6).
import { useState } from "react";
import Icon from "../../components/Icon";
import type { Ticket, TState } from "./api";
import { LIST_ORDER, STATE_LABEL, byState } from "./model";
import QuickAdd from "./QuickAdd";
import Row from "./Row";

interface Props {
  tickets: Ticket[];
  /** Only these states are shown (Mine now). */
  only?: TState[];
  selected: string | null;
  projectName: (t: Ticket) => string | undefined;
  onOpen: (key: string) => void;
  onMove: (key: string, state: TState) => void;
  onAdd: (title: string, state: TState) => Promise<void>;
}

const CLOSED_BY_DEFAULT: TState[] = ["done", "dropped"];

export default function List({ tickets, only, selected, projectName, onOpen, onMove, onAdd }: Props) {
  const groups = byState(tickets);
  const [folded, setFolded] = useState<Record<string, boolean>>({});
  const states = LIST_ORDER.filter((s) => !only || only.includes(s));
  return (
    <div className="tk-list">
      {states.map((s) => {
        const items = groups[s];
        const isFolded = folded[s] ?? CLOSED_BY_DEFAULT.includes(s);
        return (
          <section key={s} className={`tk-section st-${s}`} data-state={s} aria-label={STATE_LABEL[s]}>
            <h3>
              <button type="button" className="tk-fold" aria-expanded={!isFolded} onClick={() => setFolded({ ...folded, [s]: !isFolded })}>
                <Icon name="chevron" size={13} />
                <span>{STATE_LABEL[s]}</span>
                <span className="tk-count">{items.length}</span>
              </button>
            </h3>
            {!isFolded && (
              <>
                {!only && <QuickAdd state={s} onAdd={onAdd} />}
                {items.map((t) => (
                  <Row key={t.key} ticket={t} selected={selected === t.key} project={projectName(t)} onOpen={onOpen} onMove={onMove} />
                ))}
                {items.length === 0 && only && <div className="tk-none">Nothing {STATE_LABEL[s].toLowerCase()}.</div>}
              </>
            )}
          </section>
        );
      })}
    </div>
  );
}
