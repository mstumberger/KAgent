// One provider (models.md section 7.3): its name, whether it is on, its API format,
// address and key (a field saves as it is left: one `PUT` each), what can be done to
// it, and below it its models.
import { useRef, useState, type KeyboardEvent } from "react";
import { api, type ProviderView, type Settings, type Template } from "../../../api";
import { Badge, Confirm, Toggle, type BadgeTone } from "../../ui";
import Dropdown from "../../Dropdown";
import { Logo } from "./Catalog";
import ModelList from "./ModelList";
import { apiFormatOptions, type Run } from "./present";
import { statusOf, STATUS_LABEL, type Status } from "./shared";

const TONE: Record<Status, BadgeTone> = { ok: "ok", error: "bad", nokey: "warn", off: "neutral" };

export default function ProviderCard({
  p,
  template,
  settings,
  busy,
  run,
  reload,
}: {
  p: ProviderView;
  template?: Template;
  settings?: Settings;
  busy: boolean;
  run: Run;
  /** The lists are read again after a field was saved. */
  reload: () => void;
}) {
  const [name, setName] = useState(p.name);
  const [baseUrl, setBaseUrl] = useState(p.api.baseUrl);
  const [key, setKey] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [askDelete, setAskDelete] = useState(false);
  const [note, setNote] = useState<{ text: string; failed: boolean } | null>(null);
  // Escape puts a field back; the blur that follows must not save it.
  const skip = useRef(false);
  const custom = p.templateId === "custom";
  const st = statusOf(p);

  /** One `PUT` for one field. The line under the card says how it went. */
  const saveField = async (body: Record<string, unknown>): Promise<boolean> => {
    try {
      await api.put(`/api/providers/${p.id}`, body);
      setNote({ text: "Saved", failed: false });
      reload();
      return true;
    } catch (e) {
      setNote({ text: `Not saved: ${e instanceof Error ? e.message : String(e)}`, failed: true });
      return false;
    }
  };
  const saveKey = async () => {
    // The field is cleared after its save.
    if (key.trim() && (await saveField({ apiKey: key.trim() }))) setKey("");
  };
  /** On leaving a field: one `PUT` when it changed. After Escape there is nothing to save. */
  const leave = (changed: boolean, save: () => unknown) => () => {
    if (skip.current) {
      skip.current = false;
      return;
    }
    if (changed) void save();
  };
  /** Enter saves (the blur does); Escape puts the stored value back. */
  const onKey = (revert: () => void) => (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Enter") e.currentTarget.blur();
    else if (e.key === "Escape") {
      e.preventDefault();
      skip.current = true;
      revert();
      e.currentTarget.blur();
    }
  };

  return (
    <div className="pv-provider">
      <div className="pv-detail-head">
        <div className="pv-title">
          <Logo name={p.name || "?"} />
          <div className="pv-title-text">
            <input className="pv-title-input" aria-label="Provider name" value={name} onChange={(e) => setName(e.target.value)} onBlur={leave(name !== p.name, () => saveField({ name }))} onKeyDown={onKey(() => setName(p.name))} />
            <Badge tone={TONE[st]}>{STATUS_LABEL[st]}</Badge>
          </div>
        </div>
        <Toggle
          label="Enabled"
          checked={p.enabled}
          disabled={busy}
          onChange={(v) => void run(v ? "Enable" : "Disable", async () => void (await api.put(`/api/providers/${p.id}`, { enabled: v })))}
        />
      </div>
      {p.lastError && <div className="pv-feedback bad">{p.lastError}</div>}

      <div className="pv-form">
        <div className="pv-field">
          <span id="pv-api-format">API format</span>
          <Dropdown id="pv-api-format-value" ariaLabel="API format" value={p.api.type} options={apiFormatOptions()} onChange={(type) => void saveField({ apiType: type })} />
        </div>
        <label className="pv-field">
          <span>Base URL</span>
          <input
            value={custom ? baseUrl : p.api.baseUrl}
            readOnly={!custom}
            title={custom ? undefined : "The address of its template (read only)"}
            placeholder={template?.api.baseUrl || "https://api.example.com/v1"}
            onChange={(e) => setBaseUrl(e.target.value)}
            onBlur={leave(baseUrl !== p.api.baseUrl, () => saveField({ baseUrl }))}
            onKeyDown={onKey(() => setBaseUrl(p.api.baseUrl))}
          />
        </label>
        <label className="pv-field">
          <span>API key</span>
          <span className="pv-key">
            <input
              type={showKey ? "text" : "password"}
              autoComplete="off"
              aria-label="API key"
              placeholder={p.hasKey ? `Stored (${p.apiKeyMasked}) - type to replace` : "Paste the API key"}
              value={key}
              onChange={(e) => {
                setKey(e.target.value);
                skip.current = false;
              }}
              onBlur={leave(key.trim() !== "", saveKey)}
              onKeyDown={onKey(() => setKey(""))}
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
      {note && (
        <div className={`pv-feedback ${note.failed ? "bad" : "ok"}`} role="status">
          {note.text}
        </div>
      )}

      <ModelList p={p} template={template} settings={settings} busy={busy} run={run} reload={reload} />

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

type TestResult = { ok: boolean; latencyMs: number; error?: string };
