// Automations (automations.md section 1, config.md section 11 `settings.tasks`): the policy
// a task gets when its record leaves a field out, and how many agent runs work at once.
// The tasks themselves live in the Tasks tab.
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Field, GroupCard, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import "../../views/tasks/tasks.css";
import Select from "../Select";

interface TasksSettings {
  onApproval?: "deny" | "wait";
  deliver?: string[];
  timeoutMin?: number;
  maxConcurrent?: number;
}

/** A whole number from `min` to `max`. */
const whole = (text: string, min: number, max: number) => /^\d+$/.test(text.trim()) && Number(text) >= min && Number(text) <= max;

const CHANNELS = [
  { id: "desktop", label: "Desktop notification" },
  { id: "session", label: "The run's session" },
  { id: "telegram", label: "Telegram (needs the Telegram bot)" },
];

export default function Tasks({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const section = (settings?.tasks ?? {}) as TasksSettings;
  const { save, busy } = useSettingPatch();
  const [onApproval, setOnApproval, approvalDirty] = useDraft(section.onApproval ?? "deny");
  const [deliver, setDeliver, deliverDirty] = useDraft(section.deliver ?? ["desktop", "session"]);
  const [timeout, setTimeout_, timeoutDirty] = useDraft(String(section.timeoutMin ?? 30));
  const [max, setMax, maxDirty] = useDraft(String(section.maxConcurrent ?? 2));
  const timeoutOk = whole(timeout, 1, 1440);
  const maxOk = whole(max, 1, 8);
  const dirty = approvalDirty || deliverDirty || timeoutDirty || maxDirty;
  const ok = timeoutOk && maxOk && deliver.length > 0;
  const toggle = (id: string) => setDeliver(deliver.includes(id) ? deliver.filter((c) => c !== id) : [...deliver, id]);

  const submit = () => {
    if (dirty && ok) void save("tasks", { onApproval, deliver, timeoutMin: Number(timeout), maxConcurrent: Number(max) });
  };

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Defaults of a new task" description="A task that does not choose these itself gets them. The form in the Tasks tab can set each one per task.">
        <Field label="A step that needs approval" hint="Skip it: the run goes on without it. Wait for me: the run waits for your answer until the time limit.">
          <Select aria-label="A step that needs approval" value={onApproval} disabled={!settings} onChange={(e) => setOnApproval(e.target.value as "deny" | "wait")}>
            <option value="deny">Skip it</option>
            <option value="wait">Wait for me</option>
          </Select>
        </Field>
        <fieldset className="task-checks">
          <legend className="hint">Deliver the result to</legend>
          {CHANNELS.map((c) => (
            <label key={c.id}>
              <input type="checkbox" checked={deliver.includes(c.id)} disabled={!settings} onChange={() => toggle(c.id)} />
              {c.label}
            </label>
          ))}
        </fieldset>
        <Field label="Time limit of a run (minutes)" hint="1 to 1440. The default is 30. A run that takes longer is cancelled with the status timeout.">
          <input type="number" min={1} max={1440} step={1} aria-label="Time limit of a run" value={timeout} disabled={!settings} onChange={(e) => setTimeout_(e.target.value)} />
        </Field>
      </GroupCard>
      <GroupCard title="Running at once" description="How many agent runs of tasks work at the same time. More wait for a free slot.">
        <Field label="At most" hint="1 to 8. The default is 2.">
          <input type="number" min={1} max={8} step={1} aria-label="Task runs at once" value={max} disabled={!settings} onChange={(e) => setMax(e.target.value)} />
        </Field>
      </GroupCard>
      <div className="row">
        <button className="primary" disabled={!dirty || !ok || busy || !settings} onClick={submit}>
          Save
        </button>
        {!timeoutOk && <span className="ui-inline-bad">The time limit is a whole number from 1 to 1440.</span>}
        {!maxOk && <span className="ui-inline-bad">At most is a whole number from 1 to 8.</span>}
        {deliver.length === 0 && <span className="ui-inline-bad">Choose at least one place for the result.</span>}
      </div>
    </SectionPage>
  );
}
