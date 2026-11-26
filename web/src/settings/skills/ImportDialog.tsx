// Import skills from other tools (skills.md sections 7 and 9): the folders that were
// found and the skills they hold, what will be imported and how, and what the import did.
import { useEffect, useState } from "react";
import { skillsApi, type ImportMode, type ImportResult, type ImportSource } from "../../api";
import Dialog from "../dialog";
import "./dialogs.css";

interface Props {
  onClose: () => void;
  /** The page reads its list again. */
  onImported: () => void;
  report: (text: string, failed?: boolean) => void;
}

const why = (e: unknown) => (e instanceof Error ? e.message : String(e));
const skills = (n: number) => `${n} ${n === 1 ? "skill" : "skills"}`;

export default function ImportDialog({ onClose, onImported, report }: Props) {
  const [sources, setSources] = useState<ImportSource[] | null>(null);
  const [error, setError] = useState("");
  const [checked, setChecked] = useState<string[]>([]);
  const [mode, setMode] = useState<ImportMode>("copy");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ImportResult | null>(null);

  // The folders of other tools under the home directory, and the skills they hold.
  useEffect(() => {
    skillsApi
      .importSources()
      .then((list) => {
        setSources(list);
        setError("");
      })
      .catch((e: unknown) => setError(why(e)));
  }, []);

  const found = (sources ?? []).filter((s) => s.exists);
  const missing = (sources ?? []).filter((s) => !s.exists).map((s) => s.agent);
  /** What can still be imported: not installed, in a folder that was found. */
  const open = found.flatMap((s) => s.skills).filter((c) => !c.installed);
  const isNew = (name: string) => open.some((c) => c.name === name);
  const isChecked = (name: string) => isNew(name) && checked.includes(name);
  const picked = open.filter((c) => isChecked(c.name)).length;

  const setMany = (names: string[], on: boolean) =>
    setChecked((list) => (on ? [...new Set([...list, ...names])] : list.filter((n) => !names.includes(n))));

  const run = async () => {
    setBusy(true);
    setResult(null);
    setError("");
    const total: ImportResult = { imported: [], skipped: [], disabled: [] };
    try {
      // One request per folder. The daemon checks each name again before it writes.
      for (const source of found) {
        const names = source.skills.filter((c) => isChecked(c.name)).map((c) => c.name);
        if (names.length === 0) continue;
        const part = await skillsApi.import(source.path, names, mode);
        total.imported.push(...part.imported);
        total.skipped.push(...part.skipped);
        total.disabled.push(...part.disabled);
      }
      setChecked([]);
      report(total.imported.length ? `Imported ${skills(total.imported.length)}` : "Nothing was imported");
    } catch (e) {
      setError(why(e));
    } finally {
      // What arrived is in the list; what was skipped is said here.
      if (total.imported.length > 0 || total.skipped.length > 0) setResult(total);
      setBusy(false);
      onImported();
      // The tree shows what is installed now.
      skillsApi.importSources().then(setSources).catch(() => {});
    }
  };

  return (
    <Dialog
      title="Import skills"
      wide
      onClose={() => {
        if (!busy) onClose();
      }}
      footer={
        <>
          <button type="button" disabled={busy || open.length === 0} onClick={() => setMany(open.map((c) => c.name), true)}>
            Select all
          </button>
          <button type="button" disabled={busy || checked.length === 0} onClick={() => setChecked([])}>
            None
          </button>
          <span className="sk-imp-mode" role="radiogroup" aria-label="How to import">
            <label title="A copy under KAgent's imported folder. The folder of the other tool is not changed.">
              <input type="radio" name="sk-import-mode" checked={mode === "copy"} disabled={busy} onChange={() => setMode("copy")} /> Copy
            </label>
            <label title="A link to the folder of the other tool. Deleting the skill removes the link only.">
              <input type="radio" name="sk-import-mode" checked={mode === "symlink"} disabled={busy} onChange={() => setMode("symlink")} /> Link
            </label>
          </span>
          <span className="spacer" />
          <button type="button" disabled={busy} onClick={onClose}>
            Close
          </button>
          <button type="button" className="primary" disabled={busy || picked === 0} onClick={() => void run()}>
            {busy ? "Importing..." : picked ? `Import ${skills(picked)}` : "Import"}
          </button>
        </>
      }
    >
      <p className="hint">Skills of other tools on this computer. Pick the ones KAgent should know.</p>
      {error && (
        <p className="sk-error" role="alert">
          {error}
        </p>
      )}
      {!sources && !error && <p className="hint">Looking for the skills of other tools...</p>}
      {sources && found.length === 0 && <p className="hint">No folder of skills of another tool was found.</p>}
      {found.map((source) => {
        const theirs = source.skills.filter((c) => !c.installed).map((c) => c.name);
        const some = theirs.filter(isChecked).length;
        return (
          <section key={source.path} className="sk-imp-source" data-agent={source.agent}>
            <label className="sk-imp-head">
              <input
                type="checkbox"
                aria-label={`The new skills of ${source.agent}`}
                checked={theirs.length > 0 && some === theirs.length}
                ref={(el) => {
                  if (el) el.indeterminate = some > 0 && some < theirs.length;
                }}
                disabled={busy || theirs.length === 0}
                onChange={(e) => setMany(theirs, e.target.checked)}
              />
              <span className="sk-imp-agent">{source.agent}</span>
              <code className="sk-imp-path" title={source.path}>
                {source.path}
              </code>
              <span className="hint">
                {skills(source.skills.length)}, {theirs.length} new
              </span>
            </label>
            <ul className="sk-imp-list">
              {source.skills.map((c) => (
                <li key={c.path} className={c.installed ? "installed" : undefined}>
                  <label className="sk-imp-skill" title={`${c.description}\n${c.path}${c.realPath !== c.path ? ` (same folder as ${c.realPath})` : ""}`}>
                    <input type="checkbox" checked={isChecked(c.name)} disabled={busy || c.installed} onChange={(e) => setMany([c.name], e.target.checked)} />
                    <span className="sk-imp-name">{c.name}</span>
                    <span className="sk-imp-desc">{c.description || "no description"}</span>
                  </label>
                  {c.installed && <span className="sk-imp-state">{c.duplicateOf && c.duplicateOf !== c.name ? `same as ${c.duplicateOf}` : "installed"}</span>}
                </li>
              ))}
            </ul>
          </section>
        );
      })}
      {sources && missing.length > 0 && <p className="hint">Not found: {missing.join(", ")}.</p>}
      {result && (
        <div className="sk-imp-result" role="status">
          <p>{result.imported.length ? `Imported ${result.imported.length}: ${result.imported.join(", ")}` : "Nothing was imported."}</p>
          {result.skipped.length > 0 && <p className="hint">Not imported: {result.skipped.map((s) => `${s.name} (${s.reason})`).join(", ")}.</p>}
          {result.disabled.length > 0 && <p>They are off until you enable them.</p>}
        </div>
      )}
      <p className="sk-imp-off">Imported skills are off until you enable them.</p>
    </Dialog>
  );
}
