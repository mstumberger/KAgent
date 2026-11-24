// Everything KAgent changed on this machine, with undo.
import { useEffect, useState } from "react";
import { api, type Change } from "../api";
import { useStore } from "../store";

export default function Changes() {
  const s = useStore();
  const [changes, setChanges] = useState<Change[]>([]);
  const [msg, setMsg] = useState("");

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

  return (
    <div className="page">
      <div className="page-head">
        <h2>Changes</h2>
        <span className="hint">Every change KAgent made, newest first. Undo restores the recorded previous state.</span>
      </div>
      {msg && <pre className="banner">{msg}</pre>}
      {changes.length === 0 && <div className="empty">No changes yet.</div>}
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
          {changes.map((c) => (
            <tr key={c.id} className={`ch-${c.status}`}>
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
                {c.status === "applied" && c.undo ? <button onClick={() => void undo(c)}>Undo</button> : <span className="sub">{c.undo ? "" : "no undo"}</span>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
