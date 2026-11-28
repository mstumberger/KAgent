// The "Usage" block of a provider card (usage.md section 7): its limits, what went through KAgent today and this month
// per model, when a limit was last seen, and the limits the owner types (counted by KAgent, never prefilled).
import { useEffect, useState } from "react";
import type { ProviderView } from "../../../api";
import { LimitBar } from "../../../components/UsageCard";
import { count, resetText, usageOf, useUsage, type KnownLimit, type Totals } from "../../../usage";
import { readUsage } from "../../../usageLine";
import { useSettingPatch } from "../../useSettingPatch";
import Select from "../../Select";

const WINDOWS: KnownLimit["window"][] = ["minute", "day", "month"];
const UNITS: KnownLimit["unit"][] = ["requests", "tokens"];

const tokens = (t: Totals) => `${count(t.inputTokens)} in, ${count(t.outputTokens)} out`;

/** The rows of the known limits: edited as text, saved with one PUT. */
function KnownEditor({ providerId, saved }: { providerId: string; saved: KnownLimit[] }) {
  const { save, busy } = useSettingPatch();
  const [rows, setRows] = useState<{ window: KnownLimit["window"]; unit: KnownLimit["unit"]; total: string }[]>(() => saved.map((k) => ({ ...k, total: String(k.total) })));
  const parsed = rows.map((r) => ({ ...r, n: Number(r.total) }));
  const bad = parsed.some((r) => !Number.isInteger(r.n) || r.n < 1) || new Set(rows.map((r) => `${r.window}/${r.unit}`)).size !== rows.length;
  const dirty = JSON.stringify(rows.map((r) => [r.window, r.unit, r.total])) !== JSON.stringify(saved.map((k) => [k.window, k.unit, String(k.total)]));
  const set = (i: number, patch: Partial<(typeof rows)[number]>) => setRows((rs) => rs.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <div className="ub-known" role="group" aria-label="Known limits">
      <span className="ui-field-label">Known limits</span>
      <span className="hint">A limit you know (a free tier, a plan). KAgent counts its own requests and tokens against it; other apps using the same key are not included.</span>
      {rows.map((r, i) => (
        <div className="ub-known-row" key={i}>
          <input type="number" min={1} step={1} aria-label="Total" value={r.total} onChange={(e) => set(i, { total: e.target.value })} />
          <Select aria-label="Unit" value={r.unit} onChange={(e) => set(i, { unit: e.target.value as KnownLimit["unit"] })}>
            {UNITS.map((u) => (
              <option key={u}>{u}</option>
            ))}
          </Select>
          <span>per</span>
          <Select aria-label="Window" value={r.window} onChange={(e) => set(i, { window: e.target.value as KnownLimit["window"] })}>
            {WINDOWS.map((w) => (
              <option key={w}>{w}</option>
            ))}
          </Select>
          <button type="button" aria-label="Remove limit" onClick={() => setRows((rs) => rs.filter((_, j) => j !== i))}>
            Remove
          </button>
        </div>
      ))}
      <div className="row">
        <button type="button" onClick={() => setRows((rs) => [...rs, { window: "day", unit: "requests", total: "" }])}>
          Add limit
        </button>
        <button
          type="button"
          className="primary"
          disabled={busy || !dirty || bad}
          onClick={() => {
            void save("usage", { knownLimits: { [providerId]: parsed.map((r) => ({ window: r.window, unit: r.unit, total: r.n })) } }).then(() => readUsage());
          }}
        >
          Save limits
        </button>
        {bad && dirty && <span className="ui-inline-bad">A whole number from 1, one limit for each window and unit.</span>}
      </div>
    </div>
  );
}

export default function UsageBlock({ p }: { p: ProviderView }) {
  const usage = usageOf(useUsage(), p.id);
  useEffect(() => {
    void readUsage();
  }, [p.id]);
  if (!usage) return null;
  return (
    <section className="ub" aria-label="Usage">
      <h4 className="ub-title">Usage</h4>
      {usage.local ? (
        <p className="hint">A local provider has no limits: only what went through KAgent is counted.</p>
      ) : usage.limits.length === 0 ? (
        <p className="hint">No limit known yet: it shows once the provider reports one on an answer, or when you type one below.</p>
      ) : (
        <div className="ub-limits">
          {usage.limits.map((l) => (
            <LimitBar key={l.id} limit={l} />
          ))}
        </div>
      )}
      <div className="ub-counts">
        <span className="ub-count" data-count="today">
          Today through KAgent: {usage.today.requests} requests, {tokens(usage.today)}
        </span>
        <span className="ub-count" data-count="month">
          This month: {usage.month.requests} requests, {tokens(usage.month)}
        </span>
        <span className="hint">Live voice is not counted.</span>
      </div>
      {usage.models.length > 0 && (
        <table className="ub-models">
          <thead>
            <tr>
              <th>Model</th>
              <th>Today</th>
              <th>This month</th>
            </tr>
          </thead>
          <tbody>
            {usage.models.map((m) => (
              <tr key={m.model} data-model={m.model}>
                <td>{m.model}</td>
                <td>
                  {m.today.requests} requests, {tokens(m.today)}
                </td>
                <td>
                  {m.month.requests} requests, {tokens(m.month)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <span className="hint ub-checked">{usage.checkedAt ? `Last checked ${resetText(usage.checkedAt)}` : "No answer with a limit has been seen yet."}</span>
      {!usage.local && <KnownEditor key={JSON.stringify(usage.knownLimits)} providerId={p.id} saved={usage.knownLimits} />}
    </section>
  );
}
