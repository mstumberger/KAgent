// One provider: its connection, its key and its models. A role given to a model is a
// settings save (useSettingPatch); everything else goes through `run`, and the shell
// loads the lists again and shows the result in the header.
import { useMemo, useState } from "react";
import { api, type ProviderView, type Settings, type Template } from "../../../api";
import { Badge, Confirm, Toggle, type BadgeTone } from "../../ui";
import { useSettingPatch } from "../../useSettingPatch";
import { Logo } from "./Catalog";
import { API_FORMATS, statusOf, STATUS_LABEL, type Status } from "./shared";

const TONE: Record<Status, BadgeTone> = { ok: "ok", error: "bad", nokey: "warn", off: "neutral" };
type TestResult = { ok: boolean; latencyMs: number; error?: string; reply?: string };

export default function ProviderDetail({
  p,
  template,
  settings,
  busy,
  run,
}: {
  p: ProviderView;
  template?: Template;
  settings?: Settings;
  busy: boolean;
  run: (label: string, fn: () => Promise<string | void>) => Promise<void>;
}) {
  const { save } = useSettingPatch();
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.api.baseUrl);
  const [key, setKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [filter, setFilter] = useState("");
  const [onlyEnabled, setOnlyEnabled] = useState(p.models.length > 15);
  const [askDelete, setAskDelete] = useState(false);
  const dirty = name !== p.name || baseUrl !== p.api.baseUrl || key.trim() !== "";
  const st = statusOf(p);

  const models = useMemo(
    () => p.models.filter((m) => (!onlyEnabled || m.enabled) && m.id.toLowerCase().includes(filter.toLowerCase())),
    [p.models, filter, onlyEnabled],
  );

  /** The roles `ref` has now (chat, fallback, voice, vision, fast). */
  const roles = (ref: string) =>
    (
      [
        ["chat", settings?.chatModel],
        ["fallback", settings?.fallbackModel],
        ["voice", settings?.liveModel],
        ["vision", settings?.visionModel],
        ["fast", settings?.fastModel],
      ] as [string, string | undefined][]
    )
      .filter(([, current]) => current === ref)
      .map(([role]) => role);

  return (
    <div className="pv-provider">
      <div className="pv-detail-head">
        <div className="pv-title">
          <Logo name={p.name} />
          <div>
            <h3>{p.name}</h3>
            <Badge tone={TONE[st]}>{STATUS_LABEL[st]}</Badge>
          </div>
        </div>
        <Toggle
          checked={p.enabled}
          disabled={busy}
          title={p.enabled ? "Disable provider" : "Enable provider"}
          onChange={(v) => void run(v ? "Enable" : "Disable", async () => void (await api.put(`/api/providers/${p.id}`, { enabled: v })))}
        />
      </div>
      {p.lastError && <div className="pv-feedback bad">{p.lastError}</div>}

      <div className="pv-form">
        <label className="pv-field">
          <span>Name</span>
          <input value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="pv-field">
          <span>API format</span>
          <input value={API_FORMATS[p.api.type] ?? p.api.type} readOnly title="Set by the provider template (read only)" />
        </label>
        <label className="pv-field">
          <span>Base URL</span>
          <input value={baseUrl} placeholder="https://api.example.com/v1" onChange={(e) => setBaseUrl(e.target.value)} />
        </label>
        <label className="pv-field">
          <span>API key</span>
          <span className="pv-key">
            <input
              type={showKey ? "text" : "password"}
              placeholder={p.hasKey ? `Stored (${p.apiKeyMasked}) - type to replace` : "Paste the API key"}
              value={key}
              onChange={(e) => setKey(e.target.value)}
            />
            <button type="button" onClick={() => setShowKey(!showKey)}>
              {showKey ? "Hide" : "Show"}
            </button>
          </span>
          {template?.keyUrl && (
            <a className="hint" href={template.keyUrl} target="_blank" rel="noreferrer">
              Get a key
            </a>
          )}
        </label>
        <div className="row">
          <button
            className="primary"
            disabled={busy || !dirty}
            onClick={() =>
              void run("Save", async () => {
                await api.put(`/api/providers/${p.id}`, { name, baseUrl, ...(key.trim() ? { apiKey: key.trim() } : {}) });
                setKey("");
                return `${name} saved`;
              })
            }
          >
            Save
          </button>
          <button
            disabled={busy}
            onClick={() =>
              void run("Test connection", async () => {
                const r = await api.post<TestResult>(`/api/providers/${p.id}/test`, {});
                if (!r.ok) throw new Error(r.error ?? "no reply");
                return `Connected in ${r.latencyMs} ms`;
              })
            }
          >
            Test connection
          </button>
          <button disabled={busy} onClick={() => void run("Fetch models", async () => void (await api.post(`/api/providers/${p.id}/fetch-models`)))}>
            Fetch models
          </button>
          <span className="spacer" />
          <button className="danger" disabled={busy} onClick={() => setAskDelete(true)}>
            Delete
          </button>
        </div>
      </div>

      <div className="pv-models">
        <div className="pv-models-head">
          <h4>
            Models <span className="hint">{p.models.filter((m) => m.enabled).length} of {p.models.length} enabled</span>
          </h4>
          <input className="pv-search small" placeholder="Filter models" value={filter} onChange={(e) => setFilter(e.target.value)} />
          <label className="check-inline">
            <input type="checkbox" checked={onlyEnabled} onChange={(e) => setOnlyEnabled(e.target.checked)} /> Enabled only
          </label>
        </div>
        {p.models.length === 0 && <p className="hint">No models yet: Fetch models.</p>}
        <ul className="pv-model-list">
          {models.map((m) => {
            const ref = `${p.id}::${m.id}`;
            const isLive = m.roles.includes("live") || m.id.includes("-live") || m.id.includes("native-audio");
            const uses = roles(ref);
            return (
              <li key={m.id} className={`pv-model${m.enabled ? "" : " off"}`}>
                <Toggle
                  checked={m.enabled}
                  disabled={busy}
                  title={m.enabled ? "Disable model" : "Enable model"}
                  onChange={(v) => void run("Model", async () => void (await api.put(`/api/providers/${p.id}`, { models: [{ id: m.id, enabled: v }] })))}
                />
                <span className="pv-model-id">{m.id}</span>
                {uses.map((u) => (
                  <Badge key={u} tone="accent">
                    {u}
                  </Badge>
                ))}
                <span className="spacer" />
                {m.enabled && !isLive && !uses.includes("chat") && (
                  <button className="pv-mini" disabled={busy} onClick={() => void save("chatModel", ref)}>
                    Use for chat
                  </button>
                )}
                {m.enabled && !isLive && !uses.includes("fallback") && (
                  <button className="pv-mini" disabled={busy} onClick={() => void save("fallbackModel", ref)}>
                    Fallback
                  </button>
                )}
                {m.enabled && isLive && !uses.includes("voice") && (
                  <button className="pv-mini" disabled={busy} onClick={() => void save("liveModel", ref)}>
                    Use for voice
                  </button>
                )}
                {!isLive && (
                  <button
                    className="pv-mini"
                    disabled={busy}
                    onClick={() =>
                      void run("Test", async () => {
                        const r = await api.post<TestResult>(`/api/providers/${p.id}/test`, { modelId: m.id });
                        if (!r.ok) throw new Error(r.error ?? "no reply");
                        return `${m.id}: ${r.latencyMs} ms, "${(r.reply ?? "").slice(0, 60)}"`;
                      })
                    }
                  >
                    Test
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      </div>

      {askDelete && (
        <Confirm
          title={`Delete ${p.name}?`}
          body="Its API key is removed from the keyring. A role given to one of its models has to be given to another."
          confirmLabel="Delete"
          danger
          onCancel={() => setAskDelete(false)}
          onConfirm={() => {
            setAskDelete(false);
            void run("Delete", async () => void (await api.del(`/api/providers/${p.id}`)));
          }}
        />
      )}
    </div>
  );
}
