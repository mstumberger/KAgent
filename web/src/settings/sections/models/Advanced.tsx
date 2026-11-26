// Advanced: the options sent with every request of a model (reasoning level, output
// tokens, temperature) and how far one turn may go (iterations, approval timeout).
import type { FormEvent } from "react";
import { useStore } from "../../../store";
import { Field, GroupCard, Row } from "../../ui";
import { useDraft } from "../../useDraft";
import { useSettingPatch } from "../../useSettingPatch";

type ModelRef = { ref: string; config?: { reasoningLevels?: string[] } };
type Numberish = number | null;

const LIMITS = { maxIterations: 40, approvalTimeoutSec: 600 };
const positive = (n: Numberish) => n === null || (Number.isInteger(n) && n > 0);
const number = (v: string): Numberish => (v.trim() === "" ? null : Number(v));

export default function Advanced({ models }: { models: ModelRef[] }) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const level = settings?.options?.reasoningLevel ?? "";
  // What the chat models offer, and the level set now.
  const levels = [...new Set([...(level ? [level] : []), ...models.flatMap((m) => m.config?.reasoningLevels ?? [])])];
  const [options, setOptions, optionsDirty] = useDraft({ maxOutputTokens: settings?.options?.maxOutputTokens ?? null, temperature: settings?.options?.temperature ?? null });
  const [limits, setLimits, limitsDirty] = useDraft<{ maxIterations: Numberish; approvalTimeoutSec: Numberish }>({
    maxIterations: settings?.maxIterations ?? LIMITS.maxIterations,
    approvalTimeoutSec: settings?.approvalTimeoutSec ?? LIMITS.approvalTimeoutSec,
  });

  const temperatureOk = options.temperature === null || (options.temperature >= 0 && options.temperature <= 2);
  const optionsOk = temperatureOk && positive(options.maxOutputTokens);
  const limitsOk = positive(limits.maxIterations) && positive(limits.approvalTimeoutSec);

  const saveOptions = (e: FormEvent) => {
    e.preventDefault();
    if (optionsDirty && optionsOk) void save("options", { maxOutputTokens: options.maxOutputTokens, temperature: options.temperature });
  };
  // One PUT per changed key.
  const saveLimits = (e: FormEvent) => {
    e.preventDefault();
    const { maxIterations, approvalTimeoutSec } = limits;
    if (maxIterations === null || approvalTimeoutSec === null) return;
    if (maxIterations !== (settings?.maxIterations ?? LIMITS.maxIterations)) void save("maxIterations", maxIterations);
    if (approvalTimeoutSec !== (settings?.approvalTimeoutSec ?? LIMITS.approvalTimeoutSec)) void save("approvalTimeoutSec", approvalTimeoutSec);
  };

  return (
    <>
      <GroupCard title="Advanced: model options" description="Sent with every request of the chat model. Empty: what the provider chooses.">
        <Row label="Reasoning level" hint="How long a model thinks before it answers. Only a model that offers levels uses it.">
          {levels.length > 0 ? (
            <select aria-label="Reasoning level" value={level} onChange={(e) => void save("options", { reasoningLevel: e.target.value || null })}>
              <option value="">(provider default)</option>
              {levels.map((l) => (
                <option key={l} value={l}>
                  {l}
                </option>
              ))}
            </select>
          ) : (
            <span className="hint">No chat model offers reasoning levels.</span>
          )}
        </Row>
        <form className="ui-form" onSubmit={saveOptions}>
          <div className="ui-form-grid">
            <Field label="Max output tokens" hint="The longest answer, in tokens.">
              <input type="number" min={1} value={options.maxOutputTokens ?? ""} onChange={(e) => setOptions({ ...options, maxOutputTokens: number(e.target.value) })} />
            </Field>
            <Field label="Temperature" hint="0 to 2. Lower is more exact, higher more varied.">
              <input type="number" min={0} max={2} step={0.1} value={options.temperature ?? ""} onChange={(e) => setOptions({ ...options, temperature: number(e.target.value) })} />
            </Field>
          </div>
          <div className="row">
            <button className="primary" type="submit" disabled={!optionsDirty || !optionsOk}>
              Save options
            </button>
            {!optionsOk && optionsDirty && <span className="ui-inline-bad">Tokens: a whole number above 0. Temperature: 0 to 2.</span>}
          </div>
        </form>
      </GroupCard>

      <GroupCard title="Advanced: limits of a turn" description="How far one message may go.">
        <form className="ui-form" onSubmit={saveLimits}>
          <div className="ui-form-grid">
            <Field label="Max iterations" hint="Tool calls and replies one turn may take before it stops.">
              <input type="number" min={1} value={limits.maxIterations ?? ""} onChange={(e) => setLimits({ ...limits, maxIterations: number(e.target.value) })} />
            </Field>
            <Field label="Approval timeout (seconds)" hint="How long an approval card waits before the call counts as refused.">
              <input type="number" min={1} value={limits.approvalTimeoutSec ?? ""} onChange={(e) => setLimits({ ...limits, approvalTimeoutSec: number(e.target.value) })} />
            </Field>
          </div>
          <div className="row">
            <button className="primary" type="submit" disabled={!limitsDirty || !limitsOk}>
              Save limits
            </button>
            {!limitsOk && limitsDirty && <span className="ui-inline-bad">Both must be whole numbers above 0.</span>}
          </div>
        </form>
      </GroupCard>
    </>
  );
}
