// Everything KAgent changed on this machine, with undo; a file edit has its diff.
import { Fragment, useEffect, useState } from "react";
import { api, type Change } from "../api";
import DiffView from "../components/diff/DiffView";
import type { Diff, Texts } from "../components/diff/model";
import { useStore } from "../store";
import { Page, PageHeader } from "../components/page/Page";

/** `GET /api/changes/:id/diff` (diff.md section 3). */
interface ChangeDiff {
  id: string;
  path: string;
  after: "stored" | "now";
  before: string | null;
  afterText: string | null;
  diff: Diff;
}

/** A file edit KAgent made: the only changes that keep the texts a diff needs. */
const editsAFile = (c: Change) => c.tool === "write_file" || c.tool === "edit_file";

export default function Changes() {
  const s = useStore();
  const [changes, setChanges] = useState<Change[]>([]);
  const [msg, setMsg] = useState("");
  const [open, setOpen] = useState<string | null>(null);
  const [diffs, setDiffs] = useState<Record<string, ChangeDiff | string>>({});

  useEffect(() => {
    void api.get<Change[]>("/api/changes?limit=200").then(setChanges);
  }, [s.changesTick]);

  const undo = async (c: Change) => {
    setMsg(`Undoing ${c.summary}... (approve the card if asked)`);
    try {
      const r = await api.post<{ status: string; output: string }>(`/api/changes/${c.id}/undo`);
      setMsg(r.output);
    } catch (e) {
      setMsg(String(e));
    }
  };

  const toggle = (c: Change) => {
    if (open === c.id) {
      setOpen(null);
      return;
    }
    setOpen(c.id);
    if (diffs[c.id]) return;
    void api.get<ChangeDiff>(`/api/changes/${c.id}/diff`).then(
      (d) => setDiffs((all) => ({ ...all, [c.id]: d })),
      (e) => setDiffs((all) => ({ ...all, [c.id]: e instanceof Error ? e.message : String(e) })),
    );
  };
  const openInEditor = (path: string, line: number) => {
    void api.post("/api/ui/tools/open_in_editor/call", { input: { path, line } }).catch((e) => setMsg(String(e)));
  };

  return (
    <Page wide>
      <PageHeader title="Changes" description="Every change KAgent made, newest first. Undo restores the recorded previous state." />
      {msg && <pre className="banner">{msg}</pre>}
      {changes.length === 0 && <div className="empty">No changes yet.</div>}
      <div className="scroll-x">
      <table className="table">
        <thead>
          <tr>
            <th>When</th>
            <th>Change</th>
            <th>Before / after</th>
            <th>Status</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {changes.map((c) => {
            const d = diffs[c.id];
            const texts: Record<string, Texts> | undefined = d && typeof d !== "string" ? { [d.path]: { before: d.before, after: d.afterText } } : undefined;
            return (
              <Fragment key={c.id}>
                <tr className={`ch-${c.status}`}>
                  <td className="ts">{new Date(c.ts).toLocaleString()}</td>
                  <td>
                    {c.privileged && <span className="risk risk-critical">root</span>} {c.summary}
                    <div className="sub">{c.tool} {c.id}</div>
                  </td>
                  <td className="sub">
                    {c.before && <div>before: {c.before.slice(0, 120)}</div>}
                    {c.after && <div>after: {c.after.slice(0, 120)}</div>}
                  </td>
                  <td>{c.status.replace("_", " ")}</td>
                  <td>
                    {editsAFile(c) && (
                      <button type="button" aria-expanded={open === c.id} aria-label={`Diff of ${c.summary}`} onClick={() => toggle(c)}>
                        Diff
                      </button>
                    )}{" "}
                    {c.status === "applied" && c.undo ? <button onClick={() => void undo(c)}>Undo</button> : <span className="sub">{c.undo ? "" : "no undo"}</span>}
                  </td>
                </tr>
                {open === c.id && (
                  <tr className="ch-diff">
                    <td colSpan={5}>
                      {!d && <div className="sub">Reading the diff...</div>}
                      {typeof d === "string" && <div className="banner" role="alert">{d}</div>}
                      {d && typeof d !== "string" && (
                        <DiffView diff={d.diff} texts={texts} label={`Diff of ${c.summary}`} onOpenEditor={openInEditor} onRevert={c.status === "applied" && c.undo ? () => void undo(c) : undefined} />
                      )}
                      {d && typeof d !== "string" && d.after === "now" && <div className="sub">Recorded before the diff view: the right side is the file as it is now.</div>}
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
      </div>
    </Page>
  );
}
