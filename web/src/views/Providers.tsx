// Model providers, laid out like ZCode's settings: provider list on the left (status
// dot, add provider), the selected provider's connection and models on the right, and
// a searchable catalog for adding one. Keys go to the desktop keyring via the daemon.
import { useEffect, useMemo, useState } from "react";
import { api, type ProviderView, type Template } from "../api";
import { refreshState, useStore } from "../store";

const API_FORMATS: Record<string, string> = {
  "anthropic-messages": "Anthropic messages (/v1/messages)",
  "openai-chat-completions": "Chat completions (/v1/chat/completions)",
  "openai-responses": "Responses (/v1/responses)",
  "ollama-native": "Ollama native (/api/chat)",
};

type Status = "ok" | "error" | "nokey" | "off";

function statusOf(p: ProviderView): Status {
  if (!p.enabled) return "off";
  if (p.lastError) return "error";
  const local = p.api.type === "ollama-native" || p.api.baseUrl.includes("127.0.0.1") || p.api.baseUrl.includes("localhost");
  if (!p.hasKey && !local) return "nokey";
  return "ok";
}

const STATUS_LABEL: Record<Status, string> = { ok: "Connected", error: "Error", nokey: "No API key", off: "Disabled" };

function Logo({ name }: { name: string }) {
  const letter = name.trim().charAt(0).toUpperCase() || "?";
  return (
    <span className="pv-logo" aria-hidden="true">
      {letter}
    </span>
  );
}

export default function Providers() {
  const s = useStore();
  const [templates, setTemplates] = useState<Template[]>([]);
  const [providers, setProviders] = useState<ProviderView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const reload = async () => {
    const [t, p] = await Promise.all([api.get<{ templates: Template[] }>("/api/providers/templates"), api.get<{ providers: ProviderView[] }>("/api/providers")]);
    setTemplates(t.templates);
    setProviders(p.providers);
    setSelectedId((cur) => cur ?? p.providers[0]?.id ?? null);
    if (p.providers.length === 0) setAdding(true);
  };

  useEffect(() => {
    void reload();
  }, []);

  const run = async (label: string, fn: () => Promise<string | void>) => {
    setBusy(true);
    setFeedback(null);
    try {
      const msg = await fn();
      setFeedback({ ok: true, text: msg || `${label} done` });
      await reload();
      await refreshState();
    } catch (e) {
      setFeedback({ ok: false, text: `${label} failed: ${e instanceof Error ? e.message : String(e)}` });
    } finally {
      setBusy(false);
    }
  };

  const selected = providers.find((p) => p.id === selectedId) ?? null;

  return (
    <div className="page pv-page">
      <div className="pv-head">
        <div>
          <h2>Model providers</h2>
          <p className="hint">The models KAgent can use for chat, voice and fallback. API keys are stored in your desktop keyring.</p>
        </div>
        <div className="row">
          <button disabled={busy} onClick={() => void reload()}>
            Refresh
          </button>
          <button
            className="primary"
            onClick={() => {
              setAdding(true);
              setFeedback(null);
            }}
          >
            Add provider
          </button>
        </div>
      </div>

      <div className="pv-layout">
        <nav className="pv-nav" aria-label="Providers">
          {providers.map((p) => {
            const st = statusOf(p);
            const active = !adding && p.id === selectedId;
            return (
              <button
                key={p.id}
                className={`pv-nav-item${active ? " active" : ""}`}
                onClick={() => {
                  setSelectedId(p.id);
                  setAdding(false);
                  setFeedback(null);
                }}
              >
                <Logo name={p.name} />
                <span className="pv-nav-name">{p.name}</span>
                <span className={`pv-dot ${st}`} title={STATUS_LABEL[st]} />
              </button>
            );
          })}
          {providers.length === 0 && <div className="hint pv-nav-empty">No providers yet</div>}
        </nav>

        <section className="pv-detail">
          {feedback && <div className={`pv-feedback ${feedback.ok ? "ok" : "bad"}`}>{feedback.text}</div>}
          {adding ? (
            <AddProvider
              templates={templates}
              existing={providers}
              busy={busy}
              onCancel={providers.length ? () => setAdding(false) : undefined}
              onAdd={(templateId, apiKey) =>
                void run("Add provider", async () => {
                  const r = await api.post<{ id: string }>("/api/providers", { templateId, apiKey: apiKey || undefined });
                  setAdding(false);
                  setSelectedId(r.id);
                  if (apiKey || templateId === "ollama") await api.post(`/api/providers/${r.id}/fetch-models`).catch(() => undefined);
                  return "Provider added";
                })
              }
            />
          ) : selected ? (
            <ProviderDetail key={selected.id} p={selected} template={templates.find((t) => t.id === selected.templateId)} settings={s.state?.settings} busy={busy} run={run} />
          ) : (
            <p className="hint">Select a provider.</p>
          )}
        </section>
      </div>
    </div>
  );
}

function AddProvider({
  templates,
  existing,
  busy,
  onAdd,
  onCancel,
}: {
  templates: Template[];
  existing: ProviderView[];
  busy: boolean;
  onAdd: (templateId: string, apiKey: string) => void;
  onCancel?: () => void;
}) {
  const [q, setQ] = useState("");
  const [pick, setPick] = useState<Template | null>(null);
  const [key, setKey] = useState("");
  const list = templates.filter((t) => `${t.name} ${t.id} ${t.api.baseUrl}`.toLowerCase().includes(q.toLowerCase()));
  const added = new Set(existing.map((p) => p.templateId));
  const local = pick && (pick.api.type === "ollama-native" || pick.api.baseUrl.includes("127.0.0.1"));
  return (
    <div className="pv-add">
      <div className="pv-detail-head">
        <h3>Add model provider</h3>
        {onCancel && <button onClick={onCancel}>Back to provider details</button>}
      </div>
      {!pick ? (
        <>
          <input className="pv-search" placeholder="Search providers" value={q} onChange={(e) => setQ(e.target.value)} autoFocus />
          <div className="pv-catalog">
            {list.map((t) => (
              <button key={t.id} className="pv-cat-item" onClick={() => setPick(t)}>
                <Logo name={t.name} />
                <span>
                  <span className="pv-cat-name">
                    {t.name}
                    {added.has(t.id) && <span className="pv-tag">added</span>}
                  </span>
                  <span className="hint">{API_FORMATS[t.api.type] ?? t.api.type}</span>
                </span>
              </button>
            ))}
            {list.length === 0 && <p className="hint">No providers found</p>}
          </div>
        </>
      ) : (
        <div className="pv-form">
          <div className="pv-picked">
            <Logo name={pick.name} />
            <div>
              <div className="pv-cat-name">{pick.name}</div>
              <div className="hint">{pick.api.baseUrl}</div>
            </div>
            <button onClick={() => setPick(null)}>Change</button>
          </div>
          {!local && (
            <label className="pv-field">
              <span>API key</span>
              <input type="password" placeholder="Paste the API key" value={key} onChange={(e) => setKey(e.target.value)} autoFocus />
              {pick.keyUrl && (
                <a className="hint" href={pick.keyUrl} target="_blank" rel="noreferrer">
                  Get a key
                </a>
              )}
            </label>
          )}
          <div className="row">
            <button className="primary" disabled={busy || (!local && !key.trim())} onClick={() => onAdd(pick.id, key.trim())}>
              Add provider
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function ProviderDetail({
  p,
  template,
  settings,
  busy,
  run,
}: {
  p: ProviderView;
  template?: Template;
  settings?: { chatModel: string; liveModel?: string; [k: string]: unknown };
  busy: boolean;
  run: (label: string, fn: () => Promise<string | void>) => Promise<void>;
}) {
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.api.baseUrl);
  const [key, setKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [filter, setFilter] = useState("");
  const [onlyEnabled, setOnlyEnabled] = useState(p.models.length > 15);
  const dirty = name !== p.name || baseUrl !== p.api.baseUrl || key.trim() !== "";
  const st = statusOf(p);
  const fallback = (settings?.fallbackModel as string) ?? "";

  const models = useMemo(
    () => p.models.filter((m) => (!onlyEnabled || m.enabled) && m.id.toLowerCase().includes(filter.toLowerCase())),
    [p.models, filter, onlyEnabled],
  );

  const assign = (field: string, ref: string, label: string) => run(label, async () => {
    await api.put("/api/settings", { [field]: ref });
    return `${label}: ${ref.split("::").pop()}`;
  });

  return (
    <div className="pv-provider">
      <div className="pv-detail-head">
        <div className="pv-title">
          <Logo name={p.name} />
          <div>
            <h3>{p.name}</h3>
            <span className={`pv-status ${st}`}>
              <span className={`pv-dot ${st}`} /> {STATUS_LABEL[st]}
            </span>
          </div>
        </div>
        <label className="pv-switch" title={p.enabled ? "Disable provider" : "Enable provider"}>
          <input type="checkbox" checked={p.enabled} onChange={(e) => void run(e.target.checked ? "Enable" : "Disable", async () => void (await api.put(`/api/providers/${p.id}`, { enabled: e.target.checked })))} />
          <span />
        </label>
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
            <input type={showKey ? "text" : "password"} placeholder={p.hasKey ? `Stored (${p.apiKeyMasked}) - type to replace` : "Paste the API key"} value={key} onChange={(e) => setKey(e.target.value)} />
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
                const r = await api.post<{ ok: boolean; latencyMs: number; error?: string; reply?: string }>(`/api/providers/${p.id}/test`, {});
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
          <button
            className="danger"
            disabled={busy}
            onClick={() => {
              if (confirm(`Delete ${p.name}? Its API key is removed from the keyring.`)) void run("Delete", async () => void (await api.del(`/api/providers/${p.id}`)));
            }}
          >
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
            const uses = [settings?.chatModel === ref && "chat", fallback === ref && "fallback", settings?.liveModel === ref && "voice"].filter(Boolean) as string[];
            return (
              <li key={m.id} className={`pv-model${m.enabled ? "" : " off"}`}>
                <label className="pv-switch small" title={m.enabled ? "Disable model" : "Enable model"}>
                  <input type="checkbox" checked={m.enabled} onChange={(e) => void run("Model", async () => void (await api.put(`/api/providers/${p.id}`, { models: [{ id: m.id, enabled: e.target.checked }] })))} />
                  <span />
                </label>
                <span className="pv-model-id">{m.id}</span>
                {uses.map((u) => (
                  <span key={u} className="pv-tag on">
                    {u}
                  </span>
                ))}
                <span className="spacer" />
                {m.enabled && !isLive && !uses.includes("chat") && (
                  <button className="pv-mini" disabled={busy} onClick={() => void assign("chatModel", ref, "Chat model")}>
                    Use for chat
                  </button>
                )}
                {m.enabled && !isLive && !uses.includes("fallback") && (
                  <button className="pv-mini" disabled={busy} onClick={() => void assign("fallbackModel", ref, "Fallback model")}>
                    Fallback
                  </button>
                )}
                {m.enabled && isLive && !uses.includes("voice") && (
                  <button className="pv-mini" disabled={busy} onClick={() => void assign("liveModel", ref, "Voice model")}>
                    Use for voice
                  </button>
                )}
                {!isLive && (
                  <button
                    className="pv-mini"
                    disabled={busy}
                    onClick={() =>
                      void run("Test", async () => {
                        const r = await api.post<{ ok: boolean; latencyMs: number; error?: string; reply?: string }>(`/api/providers/${p.id}/test`, { modelId: m.id });
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
    </div>
  );
}
