// Web (web.md section 5): the engine the web_search tool asks, the Brave Search key and
// the address of a SearXNG instance. The key goes to the daemon, which keeps it in the
// desktop keyring: this page only hears whether one is stored.
import { useEffect, useState, type FormEvent } from "react";
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { Badge, Field, GroupCard, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import { webApi, type KeyStatus, type SearchEngine, type SearchSettings } from "../web/api";
import "../web/web.css";

const ENGINES: { id: SearchEngine; label: string; hint: string }[] = [
  { id: "duckduckgo", label: "DuckDuckGo", hint: "No key. DuckDuckGo limits a client that sends many searches; the tool then says so." },
  { id: "brave", label: "Brave Search API", hint: "Needs the key of api.search.brave.com. It stays in the daemon and your desktop keyring." },
  { id: "searxng", label: "SearXNG", hint: "Your own instance, or one you trust. The instance must allow its JSON format." },
  { id: "none", label: "Off", hint: "web_search answers that search is off. web_fetch keeps working." },
];

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** An http or https address with a host, and no password in it: what the daemon accepts. */
function isAddress(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return (url.protocol === "http:" || url.protocol === "https:") && url.hostname !== "" && url.username === "" && url.password === "";
  } catch {
    return false;
  }
}

/** The Brave key: whether one is stored, a field for another, and the way to forget it. */
function BraveKey({ report }: { report: (text: string, failed?: boolean) => void }) {
  const [status, setStatus] = useState<KeyStatus | null>(null);
  const [key, setKey] = useState("");
  const [working, setWorking] = useState(false);
  const configured = status?.configured === true;

  // Whether a key is stored, when the card shows.
  useEffect(() => {
    let open = true;
    webApi.keyStatus().then(
      (next) => {
        if (open) setStatus(next);
      },
      () => {},
    );
    return () => {
      open = false;
    };
  }, []);

  /** One action of the card: its result goes to the header, a failure too. */
  const act = async (what: string, run: () => Promise<void>) => {
    setWorking(true);
    try {
      await run();
    } catch (e) {
      report(`${what} failed: ${reason(e)}`, true);
    } finally {
      setWorking(false);
    }
  };
  const saveKey = (e: FormEvent) => {
    e.preventDefault();
    const value = key.trim();
    if (!value) return;
    void act("Saving the key", async () => {
      setStatus(await webApi.setKey(value));
      setKey("");
      report("Key saved. The next search uses it.");
    });
  };
  const deleteKey = async () => {
    await webApi.clearKey();
    setStatus({ configured: false });
    report("Key deleted. Brave Search has no key until you save one.");
  };

  return (
    <div className="web-key" data-configured={configured ? "true" : "false"}>
      <div className="web-key-line">
        <Badge tone={configured ? "ok" : "neutral"}>{configured ? "Key stored" : "No key stored"}</Badge>
        {status === null && <span className="hint">No answer from the daemon.</span>}
        {status !== null && !configured && <span className="hint">Without a key, web_search says there is none.</span>}
      </div>
      <form className="ui-form" onSubmit={saveKey}>
        <Field label="Brave Search key" hint="From api.search.brave.com. It stays in the daemon and your desktop keyring; this page only hears whether there is one.">
          <input
            type="password"
            aria-label="Brave Search key"
            autoComplete="off"
            spellCheck={false}
            value={key}
            placeholder={configured ? "A key is stored; paste another to replace it" : "BSA..."}
            onChange={(e) => setKey(e.target.value)}
          />
        </Field>
        <div className="row">
          <button className="primary" type="submit" disabled={working || key.trim() === ""}>
            Save key
          </button>
          {configured && (
            <button type="button" className="danger" disabled={working} onClick={() => void act("Deleting the key", deleteKey)}>
              Delete key
            </button>
          )}
        </div>
      </form>
    </div>
  );
}

export default function WebSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save, busy, report } = useSettingPatch();
  const search = settings?.search as SearchSettings | undefined;
  const engine: SearchEngine = search?.engine ?? "duckduckgo";
  const [url, setUrl, urlDirty] = useDraft(search?.searxUrl ?? "");
  const urlOk = isAddress(url);

  const saveUrl = (e: FormEvent) => {
    e.preventDefault();
    if (urlDirty && urlOk) void save("search", { searxUrl: url.trim() });
  };

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Web search" description="The engine the web_search tool asks. What it finds is data from the web, never instructions.">
        <div className="web-radios" role="radiogroup" aria-label="Search engine">
          {ENGINES.map((e) => (
            <label key={e.id} className="web-radio">
              <input type="radio" name="search-engine" value={e.id} checked={engine === e.id} disabled={!settings} onChange={() => void save("search", { engine: e.id })} />
              <span>
                <span className="web-radio-label">{e.label}</span>
                <span className="hint">{e.hint}</span>
              </span>
            </label>
          ))}
        </div>
        {engine === "brave" && <BraveKey report={report} />}
        {engine === "searxng" && (
          <form className="ui-form" onSubmit={saveUrl}>
            <Field label="SearXNG address" hint="Such as https://searx.example.org. The instance may be on this machine or your network: you set it, the agent never does.">
              <input type="url" aria-label="SearXNG address" value={url} placeholder="https://searx.example.org" spellCheck={false} disabled={!settings} onChange={(e) => setUrl(e.target.value)} />
            </Field>
            <div className="row">
              <button className="primary" type="submit" disabled={!urlDirty || !urlOk || busy}>
                Save address
              </button>
              {!urlOk && urlDirty && <span className="ui-inline-bad">The address must be an http or https address, without a password in it.</span>}
            </div>
          </form>
        )}
      </GroupCard>
    </SectionPage>
  );
}
