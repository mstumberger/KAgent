// The things that sit over a page and close on Back (shell.md section 2.1): a dialog, a menu, a popup, a sheet, the
// drawer. `useOverlay(open, close)` registers one; Back closes the topmost and the place does not change.
import { useEffect, useRef } from "react";

interface Overlay {
  id: number;
  close: () => void;
}

const stack: Overlay[] = [];
let next = 1;

/** Close the topmost overlay; false when none is open. */
export function closeTopOverlay(): boolean {
  const top = stack.pop();
  if (!top) return false;
  top.close();
  return true;
}

/** Close every overlay (the browser's own Back, which changes the place at the same time). */
export function closeAllOverlays() {
  while (closeTopOverlay()) {
    // each close runs once
  }
}

export const overlayCount = () => stack.length;

/** While `open`, Back closes this overlay by calling `close`. */
export function useOverlay(open: boolean, close: () => void) {
  const latest = useRef(close);
  useEffect(() => {
    latest.current = close;
  });
  useEffect(() => {
    if (!open) return;
    const o: Overlay = { id: next++, close: () => latest.current() };
    stack.push(o);
    return () => {
      const at = stack.indexOf(o);
      if (at >= 0) stack.splice(at, 1);
    };
  }, [open]);
}
