// The todo list of the open chat (plan-todo.md section 1): above the composer while the
// session has a list. A tick marks an item done and a second tick opens it again; both,
// and Clear, send the whole changed list to PUT /api/sessions/:id/todo.
import { useState } from "react";
import "./todo.css";
import { type TodoItem } from "../api";
import { saveTodo, useStore } from "../store";

/** The list after item `index` is ticked: done becomes pending, anything else done. */
function toggled(items: TodoItem[], index: number): TodoItem[] {
  return items.map((item, i) => (i === index ? { ...item, status: item.status === "completed" ? "pending" : "completed" } : item));
}

export default function TodoPanel({ sessionId }: { sessionId: string | null }) {
  const s = useStore();
  const items = sessionId ? (s.todos[sessionId] ?? []) : [];
  // One request at a time: a click waits for the answer of the one before, so every list is built on the stored one.
  const [busy, setBusy] = useState(false);
  if (!sessionId || items.length === 0) return null;
  const done = items.filter((item) => item.status === "completed").length;
  const send = async (next: TodoItem[]) => {
    setBusy(true);
    try {
      await saveTodo(sessionId, next);
    } catch {
      // The store shows why, as a notice in the header.
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="todo-panel" aria-label="Todo list" aria-busy={busy}>
      <div className="todo-head">
        <span className="todo-title">To do</span>
        <span className="todo-count">
          {done} of {items.length}
        </span>
        <button type="button" className="todo-clear" disabled={busy} onClick={() => void send([])}>
          Clear
        </button>
      </div>
      <ul className="todo-items">
        {items.map((item, i) => (
          <li key={i} className={`todo-item ${item.status}`}>
            <label className="todo-line">
              <input type="checkbox" checked={item.status === "completed"} disabled={busy} onChange={() => void send(toggled(items, i))} />
              <span className="todo-text">{item.content}</span>
              {item.status === "in_progress" && <span className="todo-state">in progress</span>}
              {item.priority === "high" && (
                <abbr className="todo-high" title="High priority">
                  !
                </abbr>
              )}
            </label>
          </li>
        ))}
      </ul>
    </section>
  );
}
