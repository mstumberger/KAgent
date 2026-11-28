// Alerts raised by the watchers, sentinel and network monitor.
import { useEffect, useState } from "react";
import { api, type Alert } from "../api";
import { openSession, refreshSessions, refreshState, setView, useStore } from "../store";
import { Page, PageHeader } from "../components/page/Page";

export default function Alerts() {
  const s = useStore();
  const [alerts, setAlerts] = useState<Alert[]>([]);
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);

  useEffect(() => {
    void api.get<Alert[]>(`/api/alerts?limit=200${all ? "" : "&status=open"}`).then(setAlerts);
  }, [all, s.alertsTick]);

  const ack = async (id: string) => {
    await api.post(`/api/alerts/${id}/ack`, {});
    setAlerts((a) => (all ? a.map((x) => (x.id === id ? { ...x, status: "acked" } : x)) : a.filter((x) => x.id !== id)));
    void refreshState();
  };
  const investigate = async (id: string) => {
    setBusy(id);
    const r = await api.post<{ sessionId: string }>(`/api/alerts/${id}/investigate`);
    await refreshSessions();
    await openSession(r.sessionId);
    setView("chat");
  };

  return (
    <Page>
      <PageHeader
        title="Alerts"
        actions={
          <label className="inline">
            <input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} /> show acknowledged
          </label>
        }
      />
      {alerts.length === 0 && <div className="empty">No {all ? "" : "open "}alerts. KAgent watches the journal, services, disks, temperatures, SSH, persistence and (if configured) your router.</div>}
      {alerts.map((a) => (
        <div key={a.id} className={`alert-card sev-${a.severity} st-${a.status}`}>
          <div className="alert-head">
            <span className={`sev sev-${a.severity}`}>{a.severity}</span>
            <span className="alert-title">{a.title}</span>
            {a.count > 1 && <span className="count">x{a.count}</span>}
            <span className="spacer" />
            <span className="ts">{new Date(a.lastSeen).toLocaleString()}</span>
          </div>
          <div className="alert-cause">{a.cause}</div>
          {a.evidence?.[0]?.text && <pre className="alert-evidence">{a.evidence.map((e) => e.text).join("\n")}</pre>}
          <div className="alert-proposal">Suggested: {a.proposal}</div>
          <div className="alert-actions">
            <button className="primary" disabled={busy === a.id} onClick={() => void investigate(a.id)}>
              Investigate
            </button>
            {a.status === "open" && <button onClick={() => void ack(a.id)}>Acknowledge</button>}
            <span className="rule">{a.rule}</span>
          </div>
        </div>
      ))}
    </Page>
  );
}
