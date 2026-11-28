// The card of a provider's limits (usage.md section 7): each limit as a bar with its percentage or used / total,
// when it resets and where the number comes from.
import { amount, fraction, percent, resetText, sourceText, type Limit, type UsageProvider } from "../usage";
import "./usage.css";

/** One limit as a bar. */
export function LimitBar({ limit }: { limit: Limit }) {
  const f = fraction(limit);
  const reset = resetText(limit.resetsAt);
  return (
    <div className={`uc-limit uc-${limit.level}`} data-limit={limit.id}>
      <div className="uc-line">
        <span className="uc-name">{limit.name}</span>
        <span className="uc-amount">{amount(limit)}</span>
        <span className="uc-pct">{percent(limit)}</span>
      </div>
      <div className="uc-bar" role="progressbar" aria-label={limit.name} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(f * 100)}>
        <i style={{ width: `${Math.round(f * 100)}%` }} />
      </div>
      <div className="uc-meta">
        {reset ? `resets at ${reset}` : "no reset time known"} · {sourceText(limit)}
      </div>
    </div>
  );
}

export default function UsageCard({ provider }: { provider: UsageProvider }) {
  return (
    <span className="uc" role="group" aria-label={`${provider.name} limits`}>
      <span className="uc-title">{provider.name}</span>
      {provider.limits.length === 0 ? <span className="uc-none">{provider.local ? "Local: no limits." : "No limit known."}</span> : provider.limits.map((l) => <LimitBar key={l.id} limit={l} />)}
    </span>
  );
}

/** The thin bar of a provider row in Settings -> Models: its tightest limit and the percentage. */
export function UsageBar({ limit }: { limit: Limit | null | undefined }) {
  if (!limit) return null;
  const f = fraction(limit);
  return (
    <span className={`ur-bar uc-${limit.level}`} title={`${limit.name}: ${percent(limit)}`} data-level={limit.level}>
      <span className="uc-bar" aria-hidden="true">
        <i style={{ width: `${Math.round(f * 100)}%` }} />
      </span>
      <span className="ur-bar-pct">{percent(limit)}</span>
    </span>
  );
}
