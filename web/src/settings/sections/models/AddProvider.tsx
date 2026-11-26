// Add a provider (models.md section 7.2): pick a template from the catalog, then give
// its key. The `custom` template asks for a name, an address and the message format.
// The key goes to the desktop keyring through the daemon; a local provider needs none.
import { useId, useState } from "react";
import type { NewProvider, ProviderView, Template } from "../../../api";
import Dropdown from "../../Dropdown";
import Catalog, { Logo } from "./Catalog";
import { apiFormatOptions } from "./present";

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
  onAdd: (request: NewProvider) => void;
  onCancel?: () => void;
}) {
  const [pick, setPick] = useState<Template | null>(null);
  const formatId = useId();
  const [key, setKey] = useState("");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiType, setApiType] = useState("openai-chat-completions");
  const custom = pick?.id === "custom";
  const local = !!pick && !custom && (pick.api.type === "ollama-native" || pick.api.baseUrl.includes("127.0.0.1"));
  const ready = custom ? name.trim() !== "" && baseUrl.trim() !== "" : local || key.trim() !== "";
  const add = () => {
    if (!pick) return;
    onAdd(custom ? { templateId: pick.id, name: name.trim(), baseUrl: baseUrl.trim(), apiType, ...(key.trim() ? { apiKey: key.trim() } : {}) } : { templateId: pick.id, ...(key.trim() ? { apiKey: key.trim() } : {}) });
  };
  return (
    <div className="pv-add">
      <div className="pv-detail-head">
        <h3>{custom ? "Create custom provider" : "Add model provider"}</h3>
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
              <div className="hint">{custom ? "Any server with an OpenAI-style API: name it and say where it is." : pick.api.baseUrl}</div>
            </div>
            <button onClick={() => setPick(null)}>Change</button>
          </div>
          {custom && (
            <>
              <label className="pv-field">
                <span>Name</span>
                <input placeholder="My provider" value={name} autoFocus onChange={(e) => setName(e.target.value)} />
              </label>
              <label className="pv-field">
                <span>Base URL</span>
                <input placeholder="https://api.example.com/v1" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} />
              </label>
              <div className="pv-field">
                <span id={`${formatId}-label`}>API format</span>
                <Dropdown id={`${formatId}-value`} ariaLabel="API format" value={apiType} options={apiFormatOptions()} onChange={setApiType} />
              </div>
            </>
          )}
          {(custom || !local) && (
            <label className="pv-field">
              <span>API key{custom && " (optional)"}</span>
              <input type="password" placeholder={custom ? "None needed by some servers" : "Paste the API key"} value={key} autoFocus={!custom} onChange={(e) => setKey(e.target.value)} />
              {pick.keyUrl && (
                <a className="hint" href={pick.keyUrl} target="_blank" rel="noreferrer">
                  Get a key
                </a>
              )}
            </label>
          )}
          <div className="row">
            <button className="primary" disabled={busy || !ready} onClick={add}>
              Add provider
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
