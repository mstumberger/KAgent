// Intrusion detection (honeytokens, persistence baseline) and LAN monitoring
// through a MikroTik router.
import { useEffect, useState } from "react";
import { api } from "../api";

interface SentinelState {
  honeytokens: { path: string; kind: string }[];
  baseline: boolean;
}
interface RouterCfg {
  host: string;
  user: string;
  fingerprint: string;
  flowPort: number;
  honeypotIp: string;
  trapUser: string;
  https: boolean;
}
interface NetState {
  configured: boolean;
  config: RouterCfg;
  router: { ok: boolean; error?: string; resource?: Record<string, unknown>; connections?: unknown };
}
interface Device {
  mac: string;
  ip: string;
  hostname: string;
  firstSeen: number;
  lastSeen: number;
}
interface Top {
  device: string;
  remote: string;
  port: number;
  proto: number;
  bytes: number;
}

export default function Security() {
  const [sent, setSent] = useState<SentinelState | null>(null);
  const [net, setNet] = useState<NetState | null>(null);
  const [devices, setDevices] = useState<Device[]>([]);
  const [top, setTop] = useState<Top[]>([]);
  const [form, setForm] = useState<RouterCfg & { password: string }>({ host: "", user: "kagent-ro", fingerprint: "", flowPort: 2055, honeypotIp: "", trapUser: "", https: true, password: "" });
  const [msg, setMsg] = useState("");
  const [report, setReport] = useState("");

  const load = async () => {
    setSent(await api.get<SentinelState>("/api/sentinel"));
    const n = await api.get<NetState>("/api/net");
    setNet(n);
    if (n.configured) {
      setForm((f) => ({ ...f, ...n.config, password: "" }));
      setDevices(await api.get<Device[]>("/api/net/devices"));
      setTop(await api.get<Top[]>("/api/net/top?minutes=60"));
    }
  };
  useEffect(() => {
    void load();
  }, []);

  const honey = async (enable: boolean) => {
    const r = await api.post<{ note?: string }>("/api/sentinel/honeytokens", { enable });
    setMsg(enable ? `Decoys created. ${r.note ?? ""}` : "Decoys removed.");
    void load();
  };
  const rebaseline = async () => {
    const r = await api.post<{ items: number }>("/api/sentinel/rebaseline");
    setMsg(`New persistence baseline: ${r.items} items. Changes from now on raise alerts.`);
    void load();
  };
  const saveRouter = async () => {
    setMsg("Connecting to the router...");
    const body: Record<string, unknown> = { ...form };
    if (!form.password) delete body.password;
    const r = await api.put<{ router: { ok: boolean; error?: string } }>("/api/net/router", body);
    setMsg(r.router.ok ? "Router connected; certificate pinned." : `Saved, but the router did not answer: ${r.router.error}`);
    void load();
  };
  const runReport = async () => {
    setReport("running...");
    const r = await api.post<{ output: string }>("/api/tools/security_report/call", { input: {}, sessionId: "ui" });
    setReport(r.output);
  };

  return (
    <div className="page">
      <h2>Security</h2>
      {msg && <pre className="banner">{msg}</pre>}
      <section className="card">
        <h3>Tripwires on this PC</h3>
        <p className="hint">
          Honeytokens are decoy credentials (~/.aws/credentials.legacy, ~/.ssh/id_backup, ~/.config/backup.env, ~/Documents/passwords-old.kdbx). Nothing legitimate opens them, so
          any access is a high alert.
        </p>
        <div className="row">
          {sent?.honeytokens.length ? (
            <>
              <span className="ok-text">{sent.honeytokens.length} decoys armed</span>
              <button onClick={() => void honey(false)}>Remove decoys</button>
            </>
          ) : (
            <button className="primary" onClick={() => void honey(true)}>
              Arm decoys
            </button>
          )}
        </div>
        <p className="hint">
          Persistence watch compares SSH keys, systemd units, autostart, cron, SUID programs, ld.so.preload, sudo/PAM and shell startup files against a baseline every 10 minutes.
          {sent?.baseline ? " Baseline exists." : " Baseline will be taken on the first run."}
        </p>
        <button onClick={() => void rebaseline()}>Accept current state as baseline</button>
        <button onClick={() => void runReport()}>Security report</button>
        {report && <pre className="tool-body">{report}</pre>}
      </section>

      <section className="card">
        <h3>Network (MikroTik)</h3>
        <p className="hint">
          Read-only REST user + Traffic Flow (IPFIX) to this PC. Run <code>kagentd mikrotik-setup &lt;this-PC-IP&gt;</code> for the RouterOS commands.
        </p>
        <div className="form-grid">
          <label>Router host<input value={form.host} placeholder="192.168.88.1" onChange={(e) => setForm({ ...form, host: e.target.value })} /></label>
          <label>User<input value={form.user} onChange={(e) => setForm({ ...form, user: e.target.value })} /></label>
          <label>Password<input type="password" value={form.password} placeholder={net?.configured ? "(unchanged)" : ""} onChange={(e) => setForm({ ...form, password: e.target.value })} /></label>
          <label>Flow port<input type="number" value={form.flowPort} onChange={(e) => setForm({ ...form, flowPort: Number(e.target.value) })} /></label>
          <label>Honeypot IP<input value={form.honeypotIp} placeholder="optional" onChange={(e) => setForm({ ...form, honeypotIp: e.target.value })} /></label>
          <label>Trap user<input value={form.trapUser} placeholder="admin2" onChange={(e) => setForm({ ...form, trapUser: e.target.value })} /></label>
          <label className="inline"><input type="checkbox" checked={form.https} onChange={(e) => setForm({ ...form, https: e.target.checked })} /> HTTPS (pinned certificate)</label>
        </div>
        {form.fingerprint && <div className="sub">pinned: {form.fingerprint}</div>}
        <button className="primary" disabled={!form.host} onClick={() => void saveRouter()}>
          Save and connect
        </button>
        {net?.configured && (
          <div className={net.router.ok ? "ok-text" : "bad-text"}>
            {net.router.ok ? `connected: ${String(net.router.resource?.["board-name"] ?? "")} RouterOS ${String(net.router.resource?.version ?? "")}` : `not reachable: ${net.router.error}`}
          </div>
        )}
      </section>

      {devices.length > 0 && (
        <section className="card">
          <h3>LAN devices ({devices.length})</h3>
          <table className="table">
            <tbody>
              {devices.map((d) => (
                <tr key={d.mac}>
                  <td>{d.ip}</td>
                  <td>{d.hostname || <span className="sub">unknown</span>}</td>
                  <td className="sub">{d.mac}</td>
                  <td className="sub">first seen {new Date(d.firstSeen * 1000).toLocaleDateString()}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
      {top.length > 0 && (
        <section className="card">
          <h3>Top traffic (last hour)</h3>
          <table className="table">
            <tbody>
              {top.map((t, i) => (
                <tr key={i}>
                  <td>{t.device}</td>
                  <td>{t.remote}:{t.port}</td>
                  <td className="num">{(t.bytes / 1048576).toFixed(1)} MiB</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
    </div>
  );
}
