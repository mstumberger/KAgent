// Watchers: when they hold back notifications that can wait.
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Field, GroupCard, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";

/** `23:00-07:00`: from and to, over midnight allowed. */
const QUIET = /^([01]?\d|2[0-3]):[0-5]\d-([01]?\d|2[0-3]):[0-5]\d$/;
const DEFAULT = "23:00-07:00";

export default function Watchers({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const [quiet, setQuiet, dirty] = useDraft(settings?.watchers?.quietHours ?? DEFAULT);
  const valid = QUIET.test(quiet.trim());

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Quiet hours" description="No non-critical notifications in this window of the day.">
        <Field label="Quiet hours" hint="From and to, HH:MM-HH:MM. A window over midnight is allowed.">
          <input value={quiet} placeholder={DEFAULT} onChange={(e) => setQuiet(e.target.value)} />
        </Field>
        <div className="row">
          <button className="primary" disabled={!dirty || !valid} onClick={() => void save("watchers", { quietHours: quiet.trim() })}>
            Save quiet hours
          </button>
          {!valid && dirty && <span className="ui-inline-bad">Write the hours as HH:MM-HH:MM, for example {DEFAULT}.</span>}
        </div>
      </GroupCard>
    </SectionPage>
  );
}
