// The Brave Search key (web.md section 3): whether one is stored, a field for another, and the way
// to forget it. The key goes to the daemon, which keeps it in the desktop keyring: this card only
// hears whether one is stored.
import { useEffect, useState, type FormEvent } from "react";
import { Badge, Field } from "../ui";
import { webApi, type KeyStatus } from "./api";

const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function BraveKey({ report }: { report: (text: string, failed?: boolean) => void }) {
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
        {status !== null && !configured && <span className="hint">Without a key, web_search skips Brave and tries the next engine.</span>}
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
