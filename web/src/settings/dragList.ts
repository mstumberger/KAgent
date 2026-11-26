// Rows the owner drags to reorder (models.md sections 7.1 and 7.4). A row is
// `draggable` while its handle is held; Up and Down on the handle move it for the
// keyboard. A .ts file, so the component files export components only.
import { useRef, useState, type DragEvent, type KeyboardEvent, type PointerEvent } from "react";

/** `keys`: the rows in their order now; `onReorder`: the whole order after a move. */
export function useDragList(keys: () => string[], onReorder: (order: string[]) => void) {
  const [armed, setArmed] = useState<string | null>(null);
  // The row a drag is over, and whether it drops after it.
  const [target, setTarget] = useState<{ key: string; after: boolean } | null>(null);
  const dragging = useRef<string | null>(null);

  const move = (from: string, to: string, after: boolean) => {
    const order = keys().filter((k) => k !== from);
    order.splice(order.indexOf(to) + (after ? 1 : 0), 0, from);
    onReorder(order);
  };
  const nudge = (key: string, step: -1 | 1) => {
    const order = keys();
    const i = order.indexOf(key);
    if (i < 0 || i + step < 0 || i + step >= order.length) return;
    [order[i], order[i + step]] = [order[i + step], order[i]];
    onReorder(order);
  };
  // The row is draggable from the press on its handle, and only then.
  const hold = (e: PointerEvent<HTMLButtonElement>, on: boolean) => {
    const row = e.currentTarget.parentElement;
    if (row) row.draggable = on;
    if (on) e.currentTarget.setPointerCapture(e.pointerId);
    setArmed(on ? (row?.dataset.key ?? null) : null);
  };
  const release = () => {
    dragging.current = null;
    setTarget(null);
    setArmed(null);
  };

  return {
    /** Spread on a row whose `data-key` is `key`; `extra` is the class of the row itself. A row that cannot be dragged (`on` false) gets only those. */
    row(key: string, extra = "", on = true) {
      if (!on) return { "data-key": key, className: extra };
      return {
        "data-key": key,
        draggable: armed === key,
        className: `${extra} draggable${armed === key ? " armed" : ""}${target?.key === key ? (target.after ? " drop-after" : " drop-before") : ""}`,
        onDragStart: (e: DragEvent<HTMLElement>) => {
          dragging.current = key;
          e.dataTransfer.effectAllowed = "move";
          e.dataTransfer.setData("text/plain", key);
        },
        onDragOver: (e: DragEvent<HTMLElement>) => {
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          const rect = e.currentTarget.getBoundingClientRect();
          const next = { key, after: e.clientY > rect.top + rect.height / 2 };
          if (dragging.current !== key && (target?.key !== next.key || target.after !== next.after)) setTarget(next);
        },
        onDrop: (e: DragEvent<HTMLElement>) => {
          e.preventDefault();
          const from = dragging.current;
          if (from && from !== key && target?.key === key) move(from, key, target.after);
          release();
        },
        onDragEnd: release,
      };
    },
    /** Spread on the handle button of a row. */
    handle(key: string, label: string) {
      return {
        type: "button" as const,
        className: `pv-grip${armed === key ? " held" : ""}`,
        title: "Drag to reorder, or press the up and down arrows",
        "aria-label": `Reorder ${label}`,
        onPointerDown: (e: PointerEvent<HTMLButtonElement>) => hold(e, true),
        // A drag cancels the pointer: `dragend` of the row releases it then.
        onPointerUp: (e: PointerEvent<HTMLButtonElement>) => hold(e, false),
        onKeyDown: (e: KeyboardEvent<HTMLButtonElement>) => {
          if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            nudge(key, e.key === "ArrowUp" ? -1 : 1);
          }
        },
      };
    },
  };
}
