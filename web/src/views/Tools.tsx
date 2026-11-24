import { useEffect, useState } from "react";
import { api, type ToolView } from "../api";
import { refreshState, useStore } from "../store";

export default function Tools() {
  const s = useStore();
  const [tools, setTools] = useState<ToolView[]>([]);

  useEffect(() => {
    void api.get<{ tools: ToolView[] }>("/api/tools").then((r) => setTools(r.tools));
  }, []);

  const setMode = async (mode: string) => {
    await api.put("/api/settings", { permissionMode: mode });
    await refreshState();
  };

  const deleteRule = async (index: number) => {
    const rules = (s.state?.settings.rules ?? []).filter((_, i) => i !== index);
    await api.put("/api/settings", { rules });
    await refreshState();
  };

  const groups = [...new Set(tools.map((t) => t.group))];
  const mode = s.state?.settings.permissionMode ?? "ask";

  return (
    <div className="page">
      <h2>Tools &amp; permissions</h2>
      <div className="card">
        <h3>Permission mode</h3>
        <div className="row">
          {["read-only", "ask", "trusted", "full"].map((m) => (
            <button key={m} className={m === mode ? "primary" : ""} disabled={m === mode} onClick={() => void setMode(m)}>
              {m}
            </button>
          ))}
        </div>
        <div className="hint">
          read-only denies every mutating call; ask shows approval cards (answer them by click or, in a voice session, by saying "yes" / "no"; root
          actions need "approve"); trusted auto-allows unprivileged changes but still asks for typing into terminals, messages and reboots; full
          control runs everything without asking except root (sudo) actions. Also in the tray menu.
        </div>
      </div>

      <div className="card">
        <h3>Always-allow rules</h3>
        {(s.state?.settings.rules ?? []).length === 0 && <div className="hint">none yet</div>}
        {(s.state?.settings.rules ?? []).map((r, i) => (
          <div key={i} className="row spread rule-row">
            <span>
              {r.tool}: <code>{r.prefix}...</code>
            </span>
            <button className="danger" onClick={() => void deleteRule(i)}>
              remove
            </button>
          </div>
        ))}
      </div>

      {groups.map((g) => (
        <div key={g} className="card">
          <h3>
            {g} <span className="hint">{tools.filter((t) => t.group === g).length} tools</span>
          </h3>
          <table>
            <thead>
              <tr>
                <th>tool</th>
                <th>risk</th>
                <th>flags</th>
                <th>description</th>
              </tr>
            </thead>
            <tbody>
              {tools
                .filter((t) => t.group === g)
                .map((t) => (
                  <tr key={t.contract.name} className={t.enabled ? "" : "disabled"}>
                    <td>{t.contract.name}</td>
                    <td>{t.contract.risk}</td>
                    <td>
                      {t.contract.readOnly ? "RO " : "MUT "}
                      {t.contract.privileged ? "PRIV " : ""}
                      {t.contract.destructive ? "DESTRUCTIVE" : ""}
                    </td>
                    <td>{t.contract.description.slice(0, 120)}</td>
                  </tr>
                ))}
            </tbody>
          </table>
        </div>
      ))}
    </div>
  );
}
