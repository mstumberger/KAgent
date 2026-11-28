// The chip of a running screen watch (screen-watch.md section 4): an eye, the minutes left and, on a click,
// what it is looking at with Stop. It shows whenever the daemon says a watch runs, in every view and in the
// widget: nothing here can hide it, and it has no close button.
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import Icon from "./Icon";
import { minutesLeft, refreshWatch, stopWatch, whatOf, type Watch } from "../screenWatch";
import { useStore } from "../store";
import "./watch.css";

/** The page reads the daemon again this often, so a missed event cannot leave a chip behind or hide a watch. */
const REREAD_MS = 30_000;

/** The clock, to the second: a chip that appears is right at once, and counts down from there. */
function useSecond(): number {
  return useSyncExternalStore(
    (notify) => {
      const t = setInterval(notify, 1000);
      return () => clearInterval(t);
    },
    () => Math.floor(Date.now() / 1000) * 1000,
  );
}

/** Nothing renders, and no clock runs, while no watch does. */
export default function WatchChip({ compact = false }: { compact?: boolean }) {
  const w = useStore().watch;
  return w ? <Chip w={w} compact={compact} /> : null;
}

function Chip({ w, compact }: { w: Watch; compact: boolean }) {
  const now = useSecond();
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const reread = setInterval(() => void refreshWatch(), REREAD_MS);
    return () => clearInterval(reread);
  }, []);

  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  const left = minutesLeft(w, now);
  const label = w.paused ? "paused" : `${left} min`;
  const ends = new Date(w.until).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return (
    <span ref={box} className={`watch-chip${w.paused ? " paused" : ""}${compact ? " compact" : ""}`} data-state={w.paused ? "paused" : "watching"}>
      <button
        className="watch-btn"
        title={w.goal}
        aria-label={`Screen watch: ${w.paused ? "paused, the screen is locked" : `${left} minutes left`}. ${w.goal}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={() => setOpen(!open)}
      >
        <Icon name="eye" size={14} />
        <span className="watch-min">{label}</span>
      </button>
      {open && (
        <div className="watch-pop pop" role="dialog" aria-label="Screen watch" onPointerDown={(e) => e.stopPropagation()}>
          <p className="watch-goal">{w.goal}</p>
          <dl>
            <dt>Looking at</dt>
            <dd>{whatOf(w)}</dd>
            <dt>Last note</dt>
            <dd>{w.lastNote || "nothing yet"}</dd>
            <dt>Looks</dt>
            <dd>
              {w.looks}, one every {w.everySec} s{w.paused ? " (paused: the screen is locked)" : ""}
            </dd>
            <dt>Ends</dt>
            <dd>
              {ends}, {w.mode === "repeat" ? "reports every new hit" : "at the first hit"}
            </dd>
          </dl>
          <button
            className="danger watch-stop"
            onClick={() => {
              setOpen(false);
              void stopWatch();
            }}
          >
            Stop
          </button>
        </div>
      )}
    </span>
  );
}
