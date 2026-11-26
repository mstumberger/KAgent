// One core file of the memory (memory.md section 7): "About me" (`user.md`) or "This
// machine" (`machine.md`) as an editor with Save. The text is a draft, so what the owner
// types stays while the list is read again. Off or on, the file can be changed.
import { useState, type FormEvent } from "react";
import type { MemoryView } from "./client";
import { Field, GroupCard } from "../ui";
import { useDraft } from "../useDraft";

interface Props {
  title: string;
  hint: string;
  /** The file as the daemon shows it; `undefined` until the list is read. */
  view: MemoryView | undefined;
  /** Resolves with what was saved; rejects with the reason when the daemon refused. */
  onSave: (content: string) => Promise<MemoryView>;
}

export default function CoreEditor({ title, hint, view, onSave }: Props) {
  const [text, setText, dirty] = useDraft(view?.content ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!dirty) return;
    setBusy(true);
    setError("");
    try {
      await onSave(text);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <GroupCard title={title} description={hint}>
      <form className="ui-form" onSubmit={(e) => void submit(e)}>
        <Field label={`${title} text`} hint="Always in the prompt of every turn, at most 2000 characters together. Comments (&lt;!-- ... --&gt;) are left out.">
          <textarea className="mem-core-input" value={text} readOnly={!view} aria-invalid={!!error} onChange={(e) => setText(e.target.value)} />
        </Field>
        {error && (
          <p className="mem-error" role="alert">
            {error}
          </p>
        )}
        <div className="row">
          <button className="primary" type="submit" disabled={!dirty || busy || !view}>
            Save
          </button>
        </div>
      </form>
    </GroupCard>
  );
}
