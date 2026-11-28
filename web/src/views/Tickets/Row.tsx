// One ticket as a row of the list, or as a card of the board.
import type { KeyboardEvent } from "react";
import type { Ticket, TState } from "./api";
import { PRIORITY_LABEL, STATES } from "./model";
import MoveMenu from "./MoveMenu";

interface Props {
  ticket: Ticket;
  card?: boolean;
  selected: boolean;
  /** The project name is shown when more than one project is. */
  project?: string;
  onOpen: (key: string) => void;
  onMove: (key: string, state: TState) => void;
}

export default function Row({ ticket: t, card = false, selected, project, onOpen, onMove }: Props) {
  // `[` and `]` move the focused card or row to the previous or next state.
  const key = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onOpen(t.key);
    } else if (e.key === "[" || e.key === "]") {
      const at = STATES.indexOf(t.state) + (e.key === "]" ? 1 : -1);
      if (at >= 0 && at < STATES.length) onMove(t.key, STATES[at]);
    }
  };
  return (
    <div
      className={`tk-item${card ? " card" : " row"}${selected ? " selected" : ""} pr-${t.priority}`}
      data-key={t.key}
      data-state={t.state}
      role="button"
      tabIndex={0}
      aria-label={`${t.key} ${t.title}`}
      aria-pressed={selected}
      draggable={card}
      onDragStart={(e) => {
        e.dataTransfer.setData("text/plain", t.key);
        e.dataTransfer.effectAllowed = "move";
      }}
      onClick={() => onOpen(t.key)}
      onKeyDown={key}
    >
      <span className="tk-key">{t.key}</span>
      <span className="tk-title">{t.title}</span>
      {t.priority !== "none" && <span className={`tk-pri pr-${t.priority}`} title={PRIORITY_LABEL[t.priority]}>{PRIORITY_LABEL[t.priority]}</span>}
      {t.labels.map((l) => (
        <span key={l} className="tk-label">{l}</span>
      ))}
      {project && <span className="tk-project">{project}</span>}
      {t.assignee && <span className="tk-assignee" title="Assigned to the owner">me</span>}
      <MoveMenu ticket={t} onMove={onMove} />
    </div>
  );
}
