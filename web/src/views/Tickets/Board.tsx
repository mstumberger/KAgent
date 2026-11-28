// A column per state; a card is dragged to another column to change its state (tickets.md section 6).
import { useState } from "react";
import type { Ticket, TState } from "./api";
import { STATES, STATE_LABEL, byState } from "./model";
import QuickAdd from "./QuickAdd";
import Row from "./Row";

interface Props {
  tickets: Ticket[];
  selected: string | null;
  projectName: (t: Ticket) => string | undefined;
  onOpen: (key: string) => void;
  onMove: (key: string, state: TState) => void;
  onAdd: (title: string, state: TState) => Promise<void>;
}

export default function Board({ tickets, selected, projectName, onOpen, onMove, onAdd }: Props) {
  const groups = byState(tickets);
  const [over, setOver] = useState<TState | null>(null);
  return (
    <div className="tk-board">
      {STATES.map((s) => (
        <section
          key={s}
          className={`tk-column st-${s}${over === s ? " over" : ""}`}
          data-state={s}
          aria-label={STATE_LABEL[s]}
          onDragOver={(e) => {
            e.preventDefault();
            e.dataTransfer.dropEffect = "move";
            if (over !== s) setOver(s);
          }}
          onDragLeave={(e) => {
            if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(null);
          }}
          onDrop={(e) => {
            e.preventDefault();
            setOver(null);
            const key = e.dataTransfer.getData("text/plain");
            if (key) onMove(key, s);
          }}
        >
          <h3>
            <span>{STATE_LABEL[s]}</span>
            <span className="tk-count">{groups[s].length}</span>
          </h3>
          <QuickAdd state={s} onAdd={onAdd} />
          <div className="tk-cards">
            {groups[s].map((t) => (
              <Row key={t.key} ticket={t} card selected={selected === t.key} project={projectName(t)} onOpen={onOpen} onMove={onMove} />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
