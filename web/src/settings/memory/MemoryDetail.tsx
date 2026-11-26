// One topic memory (memory.md section 7): what it says and where it lives, and what the
// owner can do with it: change it (it becomes the owner's), forget it (it stays, marked,
// and is no longer recalled), delete its file. Edit shows the form instead.
import { useState } from "react";
import type { MemoryDraft, MemoryView } from "./client";
import { memoryApi } from "./client";
import MemoryEditor from "./MemoryEditor";
import { TYPE_LABEL, state } from "./shared";
import { ago } from "../skills/shared";
import { Badge, Confirm } from "../ui";

interface Props {
  /** As the list read it, with its content. */
  memory: MemoryView;
  /** The list is read again after a write. */
  onChanged: () => void;
  /** A write answered with this memory: its row takes the answer at once. */
  onPatch: (memory: MemoryView) => void;
  report: (text: string, failed?: boolean) => void;
}

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function MemoryDetail({ memory, onChanged, onPatch, report }: Props) {
  const [editing, setEditing] = useState(false);
  const [confirm, setConfirm] = useState<"forget" | "delete" | null>(null);
  const [busy, setBusy] = useState(false);
  const [serverError, setServerError] = useState<string | undefined>();
  const gone = state(memory);
  const updated = ago(memory.updatedAt);

  /** Run something on the memory, say how it went in the header, read the list again. */
  const run = async (label: string, fn: () => Promise<string>) => {
    setBusy(true);
    try {
      report(await fn());
    } catch (e) {
      report(`${label} failed: ${why(e)}`, true);
    } finally {
      setBusy(false);
      setConfirm(null);
      onChanged();
    }
  };

  const save = async (draft: MemoryDraft) => {
    setServerError(undefined);
    try {
      const saved = await memoryApi.update(memory.name, { description: draft.description, type: draft.type, content: draft.content });
      onPatch(saved);
      setEditing(false);
      report(`Saved ${saved.name}`);
    } catch (e) {
      setServerError(why(e));
    }
    onChanged();
  };

  const forget = () =>
    run("Forget", async () => {
      const forgotten = await memoryApi.forget(memory.name);
      onPatch(forgotten);
      return `Forgot ${forgotten.name}: no longer recalled, its file stays`;
    });

  const remove = () =>
    run("Delete", async () => {
      await memoryApi.remove(memory.name);
      return `Deleted ${memory.name}`;
    });

  if (editing) {
    return (
      <MemoryEditor
        editing
        busy={busy}
        existing={[]}
        serverError={serverError}
        initial={{ name: memory.name, description: memory.description, type: memory.type, content: memory.content ?? "" }}
        onSave={save}
        onCancel={() => {
          setEditing(false);
          setServerError(undefined);
        }}
      />
    );
  }

  return (
    <div className="mem-detail">
      <div className="mem-detail-head">
        <div className="mem-title">
          <h3>{memory.name}</h3>
          <Badge tone="neutral">{TYPE_LABEL[memory.type]}</Badge>
          <Badge tone={memory.source === "agent" ? "accent" : "neutral"}>{memory.source === "agent" ? "agent" : "mine"}</Badge>
          {gone === "replaced" && (
            <Badge tone="neutral">replaced</Badge>
          )}
          {gone === "forgotten" && (
            <Badge tone="neutral">forgotten</Badge>
          )}
        </div>
      </div>

      <div className="mem-facts">
        <div className="mem-fact">
          <span className="hint">Description</span>
          <span>{memory.description || "none: the first line of the content is used"}</span>
        </div>
        <div className="mem-fact">
          <span className="hint">Path</span>
          <code>{memory.path}</code>
        </div>
        <div className="mem-fact">
          <span className="hint">Written</span>
          <span>
            {memory.source === "agent" ? "by the agent" : "by you"}
            {memory.source === "agent" && !memory.reviewed ? ", not reviewed" : ""}
            {`; updated ${memory.updatedAt}`}
            {updated ? ` (${updated})` : ""}
          </span>
        </div>
        {gone && (
          <div className="mem-fact">
            <span className="hint">Recall</span>
            <span>{gone === "replaced" ? `Out of recall: replaced by ${memory.supersededBy}.` : `Out of recall since ${memory.validUntil}.`}</span>
          </div>
        )}
      </div>

      {memory.content ? <div className="mem-text">{memory.content}</div> : <p className="hint">The file has no text.</p>}

      <div className="mem-actions">
        <button
          disabled={busy}
          onClick={() => {
            setServerError(undefined);
            setEditing(true);
          }}
        >
          Edit
        </button>
        <button disabled={busy || !!gone} title={gone ? "It is out of recall already" : "It stays in the list, marked as forgotten, and is no longer recalled"} onClick={() => setConfirm("forget")}>
          Forget
        </button>
        <button className="danger" disabled={busy} onClick={() => setConfirm("delete")}>
          Delete file
        </button>
      </div>

      {confirm === "forget" && (
        <Confirm
          title={`Forget ${memory.name}?`}
          body="KAgent stops recalling it, and it leaves MEMORY.md. The file stays here: you can read it again, or delete it."
          confirmLabel="Forget"
          onConfirm={() => void forget()}
          onCancel={() => setConfirm(null)}
        />
      )}
      {confirm === "delete" && (
        <Confirm
          title={`Delete ${memory.name}?`}
          body={`The file ${memory.path} is removed. This cannot be undone.`}
          confirmLabel="Delete"
          danger
          onConfirm={() => void remove()}
          onCancel={() => setConfirm(null)}
        />
      )}
    </div>
  );
}
