import { useEffect, useState } from "react";
import { api } from "../api";
import { Page, PageHeader } from "../components/page/Page";

interface Snapshot {
  hostname: string;
  distro: string;
  kernel: string;
  arch: string;
  user: string;
  uptimeHours: number;
  loadAvg: string[];
  cpu: { model: string; logicalCores: number };
  memoryMiB: { total: number; available: number; swapTotal: number; swapFree: number };
  temperatures: { zone: string; celsius: number }[];
  battery: { capacityPercent: string; status: string } | null;
}

interface Check {
  name: string;
  status: "ok" | "warn" | "bad";
  detail: string;
}

function Doctor() {
  const [r, setR] = useState<{ score: number; checks: Check[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      setR(await api.get<{ score: number; checks: Check[] }>("/api/doctor"));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    void run();
  }, []);
  const color = !r ? "" : r.score >= 85 ? "ok-text" : r.score >= 60 ? "warn-text" : "bad-text";
  return (
    <section className="card doctor">
      <div className="doctor-head">
        <h3>Health</h3>
        {r && <span className={`score ${color}`}>{r.score}/100</span>}
        <button disabled={busy} onClick={() => void run()}>
          {busy ? "checking..." : "re-check"}
        </button>
      </div>
      {r?.checks.map((c) => (
        <div key={c.name} className={`check check-${c.status}`}>
          <span className="dotc" /> <b>{c.name}</b> <span className="sub">{c.detail}</span>
        </div>
      ))}
    </section>
  );
}

export default function System() {
  const [snap, setSnap] = useState<Snapshot | null>(null);

  useEffect(() => {
    void api.get<Snapshot>("/api/system").then(setSnap);
    const t = setInterval(() => void api.get<Snapshot>("/api/system").then(setSnap), 10000);
    return () => clearInterval(t);
  }, []);

  if (!snap) return <Page>loading...</Page>;

  return (
    <Page>
      <PageHeader title="System" />
      <Doctor />
      <div className="cards-grid">
        <div className="card">
          <h3>Machine</h3>
          <div>
            {snap.hostname} - {snap.user}
          </div>
          <div className="hint">
            {snap.distro} - kernel {snap.kernel} - {snap.arch}
          </div>
          <div>uptime {snap.uptimeHours} h</div>
          <div>load {snap.loadAvg.join(" ")}</div>
        </div>
        <div className="card">
          <h3>CPU</h3>
          <div>{snap.cpu.model}</div>
          <div>{snap.cpu.logicalCores} logical cores</div>
          <h3>Memory</h3>
          <div>
            {snap.memoryMiB.available} / {snap.memoryMiB.total} MiB available
          </div>
          <div className="hint">
            swap free {snap.memoryMiB.swapFree} / {snap.memoryMiB.swapTotal} MiB
          </div>
        </div>
        <div className="card">
          <h3>Temperatures</h3>
          {snap.temperatures.length === 0 && <div className="hint">no thermal zones</div>}
          {snap.temperatures.map((t) => (
            <div key={t.zone}>
              {t.zone}: {t.celsius} C
            </div>
          ))}
          {snap.battery && (
            <>
              <h3>Battery</h3>
              <div>
                {snap.battery.capacityPercent}% ({snap.battery.status})
              </div>
            </>
          )}
        </div>
      </div>
    </Page>
  );
}
