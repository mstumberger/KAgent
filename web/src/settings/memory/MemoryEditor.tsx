// The form of a new or a changed topic memory (memory.md section 7): name (new only),
// description, type, content. A field says what is wrong with it before anything is
// sent; what the daemon refuses on top of that is shown above the form.
import { useState, type ChangeEvent, type FormEvent } from "react";
import type { MemoryDraft, MemoryType } from "./client";
import { Field } from "../ui";
import { TYPES, draftErrors } from "./shared";
import Select from "../Select";

interface Props {
  initial?: Partial<MemoryDraft>;
  /** The name is fixed and the existing memories do not count as taken. */
  editing?: boolean;
  existing: string[];
  busy: boolean;
  serverError?: string;
  onSave: (draft: MemoryDraft) => Promise<void>;
  onCancel: () => void;
}

const empty = (): MemoryDraft => ({ name: "", description: "", type: "user", content: "" });

export default function MemoryEditor({ initial, editing = false, existing, busy, serverError, onSave, onCancel }: Props) {
  const [draft, setDraft] = useState<MemoryDraft>({ ...empty(), ...initial });
  // Fields show their error once a save was tried, then as the user types.
  const [tried, setTried] = useState(false);
  const errors = tried ? draftErrors(draft, existing, editing) : {};
  const set = (field: "name" | "description" | "content") => (e: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => setDraft((d) => ({ ...d, [field]: e.target.value }));
  const setType = (e: { target: { value: string } }) => setDraft((d) => ({ ...d, type: e.target.value as MemoryType }));

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setTried(true);
    if (Object.keys(draftErrors(draft, existing, editing)).length > 0) return;
    await onSave({ name: draft.name.trim(), description: draft.description, type: draft.type, content: draft.content });
  };

  return (
    <form className="ui-form mem-editor" onSubmit={(e) => void submit(e)} noValidate>
      {serverError && (
        <p className="mem-error" role="alert">
          {serverError}
        </p>
      )}
      {!editing && (
        <Field label="Name" hint="Lowercase words joined by hyphens: the name of the file, and what KAgent looks at first when it recalls.">
          <input value={draft.name} aria-invalid={!!errors.name} onChange={set("name")} />
          {errors.name && (
            <span className="ui-inline-bad" role="alert">
              {errors.name}
            </span>
          )}
        </Field>
      )}
      <Field label="Description" hint="One line saying what it is. Empty: the first line of the content.">
        <input value={draft.description} aria-invalid={!!errors.description} onChange={set("description")} />
        {errors.description && (
          <span className="ui-inline-bad" role="alert">
            {errors.description}
          </span>
        )}
      </Field>
      <Field label="Type" hint="Feedback is a correction: do this, not that, and why. It wins when two memories match a message equally well.">
        <Select value={draft.type} onChange={setType}>
          {TYPES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </Select>
      </Field>
      <Field label="Content" hint="The fact itself, at most 4000 characters. No secrets: keys, passwords and tokens are refused.">
        <textarea className="mem-content-input" value={draft.content} aria-invalid={!!errors.content} onChange={set("content")} />
        {errors.content && (
          <span className="ui-inline-bad" role="alert">
            {errors.content}
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
