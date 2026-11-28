// "Move to": the keyboard way to change the state of a card (tickets.md section 6).
import { useRef, useState } from "react";
import Menu from "../../components/Menu";
import Icon from "../../components/Icon";
import type { Ticket, TState } from "./api";
import { STATES, STATE_LABEL } from "./model";

interface Props {
  ticket: Ticket;
  onMove: (key: string, state: TState) => void;
}

export default function MoveMenu({ ticket, onMove }: Props) {
  const button = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<DOMRect | null>(null);
  return (
    <>
      <button
        ref={button}
        type="button"
        className="tk-move"
        aria-label={`Move ${ticket.key}`}
        aria-haspopup="menu"
        aria-expanded={anchor !== null}
        title="Move to"
        onClick={(e) => {
          e.stopPropagation();
          setAnchor(button.current?.getBoundingClientRect() ?? null);
        }}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <Icon name="chevron" size={13} />
      </button>
      {anchor && (
        <Menu
          label={`Move ${ticket.key} to`}
          anchor={anchor}
          items={STATES.map((s) => ({ id: s, label: STATE_LABEL[s], checked: s === ticket.state }))}
          onPick={(id) => {
            setAnchor(null);
            if (id !== ticket.state) onMove(ticket.key, id as TState);
          }}
          onClose={(restore) => {
            setAnchor(null);
            if (restore) button.current?.focus();
          }}
        />
      )}
    </>
  );
}
