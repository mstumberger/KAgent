// The form of the Tasks tab (automations.md section 6): what to do, and when: presets as
// chips, then the words or the cron expression in a field, with a preview of what the
// daemon makes of it. An agent task also says where its result goes, and what a step
// that needs approval does.
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useStore } from "../../store";
import { errorMessage, moment, tasksApi, type Task, type TaskInput, type TaskKind } from "./api";

/** The presets: a chip fills the field with its words. Custom is on for any other text. */
const PRESETS = [
  { id: "in", label: "In ...", text: "in 20m" },
  { id: "every", label: "Every ...", text: "every 30m" },
  { id: "daily", label: "Daily", text: "daily 09:00" },
  { id: "weekdays", label: "Weekdays", text: "weekdays 08:30" },
  { id: "weekly", label: "Weekly", text: "every monday 09:00" },
  { id: "custom", label: "Custom", text: null },
] as const;

interface Draft {
  kind: TaskKind;
  what: string;
  title: string;
  schedule: string;
  /** Left alone in this form (null): settings.tasks gives the value, in the daemon too. */
  deliver: string[] | null;
  onApproval: "deny" | "wait" | null;
  session: "new" | "reuse";
}

/** settings.tasks: the value a policy field has until the owner chooses one here. */
function defaultsOf(settings: unknown): { deliver: string[]; onApproval: "deny" | "wait" } {
  const section = (settings as { tasks?: Record<string, unknown> } | null | undefined)?.tasks;
  const deliver = Array.isArray(section?.deliver) ? section.deliver.filter((c): c is string => typeof c === "string") : [];
  return { deliver: deliver.length > 0 ? deliver : ["desktop", "session"], onApproval: section?.onApproval === "wait" ? "wait" : "deny" };
}

export default function TaskForm({ onAdded, onError }: { onAdded: (tasks: Task[]) => void; onError: (message: string) => void }) {
  // Read from the store on each render: the settings may arrive after the form mounts.
  const defaults = defaultsOf(useStore().state?.settings);
  const [form, setForm] = useState<Draft>({ kind: "agent", what: "", title: "", schedule: "", deliver: null, onApproval: null, session: "new" });
  const [busy, setBusy] = useState(false);
  // What the daemon makes of a schedule text: `for` is the text it is about.
  const [preview, setPreview] = useState<{ for: string; ok: boolean; text: string; next: string | null }>({ for: "", ok: true, text: "", next: null });
  const field = useRef<HTMLInputElement>(null);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setForm((f) => ({ ...f, [key]: value }));
  const deliver = form.deliver ?? defaults.deliver;
  const onApproval = form.onApproval ?? defaults.onApproval;

  // Asked after a pause in typing; the answer to older text is dropped.
  const schedule = form.schedule.trim();
  useEffect(() => {
    if (!schedule) return;
    let live = true;
    const timer = setTimeout(() => {
      tasksApi.preview(schedule).then(
        (p) => {
          if (live) setPreview({ for: schedule, ok: true, text: p.scheduleText, next: p.nextRun });
        },
        (e) => {
          if (live) setPreview({ for: schedule, ok: false, text: errorMessage(e), next: null });
        },
      );
    }, 250);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [schedule]);

  const add = async (e: FormEvent) => {
    e.preventDefault();
    if (busy || !form.what.trim()) return;
    setBusy(true);
    try {
      const input: TaskInput = { what: form.what, kind: form.kind };
      if (form.title.trim()) input.title = form.title.trim();
      if (schedule) input.schedule = schedule;
      // What was not chosen here is left out: the daemon takes it from settings.tasks.
      // The session of a run is always kept.
      if (form.deliver !== null) input.deliver = form.kind === "agent" ? [...form.deliver.filter((c) => c !== "session"), "session"] : form.deliver.filter((c) => c !== "session");
      if (form.kind === "agent") {
        if (form.onApproval !== null) input.onApproval = form.onApproval;
        input.session = form.session;
      }
      onAdded((await tasksApi.add(input)).tasks);
      setForm((f) => ({ ...f, what: "", title: "", schedule: "" }));
    } catch (err) {
      onError(errorMessage(err));
    }
    setBusy(false);
  };

  const toggle = (channel: string) => set("deliver", deliver.includes(channel) ? deliver.filter((c) => c !== channel) : [...deliver, channel]);
  const agent = form.kind === "agent";

  // "Runs: <scheduleText>" once the daemon answered for the text that is in the field.
  let runs: ReactNode = null;
  if (!schedule) runs = "No schedule: a to-do, run on demand with Run now.";
  else if (preview.for === schedule) {
    runs = preview.ok ? (
      <>
        Runs: {preview.text}
        {preview.next && <span className="task-next-run">next {moment(preview.next)}</span>}
      </>
    ) : (
      <span className="bad-text">{preview.text}</span>
    );
  }

  return (
    <form className="card task-form" onSubmit={(e) => void add(e)}>
      <div className="task-form-row">
        <select aria-label="Kind" value={form.kind} onChange={(e) => set("kind", e.target.value as TaskKind)}>
          <option value="agent">KAgent does</option>
          <option value="remind">Remind me</option>
        </select>
        <input
          className="grow"
          aria-label="What"
          placeholder={agent ? "Check for security updates and tell me what needs a reboot" : "Call the electrician about the doorbell"}
          value={form.what}
          onChange={(e) => set("what", e.target.value)}
        />
      </div>
      <div className="task-form-row">
        <input aria-label="Title (optional)" placeholder="Title (optional)" value={form.title} onChange={(e) => set("title", e.target.value)} />
        <input
          ref={field}
          aria-label="When"
          placeholder="When: in 20m, weekdays 08:30, 0 9 * * 1-5"
          value={form.schedule}
          onChange={(e) => set("schedule", e.target.value)}
        />
        <button className="primary" disabled={busy || !form.what.trim()}>
          Add task
        </button>
      </div>
      <div className="task-presets" role="group" aria-label="Schedule presets">
        {PRESETS.map((preset) => {
          const on = preset.text === null ? schedule !== "" && !PRESETS.some((p) => p.text === schedule) : schedule === preset.text;
          return (
            <button
              type="button"
              key={preset.id}
              className={`chip${on ? " on" : ""}`}
              aria-pressed={on}
              onClick={() => {
                set("schedule", preset.text ?? "");
                if (preset.text === null) field.current?.focus();
              }}
            >
              {preset.label}
            </button>
          );
        })}
      </div>
      <div className="task-preview" aria-live="polite">
        {runs}
      </div>
      <fieldset className="task-checks">
        <legend className="hint">Deliver to</legend>
        <label>
          <input type="checkbox" checked={deliver.includes("desktop")} onChange={() => toggle("desktop")} />
          Desktop notification
        </label>
        {agent && (
          <label title="The session of each run is always kept: it is reachable from here and listed in Chat">
            <input type="checkbox" checked disabled />
            Session (always kept)
          </label>
        )}
        <label>
          <input type="checkbox" checked={deliver.includes("telegram")} onChange={() => toggle("telegram")} />
          Telegram (needs the Telegram bot)
        </label>
      </fieldset>
      {agent && (
        <div className="task-policy">
          <label>
            Needs approval
            <select aria-label="Needs approval" value={onApproval} onChange={(e) => set("onApproval", e.target.value as "deny" | "wait")}>
              <option value="deny">Skip it</option>
              <option value="wait">Wait for me</option>
            </select>
          </label>
          <label>
            Session
            <select aria-label="Session" value={form.session} onChange={(e) => set("session", e.target.value as Draft["session"])}>
              <option value="new">A new one for each run</option>
              <option value="reuse">One session for the task</option>
            </select>
          </label>
        </div>
      )}
    </form>
  );
}

