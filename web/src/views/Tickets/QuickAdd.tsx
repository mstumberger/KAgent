// An input that makes a ticket in one state with Enter (tickets.md section 6).
import { useState } from "react";
import { errorMessage } from "./api";
import { STATE_LABEL } from "./model";
import type { TState } from "./api";

interface Props {
  state: TState;
  onAdd: (title: string, state: TState) => Promise<void>;
}

export default function QuickAdd({ state, onAdd }: Props) {
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const submit = async () => {
    const title = text.trim();
    if (!title) return;
    try {
      await onAdd(title, state);
      setText("");
      setError("");
    } catch (e) {
      setError(errorMessage(e));
    }
  };
  return (
    <div className="tk-quick">
      <input
        type="text"
        placeholder={`Add to ${STATE_LABEL[state].toLowerCase()}`}
        aria-label={`Add to ${STATE_LABEL[state]}`}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void submit();
          }
        }}
      />
      {error && <div className="tk-error" role="alert">{error}</div>}
    </div>
  );
}
