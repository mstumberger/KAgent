// Persona, language, root helper, MCP exposure, desktop (editor, projects, browser,
// voice), watcher quiet hours.
import { useEffect, useState } from "react";
import { api } from "../api";
import { refreshState, useStore } from "../store";

const GROUPS = ["system", "kernel", "services", "files", "shell", "hardware", "skills", "change", "desktop", "security", "trace", "router"];

const EDITORS: [string, string][] = [
  ["auto", "Auto (project's own, then first installed)"],
  ["vscode", "VS Code"],
  ["kate", "Kate"],
  ["idea", "IntelliJ IDEA"],
  ["pycharm", "PyCharm"],
  ["webstorm", "WebStorm"],
  ["phpstorm", "PhpStorm"],
  ["clion", "CLion"],
  ["zed", "Zed"],
];

function StartupCard() {
  const s = useStore();
  const st = s.state;
  const [startup, setStartup] = useState({ autostart: true, greeting: { enabled: true, text: "" } });
  useEffect(() => {
    if (st) {
      const su = st.settings.startup;
      setStartup((prev) => ({ ...prev, ...su, greeting: { ...prev.greeting, ...su?.greeting } }));
    }
  }, [st]);
  const save = async () => {
    await api.put("/api/settings", { startup });
    await refreshState();
  };
  return (
    <div className="form-grid">
      <label className="check">
        <input
          type="checkbox"
          checked={startup.autostart}
          onChange={(e) => setStartup({ ...startup, autostart: e.target.checked })}
        />
        Start with my session (systemd user service; no root needed)
      </label>
      <label className="check">
        <input
          type="checkbox"
          checked={startup.greeting.enabled}
          onChange={(e) => setStartup({ ...startup, greeting: { ...startup.greeting, enabled: e.target.checked } })}
        />
        Greet me when KAgent starts (desktop notification)
      </label>
      <label>
        Greeting text (empty = auto: time of day + your name)
        <input
          value={startup.greeting.text}
          placeholder="Good evening, Marko. KAgent is up and watching."
          onChange={(e) => setStartup({ ...startup, greeting: { ...startup.greeting, text: e.target.value } })}
        />
      </label>
      <div>
        <button className="primary" onClick={() => void save()}>
          Save startup
        </button>
      </div>
    </div>
  );
}

export default function Settings() {
  const s = useStore();
  const st = s.state;
  const [persona, setPersona] = useState({ name: "KAgent", address: "", language: "auto" });
  const [saved, setSaved] = useState("");
  const [chatModels, setChatModels] = useState<string[]>([]);
  useEffect(() => {
    void api.get<Record<string, { ref: string }[]>>("/api/models").then((m) => setChatModels((m.chat ?? []).map((x) => x.ref)));
  }, []);

  useEffect(() => {
    if (st) setPersona(st.settings.persona);
  }, [st]);

  if (!st) return <div className="page">loading...</div>;
  const set = async (patch: Record<string, unknown>) => {
    try {
      await api.put("/api/settings", patch);
      await refreshState();
      setSaved("saved");
    } catch (e) {
      setSaved(`not saved: ${e instanceof Error ? e.message : String(e)}`);
    }
    setTimeout(() => setSaved(""), 2500);
  };
  const groups = st.settings.toolGroups ?? [];
  const mcp = (st.settings.mcpToolGroups as string[] | undefined)?.length ? (st.settings.mcpToolGroups as string[]) : ["system", "kernel", "services", "files", "hardware"];
  const toggle = (list: string[], g: string) => (list.includes(g) ? list.filter((x) => x !== g) : [...list, g]);
  const watchers = (st.settings.watchers as { quietHours?: string } | undefined) ?? {};
  const desktop = (st.settings.desktop as { editor?: string; projectRoots?: string[] } | undefined) ?? {};
  const browser = (st.settings.browser as { port?: number; binary?: string } | undefined) ?? {};
  const voice = (st.settings.voice as Record<string, unknown> | undefined) ?? {};

  return (
    <div className="page">
      <div className="page-head">
        <h2>Settings</h2>
        {saved && <span className="ok-text">{saved}</span>}
      </div>
      <section className="card">
        <h3>Persona</h3>
        <div className="form-grid">
          <label>Name<input value={persona.name} onChange={(e) => setPersona({ ...persona, name: e.target.value })} /></label>
          <label>Address me as<input value={persona.address} onChange={(e) => setPersona({ ...persona, address: e.target.value })} /></label>
          <label>
            Language
            <select value={persona.language} onChange={(e) => setPersona({ ...persona, language: e.target.value })}>
              <option value="auto">follow me (Slovenian/English)</option>
              <option value="sl">Slovenian</option>
              <option value="en">English</option>
            </select>
          </label>
        </div>
        <button className="primary" onClick={() => void set({ persona })}>
          Save persona
        </button>
      </section>

      <section className="card">
        <h3>Startup</h3>
        <StartupCard />
      </section>

      <section className="card">
        <h3>Root helper</h3>
        <p>
          Status: <b className={st.privHelper === "installed" ? "ok-text" : "bad-text"}>{st.privHelper}</b>
        </p>
        {st.privHelper !== "installed" && (
          <p className="hint">
            Root actions (sysctl, modules, system services, packages, SMART, eBPF) need it. Run <code>kagentd sudoers</code> in a terminal and paste the printed commands. KAgent never
            runs sudo itself.
          </p>
        )}
        <label>
          Privilege method
          <select value={String(st.settings.privilege ?? "sudo")} onChange={(e) => void set({ privilege: e.target.value })}>
            <option value="sudo">sudo -n (sudoers rule, no prompt)</option>
            <option value="pkexec">pkexec (password prompt each time)</option>
            <option value="none">none (no root actions)</option>
          </select>
        </label>
      </section>

      <section className="card">
        <h3>Tools the agent may use</h3>
        <div className="chips">
          {GROUPS.map((g) => (
            <label key={g} className={`chip ${groups.includes(g) ? "on" : ""}`}>
              <input type="checkbox" checked={groups.includes(g)} onChange={() => void set({ toolGroups: toggle(groups, g) })} /> {g}
            </label>
          ))}
        </div>
        <h3>Exposed to other agents over MCP</h3>
        <p className="hint">
          <code>claude mcp add kagent -- kagentd mcp</code> lets Claude Code use these, with approvals here.
        </p>
        <div className="chips">
          {GROUPS.map((g) => (
            <label key={g} className={`chip ${mcp.includes(g) ? "on" : ""}`}>
              <input type="checkbox" checked={mcp.includes(g)} onChange={() => void set({ mcpToolGroups: toggle(mcp, g) })} /> {g}
            </label>
          ))}
        </div>
      </section>

      <section className="card">
        <h3>Models</h3>
        <label>
          Fallback chat model (used when the chat model is busy or out of quota)
          <select value={(st.settings.fallbackModel as string) || ""} onChange={(e) => void set({ fallbackModel: e.target.value })}>
            <option value="">(none)</option>
            {chatModels.map((m) => (
              <option key={m}>{m}</option>
            ))}
          </select>
        </label>
        <label>
          Vision model (reads the screen for "what does this say?"; auto = the first Gemini flash model)
          <select value={(st.settings.visionModel as string) || ""} onChange={(e) => void set({ visionModel: e.target.value })}>
            <option value="">(auto)</option>
            {chatModels.map((m) => (
              <option key={m}>{m}</option>
            ))}
          </select>
        </label>
      </section>

      <section className="card">
        <h3>Desktop and apps</h3>
        <label>
          Editor for "show me the code"
          <select value={desktop.editor || "auto"} onChange={(e) => void set({ desktop: { ...desktop, editor: e.target.value === "auto" ? "" : e.target.value } })}>
            {EDITORS.map(([id, name]) => (
              <option key={id} value={id}>
                {name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Project folders (one per line; their subfolders are projects; empty = ~/SystemInterface, ~/projects, ~/src, ...)
          <textarea
            rows={3}
            defaultValue={(desktop.projectRoots ?? []).join("\n")}
            onBlur={(e) => void set({ desktop: { ...desktop, projectRoots: e.target.value.split("\n").map((x) => x.trim()).filter(Boolean) } })}
          />
        </label>
        <label>
          Agent browser DevTools port (KAgent's own Brave/Chrome profile)
          <input type="number" defaultValue={browser.port ?? 9333} onBlur={(e) => void set({ browser: { ...browser, port: Number(e.target.value) } })} />
        </label>
        <label className="check">
          <input type="checkbox" checked={voice.pauseMedia !== false} onChange={(e) => void set({ voice: { ...voice, pauseMedia: e.target.checked } })} />
          Pause music and videos while I talk to KAgent
        </label>
      </section>

      <section className="card">
        <h3>Watchers</h3>
        <label>
          Quiet hours (no non-critical notifications)
          <input defaultValue={watchers.quietHours ?? "23:00-07:00"} onBlur={(e) => void set({ watchers: { ...watchers, quietHours: e.target.value } })} />
        </label>
      </section>
    </div>
  );
}
