// The limits ring (usage.md section 7): a thin ring that fills and changes colour with the tightest limit of a provider;
// nothing when the provider has no limit. A click or a hover on it opens the card with every limit.
import { useEffect, useId, useRef, useState } from "react";
import { fraction, usageOf, useUsage, type Limit } from "../usage";
import UsageCard from "./UsageCard";
import "./usage.css";

const R = 6.5;
const C = 2 * Math.PI * R;

/** The ring of one limit: no card. */
export function UsageRing({ limit, size = 16 }: { limit: Limit | null; size?: number }) {
  if (!limit) return null;
  const f = fraction(limit);
  return (
    <svg className={`ur ur-${limit.level}`} width={size} height={size} viewBox="0 0 16 16" role="img" aria-label={`${limit.name}: ${Math.round(f * 100)} %`} data-level={limit.level} data-fraction={f.toFixed(2)}>
      <circle className="ur-track" cx="8" cy="8" r={R} fill="none" strokeWidth="2" />
      <circle className="ur-fill" cx="8" cy="8" r={R} fill="none" strokeWidth="2" strokeLinecap="round" strokeDasharray={C} strokeDashoffset={C * (1 - f)} transform="rotate(-90 8 8)" />
    </svg>
  );
}

/** The ring of a provider, read from the usage the window holds, with its card on hover and click (unless `card` is false: the widget). */
export default function UsageRingFor({ providerId, card = true, up = false }: { providerId: string; card?: boolean; up?: boolean }) {
  const usage = usageOf(useUsage(), providerId);
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  const id = useId();

  useEffect(() => {
    if (!open) return;
    const outside = (e: Event) => {
      if (!root.current?.contains(e.target as Node)) {
        setOpen(false);
        setPinned(false);
      }
    };
    document.addEventListener("mousedown", outside);
    return () => document.removeEventListener("mousedown", outside);
  }, [open]);

  if (!usage?.tightest) return null;
  if (!card) return <UsageRing limit={usage.tightest} />;
  return (
    <span
      ref={root}
      className="ur-wrap"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => {
        if (!pinned) setOpen(false);
      }}
      onClick={(e) => {
        // The click is the ring's: it must not pick the provider or open the menu under it.
        e.stopPropagation();
        setPinned(!pinned);
        setOpen(!pinned);
      }}
      aria-describedby={open ? id : undefined}
    >
      <UsageRing limit={usage.tightest} />
      {open && (
        <span id={id} className={`ur-card${up ? " up" : ""}`} role="tooltip">
          <UsageCard provider={usage} />
        </span>
      )}
    </span>
  );
}
