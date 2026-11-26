// One skill (skills.md section 9): what it is, where it lives, its text, and what the
// owner can do with it. The edit switch shows the form instead.
import { useEffect, useState } from "react";
import Markdown from "../../components/Markdown";
import { api, skillsApi, type SkillDraft, type SkillView } from "../../api";
import { Badge, Confirm, Toggle } from "../ui";
import SkillEditor from "./SkillEditor";
import { SOURCE_LABEL, ago, copyName } from "./shared";

interface Props {
  /** The skill's row of the list (no body): it is read here. */
  skill: SkillView;
  existing: string[];
  /** The list is read again after a write. */
  onChanged: () => void;
  /** A write answered with this skill: its row takes the answer at once. */
  onPatch: (skill: SkillView) => void;
  onSelect: (key: string) => void;
  report: (text: string, failed?: boolean) => void;
}

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function SkillDetail({ skill, existing, onChanged, onPatch, onSelect, report }: Props) {
  const [full, setFull] = useState<SkillView>(skill);
  const [editing, setEditing] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [serverError, setServerError] = useState<string | undefined>();

  // The body comes with GET /api/skills/:name.
  useEffect(() => {
    skillsApi
      .get(skill.name)
      .then((view) => {
        setFull(view);
        setLoadError("");
      })
      .catch((e: unknown) => setLoadError(why(e)));
  }, [skill.name]);

  // The row knows what changed elsewhere (its switch, a new use, a reload); only the
  // body comes from here.
  const view: SkillView = { ...full, ...skill, body: full.body ?? skill.body };

  /** Run something on the skill, say how it went in the header, read the list again. */
  const run = async (label: string, fn: () => Promise<string | void>) => {
    setBusy(true);
    try {
      report((await fn()) || `${label} done`);
    } catch (e) {
      report(`${label} failed: ${why(e)}`, true);
    } finally {
      setBusy(false);
      onChanged();
    }
  };

  const save = async (draft: SkillDraft) => {
    setServerError(undefined);
    try {
      const saved = await skillsApi.update(view.name, draft);
      setFull(saved);
      onPatch(saved);
      setEditing(false);
      report(`Saved ${view.name}`);
      onChanged();
    } catch (e) {
      setServerError(why(e));
    }
  };

  const duplicate = () =>
    run("Duplicate", async () => {
      const name = copyName(view.name, existing);
      const copy = await skillsApi.create({ name, description: view.description, whenToUse: view.whenToUse, body: view.body ?? "" });
      onSelect(`${copy.source}:${copy.name}`);
      return `Copied to ${name}`;
    });

  const openFolder = () =>
    run("Open folder", async () => {
      const r = await api.post<{ status: string; output: string }>("/api/tools/open/call", { input: { target: view.path }, sessionId: "ui" });
      if (r.status !== "ok") throw new Error(r.output);
    });

  const remove = () =>
    run("Delete", async () => {
      setConfirm(false);
      await skillsApi.remove(view.name);
      report(`Deleted ${view.name}`);
    });

  if (editing) {
    return (
      <SkillEditor
        editing
        busy={busy}
        existing={existing}
        serverError={serverError}
        initial={{ name: view.name, description: view.description, whenToUse: view.whenToUse ?? "", body: view.body ?? "" }}
        onSave={save}
        onCancel={() => {
          setEditing(false);
          setServerError(undefined);
        }}
      />
    );
  }

  const deletable = view.source !== "bundled";
  return (
    <div className="sk-detail">
      <div className="sk-head">
        <div className="sk-title">
          <h3>{view.name}</h3>
          <Badge tone={view.source === "user" ? "accent" : "neutral"}>{SOURCE_LABEL[view.source]}</Badge>
          <Badge tone={view.enabled && !view.shadowed ? "ok" : "warn"}>{view.shadowed ? "hidden" : view.enabled ? "on" : "off"}</Badge>
        </div>
        {view.source === "user" && <Toggle label="Edit" checked={false} onChange={setEditing} />}
      </div>

      <div className="sk-facts">
        <div className="sk-fact">
          <span className="hint">Path</span>
          <button className="sk-path" title="Copy the path" onClick={() => void navigator.clipboard?.writeText(view.realPath).then(() => report("Path copied"), () => report("Copy failed", true))}>
            <code>{view.path}</code>
          </button>
        </div>
        {view.symlink && (
          <p className="hint">
            A link to {view.realPath}.
          </p>
        )}
        {view.description ? <p className="sk-text">{view.description}</p> : <p className="hint">No description: the agent can only pick it by its name.</p>}
        {view.whenToUse && (
          <div className="sk-fact">
            <span className="hint">When to use</span>
            <span>{view.whenToUse}</span>
          </div>
        )}
        <div className="sk-fact">
          <span className="hint">Use</span>
          <span>{view.uses ? `used ${view.uses} ${view.uses === 1 ? "time" : "times"}, last ${ago(view.lastUsed)}` : "never loaded"}</span>
        </div>
        <div className="sk-fact">
          <span className="hint">Updated</span>
          <span>{ago(view.updatedAt) || "unknown"}</span>
        </div>
        {view.importedFrom && (
          <div className="sk-fact">
            <span className="hint">Imported from</span>
            <span>
              {view.importedFrom.source}
              {view.importedFrom.path && (
                <code className="sk-origin" title={`SHA-256 of its files when it arrived: ${view.importedFrom.hash}`}>
                  {view.importedFrom.path}
                </code>
              )}
            </span>
          </div>
        )}
      </div>

      {loadError ? (
        <p className="sk-error" role="alert">
          {loadError}
        </p>
      ) : view.body ? (
        <div className="sk-body">
          <Markdown text={view.body} />
        </div>
      ) : (
        <p className="hint">Reading the skill...</p>
      )}

      <div className="sk-actions">
        <button disabled={busy} onClick={() => void run("Switch", async () => {
          const switched = await skillsApi.setEnabled(view.name, !view.enabled);
          setFull((f) => ({ ...f, enabled: switched.enabled }));
          onPatch(switched);
          return `${view.name} is ${switched.enabled ? "on" : "off"}`;
        })}>
          {view.enabled ? "Disable" : "Enable"}
        </button>
        <button disabled={busy} onClick={() => void duplicate()}>
          Duplicate
        </button>
        <button disabled={busy} onClick={() => void openFolder()}>
          Open folder
        </button>
        <button className="danger" disabled={busy || !deletable} title={deletable ? undefined : "A skill KAgent ships with can be switched off, not deleted"} onClick={() => setConfirm(true)}>
          Delete
        </button>
      </div>

      {confirm && (
        <Confirm
          title={`Delete ${view.name}?`}
          body={
            view.symlink
              ? "Only the link is removed. The folder it points to, and everything in it, stays. This cannot be undone."
              : "The folder of the skill is removed, with everything in it. This cannot be undone."
          }
          confirmLabel="Delete"
          danger
          onConfirm={() => void remove()}
          onCancel={() => setConfirm(false)}
        />
      )}
    </div>
  );
}
