// The form of a new or a changed skill (skills.md section 9): name (new only),
// description, when to use, body. A field says what is wrong with it before anything
// is sent; what the daemon refuses on top of that is shown above the form.
import { useState, type ChangeEvent, type FormEvent } from "react";
import type { SkillDraft } from "../../skillsApi";
import { Field } from "../ui";
import { draftErrors, emptyDraft, type Draft } from "./shared";

interface Props {
  initial?: Partial<Draft>;
  /** The name is fixed and the existing skills do not count as taken. */
  editing?: boolean;
  existing: string[];
  busy: boolean;
  serverError?: string;
  onSave: (draft: SkillDraft) => Promise<void>;
  onCancel: () => void;
}

export default function SkillEditor({ initial, editing = false, existing, busy, serverError, onSave, onCancel }: Props) {
  const [draft, setDraft] = useState<Draft>({ ...emptyDraft(), ...initial });
  // Fields show their error once a save was tried, then as the user types.
  const [tried, setTried] = useState(false);
  const errors = tried ? draftErrors(draft, existing, editing) : {};
  const set = (field: keyof Draft) => (e: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setDraft((d) => ({ ...d, [field]: e.target.value }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setTried(true);
    if (Object.keys(draftErrors(draft, existing, editing)).length > 0) return;
    const { name, description, whenToUse, body } = draft;
    await onSave(editing ? { description, whenToUse, body } : { name: name.trim(), description, whenToUse, body });
  };

  return (
    <form className="ui-form sk-editor" onSubmit={(e) => void submit(e)} noValidate>
      {serverError && (
        <p className="sk-error" role="alert">
          {serverError}
        </p>
      )}
      <Field label="Name" hint={editing ? "The name is the folder; it does not change." : "Lowercase words joined by hyphens. In chat, /name calls the skill."}>
        <input value={draft.name} readOnly={editing} aria-invalid={!!errors.name} onChange={set("name")} />
        {errors.name && (
          <span className="ui-inline-bad" role="alert">
            {errors.name}
          </span>
        )}
      </Field>
      <Field label="Description" hint="The words the agent matches a request against. Write them as you would ask.">
        <input value={draft.description} aria-invalid={!!errors.description} onChange={set("description")} />
        {errors.description && (
          <span className="ui-inline-bad" role="alert">
            {errors.description}
          </span>
        )}
      </Field>
      <Field label="When to use" hint="Optional: the cases the description cannot hold.">
        <input value={draft.whenToUse} onChange={set("whenToUse")} />
      </Field>
      <Field label="Body" hint="Markdown. Numbered steps that name the tools, and what a good answer looks like.">
        <textarea className="sk-body-input" value={draft.body} aria-invalid={!!errors.body} onChange={set("body")} />
        {errors.body && (
          <span className="ui-inline-bad" role="alert">
            {errors.body}
          </span>
        )}
      </Field>
      <div className="row">
        <button className="primary" type="submit" disabled={busy}>
          Save
        </button>
        <button type="button" disabled={busy} onClick={onCancel}>
          Cancel
        </button>
      </div>
    </form>
  );
}
