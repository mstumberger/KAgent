// The form of a new or a changed subagent profile (agents.md section 1): name (new only),
// description, model, tool groups as chips, max iterations, prompt. A field says what is
// wrong with it before anything is sent; what the daemon refuses on top of that is shown
// above the form.
import { useState, type ChangeEvent, type FormEvent } from "react";
import type { ProfileBody } from "./api";
import ChooserField from "../../components/chooser/ChooserField";
import type { PickerModel } from "../../picker";
import { MAX_ITERATIONS, MAX_PROMPT, bodyOf, draftErrors, emptyDraft, type Draft } from "./draft";
import { groupAbout, groupLabel } from "../groups";

interface Props {
  initial: Draft;
  /** The name is fixed, and the profiles that exist do not count as taken. */
  editing?: boolean;
  /** The names that exist: the daemon answers 409 for a built-in one. */
  taken: string[];
  models: PickerModel[];
  /** The tool groups a profile may name. */
  groups: string[];
  busy: boolean;
  serverError?: string;
  /** Asks first (the page shows the confirmation). */
  onDelete?: () => void;
  onSave: (name: string, body: ProfileBody) => Promise<void>;
  onCancel: () => void;
}

export default function ProfileEditor({ initial, editing = false, taken, models, groups, busy, serverError, onDelete, onSave, onCancel }: Props) {
  const [draft, setDraft] = useState<Draft>({ ...emptyDraft(), ...initial });
  // Fields show their error once a save was tried, then as the user types.
  const [tried, setTried] = useState(false);
  const errors = tried ? draftErrors(draft, taken, editing) : {};
  const set = (field: keyof Draft) => (e: ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) => setDraft((d) => ({ ...d, [field]: e.target.value }));
  const toggle = (group: string) => setDraft((d) => ({ ...d, groups: d.groups.includes(group) ? d.groups.filter((g) => g !== group) : [...d.groups, group] }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setTried(true);
    if (Object.keys(draftErrors(draft, taken, editing)).length > 0) return;
    await onSave(draft.name.trim(), bodyOf(draft));
  };

  return (
    <form className="ui-form sb-editor" onSubmit={(e) => void submit(e)} noValidate>
      {serverError && (
        <p className="sb-error" role="alert">
          {serverError}
        </p>
      )}
      <label className="ui-field">
        <span className="ui-field-label">Name</span>
        <input value={draft.name} readOnly={editing} aria-label="Profile name" aria-invalid={!!errors.name} onChange={set("name")} />
        {errors.name && (
          <span className="ui-inline-bad" role="alert">
            {errors.name}
          </span>
        )}
      </label>
      <label className="ui-field">
        <span className="ui-field-label">Description</span>
        <input value={draft.description} aria-label="Profile description" aria-invalid={!!errors.description} onChange={set("description")} />
        <span className="hint">What the profile is for. The agent reads it when it picks one; it is the title of the subagent's session.</span>
        {errors.description && (
          <span className="ui-inline-bad" role="alert">
            {errors.description}
          </span>
        )}
      </label>
      <div className="ui-field">
        <span className="ui-field-label">Model</span>
        {/* A model set in the file that the providers no longer list is the current row, so it is seen. */}
        <ChooserField ariaLabel="Profile model" models={models} value={draft.model} none={{ label: "The chat model" }} keptHint={() => "the providers no longer list it"} onChange={(model) => setDraft((d) => ({ ...d, model }))} />
        <span className="hint">A small local model fits research; the chat model is the default.</span>
      </div>
      <div className="ui-field">
        <span className="ui-field-label">Tool groups</span>
        <div className="chips" role="group" aria-label="Tool groups">
          {groups.map((g) => (
            <label key={g} className={`chip${draft.groups.includes(g) ? " on" : ""}`} title={groupAbout(g)}>
              <input type="checkbox" checked={draft.groups.includes(g)} onChange={() => toggle(g)} /> {groupLabel(g)}
            </label>
          ))}
        </div>
        <span className="hint">Only tools of the picked groups are offered, and a call outside them is refused. None picked: every enabled group.</span>
      </div>
      <label className="ui-field">
        <span className="ui-field-label">Max iterations</span>
        <input type="number" min={1} max={MAX_ITERATIONS} step={1} value={draft.iterations} aria-label="Max iterations" aria-invalid={!!errors.iterations} onChange={set("iterations")} />
        {errors.iterations && (
          <span className="ui-inline-bad" role="alert">
            {errors.iterations}
          </span>
        )}
      </label>
      <label className="ui-field">
        <span className="ui-field-label">Prompt</span>
        <textarea className="sb-prompt" rows={5} value={draft.prompt} aria-label="Profile prompt" aria-invalid={!!errors.prompt} onChange={set("prompt")} />
        <span className="hint">Extra system prompt text for the subagent (at most {MAX_PROMPT.toLocaleString("en")} characters). Leave it empty for none.</span>
        {errors.prompt && (
          <span className="ui-inline-bad" role="alert">
            {errors.prompt}
          </span>
        )}
      </label>
      <div className="row">
        <button className="primary" type="submit" disabled={busy}>
          Save
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
        {onDelete && (
          <button type="button" className="danger sb-delete" disabled={busy} onClick={onDelete}>
            Delete profile
          </button>
        )}
      </div>
    </form>
  );
}
