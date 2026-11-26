// Add a provider: pick a template from the catalog, then give its key. The key goes to
// the desktop keyring through the daemon; a local provider needs none.
import { useState } from "react";
import type { ProviderView, Template } from "../../../api";
import Catalog, { Logo } from "./Catalog";

export default function AddProvider({
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
  const [pick, setPick] = useState<Template | null>(null);
  const [key, setKey] = useState("");
  const local = !!pick && (pick.api.type === "ollama-native" || pick.api.baseUrl.includes("127.0.0.1"));
  return (
    <div className="pv-add">
      <div className="pv-detail-head">
        <h3>Add model provider</h3>
        {onCancel && <button onClick={onCancel}>Back to provider details</button>}
      </div>
      {!pick ? (
        <Catalog templates={templates} existing={existing} onPick={setPick} />
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
