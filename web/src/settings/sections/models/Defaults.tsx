// Defaults (models.md section 7.6): which chat model answers, steps in, does the
// short jobs and reads the screen; then the temperature. The effort is chosen per
// conversation in the composer (picker.md). The
// live model is chosen in the Voice section.
import type { FormEvent } from "react";
import type { ModelView, ProviderView } from "../../../api";
import { useStore } from "../../../store";
import Dropdown from "../../Dropdown";
import { Field, GroupCard, Row } from "../../ui";
import { useDraft } from "../../useDraft";
import { useSettingPatch } from "../../useSettingPatch";
import { modelOptions, unusableWhy } from "./present";

type Pick = { label: string; key: "chatModel" | "fallbackModel" | "fastModel" | "visionModel"; none: string; hint: string };

const PICKS: Pick[] = [
  { label: "Default chat model", key: "chatModel", none: "(none)", hint: "Answers every conversation." },
  { label: "Fallback", key: "fallbackModel", none: "(none)", hint: "Used when the chat model is busy or out of quota." },
  { label: "Fast", key: "fastModel", none: "(none)", hint: "A smaller model for the short jobs: it names new sessions and summarizes long conversations." },
  { label: "Vision", key: "visionModel", none: "(auto: the first Gemini flash model)", hint: 'Reads the screen for "what does this say?"' },
];

export default function Defaults({ chat, providers }: { chat: ModelView[]; providers: ProviderView[] }) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const [temperature, setTemperature, temperatureDirty] = useDraft({ value: settings?.options?.temperature ?? null });

  const temperatureOk = temperature.value === null || (temperature.value >= 0 && temperature.value <= 2);
  const saveTemperature = (e: FormEvent) => {
    e.preventDefault();
    if (temperatureDirty && temperatureOk) void save("options", { temperature: temperature.value });
  };

  return (
    <GroupCard title="Defaults" description="Which model does which job. Only a model with the chat role can be chosen; one that cannot be used says why.">
      {PICKS.map(({ label, key, none, hint }) => {
        const current = settings?.[key] ?? "";
        const why = current ? unusableWhy(current, providers) : null;
        return (
          <Row key={key} label={label} hint={hint}>
            <div className="dd-cell">
              <Dropdown ariaLabel={label} value={current} options={modelOptions(chat, providers, current, none)} onChange={(value) => void save(key, value)} />
              {why && <span className="ui-inline-bad">{why}</span>}
            </div>
          </Row>
        );
      })}
      <form className="ui-form" onSubmit={saveTemperature}>
        <Field label="Temperature" hint="0 to 2. Lower is more exact, higher more varied. Empty: what the provider chooses.">
          <input type="number" min={0} max={2} step={0.1} value={temperature.value ?? ""} onChange={(e) => setTemperature({ value: e.target.value.trim() === "" ? null : Number(e.target.value) })} />
        </Field>
        <div className="row">
          <button className="primary" type="submit" disabled={!temperatureDirty || !temperatureOk}>
            Save temperature
          </button>
          {!temperatureOk && temperatureDirty && <span className="ui-inline-bad">Temperature: 0 to 2.</span>}
        </div>
      </form>
    </GroupCard>
  );
}
