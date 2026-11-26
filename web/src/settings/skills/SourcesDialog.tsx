// Git sources of skills (skills.md sections 8 and 9): the sources, their sync and their
// removal, and the form that adds one, with the catalog as suggestions.
import { useEffect, useState, type ChangeEvent, type FormEvent } from "react";
import { skillsApi, type CatalogEntry, type SourceForm, type SourceReport, type SourceView } from "../../api";
import Dialog from "../dialog";
import "./dialogs.css";
import { Field } from "../ui";
import { ago } from "./shared";

interface Props {
  onClose: () => void;
  /** Skills may have arrived or gone: the page reads its list again. */
  onChanged: () => void;
  report: (text: string, failed?: boolean) => void;
}

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));
const empty: SourceForm = { url: "", ref: "", subdir: "", name: "" };
const count = (n: number) => `${n} ${n === 1 ? "skill" : "skills"}`;
/** What a report says of what was not done. */
const notDone = (report: SourceReport) => {
  const skipped = report.skipped.map((s) => `${s.name} (${s.reason})`).join(", ");
  return skipped ? `; not installed: ${skipped}` : "";
};

export default function SourcesDialog({ onClose, onChanged, report }: Props) {
  const [sources, setSources] = useState<SourceView[] | null>(null);
  const [catalog, setCatalog] = useState<CatalogEntry[]>([]);
  const [loadError, setLoadError] = useState("");
  const [form, setForm] = useState<SourceForm>(empty);
  const [formError, setFormError] = useState("");
  const [adding, setAdding] = useState(false);
  /** The source being synced or removed. */
  const [busy, setBusy] = useState<string | null>(null);
  /** The source whose removal is being confirmed. */
  const [confirm, setConfirm] = useState<SourceView | null>(null);
  const [alsoRemove, setAlsoRemove] = useState(false);
  const [status, setStatus] = useState("");

  useEffect(() => {
    skillsApi
      .sources()
      .then((r) => {
        setSources(r.sources);
        setCatalog(r.catalog);
        setLoadError("");
      })
      .catch((e: unknown) => setLoadError(why(e)));
  }, []);

  const readAgain = () =>
    skillsApi
      .sources()
      .then((r) => {
        setSources(r.sources);
        setLoadError("");
      })
      .catch((e: unknown) => setLoadError(why(e)));
  const set = (field: keyof SourceForm) => (e: ChangeEvent<HTMLInputElement>) => setForm((f) => ({ ...f, [field]: e.target.value }));

  const add = async (e: FormEvent) => {
    e.preventDefault();
    if (!form.url.trim()) {
      setFormError("Give an https:// address, or the path of a folder.");
      return;
    }
    setAdding(true);
    setFormError("");
    setStatus("");
    const filled: SourceForm = {
      url: form.url.trim(),
      ...(form.ref?.trim() ? { ref: form.ref?.trim() } : {}),
      ...(form.subdir?.trim() ? { subdir: form.subdir?.trim() } : {}),
      ...(form.name?.trim() ? { name: form.name?.trim() } : {}),
    };
    try {
      const added = await skillsApi.addSource(filled);
      const text = `Added ${added.name}: ${count(added.added.length)}` + notDone(added);
      setStatus(text);
      report(text);
      setForm(empty);
    } catch (err) {
      setFormError(why(err));
    } finally {
      setAdding(false);
      readAgain();
      onChanged();
    }
  };

  const sync = async (source: SourceView) => {
    setBusy(source.id);
    setStatus("");
    try {
      const r = await skillsApi.syncSource(source.id);
      const text = `${source.name}: ${r.added.length} added, ${(r.updated ?? []).length} updated, ${(r.removed ?? []).length} removed` + notDone(r);
      setStatus(text);
      report(text);
    } catch (err) {
      setStatus(`${source.name} could not be synced: ${why(err)}`);
      report(`Syncing ${source.name} failed: ${why(err)}`, true);
    } finally {
      setBusy(null);
      readAgain();
      onChanged();
    }
  };

  const remove = async () => {
    const target = confirm;
    if (!target) return;
    setConfirm(null);
    setBusy(target.id);
    setStatus("");
    const withSkills = alsoRemove && target.skills.length > 0;
    try {
      await skillsApi.removeSource(target.id, withSkills);
      const text = `Removed ${target.name}${withSkills ? ` and its ${count(target.skills.length)}` : ""}`;
      setStatus(text);
      report(text);
    } catch (err) {
      setStatus(`${target.name} could not be removed: ${why(err)}`);
      report(`Removing ${target.name} failed: ${why(err)}`, true);
    } finally {
      setBusy(null);
      setAlsoRemove(false);
      readAgain();
      onChanged();
    }
  };

  const picked = catalog.find((c) => c.url === form.url.trim());

  return (
    <Dialog
      title="Skill sources"
      wide
      onClose={() => {
        if (busy === null && !adding) onClose();
      }}
      footer={
        <>
          <span className="hint">A sync brings what the source holds now. New and changed skills arrive switched off.</span>
          <span className="spacer" />
          <button type="button" onClick={onClose}>
            Close
          </button>
        </>
      }
    >
      {loadError && (
        <p className="sk-error" role="alert">
          {loadError}
        </p>
      )}
      {status && (
        <p className="sk-src-status" role="status">
          {status}
        </p>
      )}
      {!sources && !loadError && <p className="hint">Reading the sources...</p>}
      {sources && sources.length === 0 && <p className="hint">No source yet. Add one below.</p>}
      {sources && sources.length > 0 && (
        <table className="sk-src">
          <colgroup>
            <col className="c-name" />
            <col className="c-url" />
            <col className="c-skills" />
            <col className="c-sync" />
            <col className="c-act" />
          </colgroup>
          <thead>
            <tr>
              <th>Source</th>
              <th>URL or folder</th>
              <th>Skills</th>
              <th>Last sync and errors</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {sources.map((s) => {
              const mine = busy === s.id;
              return (
                <tr key={s.id} data-source={s.id}>
                  <td>
                    <span className="sk-src-name">{s.name}</span>
                    <span className="hint sk-src-where">{[s.ref || "default branch", s.commit ? `at ${s.commit.slice(0, 8)}` : ""].filter(Boolean).join(" ")}</span>
                  </td>
                  <td>
                    <code title={s.url}>{s.url}</code>
                    {s.subdir && <span className="hint sk-src-where">{s.subdir}</span>}
                  </td>
                  <td title={s.skills.map((k) => `${k.name} (synced ${ago(k.syncedAt)})`).join(", ")}>{s.skills.length}</td>
                  <td>
                    {s.lastSync ? ago(s.lastSync) : "never"}
                    {s.lastError && (
                      <span className="sk-error" title={s.lastError}>
                        {s.lastError}
                      </span>
                    )}
                  </td>
                  <td>
                    {confirm?.id === s.id ? (
                      <div className="sk-src-confirm">
                        {s.skills.length > 0 && (
                          <label className="hint">
                            <input type="checkbox" checked={alsoRemove} onChange={(e) => setAlsoRemove(e.target.checked)} /> also remove the {count(s.skills.length)} it installed
                          </label>
                        )}
                        <div className="sk-src-act">
                          <button type="button" className="danger" disabled={mine} onClick={() => void remove()}>
                            Remove it
                          </button>
                          <button
                            type="button"
                            disabled={mine}
                            onClick={() => {
                              setConfirm(null);
                              setAlsoRemove(false);
                            }}
                          >
                            Keep it
                          </button>
                        </div>
                      </div>
                    ) : (
                      <div className="sk-src-act">
                        <button type="button" disabled={busy !== null} onClick={() => void sync(s)}>
                          {mine ? "Working..." : "Sync"}
                        </button>
                        <button
                          type="button"
                          className="danger"
                          disabled={busy !== null}
                          onClick={() => {
                            setConfirm(s);
                            setAlsoRemove(false);
                          }}
                        >
                          Remove
                        </button>
                      </div>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <form className="ui-form sk-src-add" onSubmit={(e) => void add(e)} noValidate>
        <div className="ui-form-grid">
          <Field label="URL or folder" hint="https://github.com/<owner>/<repository> is cloned; a path on this computer is read where it is.">
            <input value={form.url} placeholder="https://github.com/owner/repo" aria-invalid={!!formError} onChange={set("url")} />
          </Field>
          <Field label="Branch, tag or commit" hint="Optional. Empty is the default branch.">
            <input value={form.ref ?? ""} onChange={set("ref")} />
          </Field>
          <Field label="Subfolder" hint="Optional. The folder inside the repository that holds the skills.">
            <input value={form.subdir ?? ""} placeholder="skills" onChange={set("subdir")} />
          </Field>
          <Field label="Name" hint="Optional. Empty is the name of the repository.">
            <input value={form.name ?? ""} onChange={set("name")} />
          </Field>
        </div>
        <div className="sk-src-catalog">
          <span className="hint">Suggestions:</span>
          {catalog.map((c) => (
            <button
              type="button"
              key={c.url}
              title={`${c.note}. Click to fill the form.`}
              onClick={() => {
                setForm({ url: c.url, ref: "", subdir: c.subdir, name: c.name });
                setFormError("");
              }}
            >
              {c.name}
            </button>
          ))}
          {picked && <span className="hint">{picked.note}</span>}
        </div>
        {formError && (
          <p className="sk-error" role="alert">
            {formError}
          </p>
        )}
        <div className="row">
          <button className="primary" type="submit" disabled={adding || busy !== null}>
            {adding ? "Cloning..." : "Add source"}
          </button>
        </div>
      </form>
    </Dialog>
  );
}
