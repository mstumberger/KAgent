// The owner's running browser (browser.md sections 2, 6 and 7): its profile folder, how
// far the agent may act in the owner's own tabs, the connection, and its status.
import { useEffect, useState, type FormEvent } from "react";
import { useStore } from "../../store";
import { Badge, Field, GroupCard, Row, type BadgeTone } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import { browserApi, INSTRUCTION_URL, type AttachStatus, type AttachTrust, type BrowserStatus } from "./api";
import Select from "../Select";

const TRUST: { id: AttachTrust; label: string; hint: string }[] = [
  { id: "strict", label: "Strict (asks every time)", hint: "In your own tabs, navigating, scrolling, clicks, typing and key presses show a card every time, in every mode, and nothing is remembered. Applies after a daemon restart." },
  { id: "normal", label: "Normal (as in KAgent's own window)", hint: "In your own tabs, clicks, typing and key presses ask as in KAgent's own window; navigating and scrolling do not. Tabs the agent opened always work as its own window. Applies after a daemon restart." },
];

/** The state of the connection, for the badge of the status card. */
function stateOf(attach: AttachStatus | undefined): { tone: BadgeTone; text: string } {
  if (!attach) return { tone: "neutral", text: "No answer from the daemon" };
  if (attach.connected) return { tone: "ok", text: "Connected" };
  if (attach.debugging === "off") return { tone: "bad", text: "Remote debugging is off" };
  if (attach.debugging === "unknown") return { tone: "bad", text: "No browser profile found" };
  return { tone: "warn", text: "Not connected" };
}

/** The instruction line of browser.md section 6. */
function instructionLine(browser: string): string {
  return `In ${browser}, open ${INSTRUCTION_URL} and tick 'Allow remote debugging for this browser instance'.`;
}

export function AttachCard() {
  const settings = useStore().state?.settings;
  const { save, report, busy } = useSettingPatch();
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [working, setWorking] = useState(false);
  const trust = settings?.browser?.attachTrust ?? "strict";
  const [folder, setFolder, dirty] = useDraft(settings?.browser?.attachProfile ?? "");
  const attach = status?.attach;
  const detected = attach?.profile ?? "";
  const state = stateOf(attach);

  // The status when the card opens. It reads the port file; the browser is not contacted.
  useEffect(() => {
    let open = true;
    browserApi.status().then(
      (s) => {
        if (open) setStatus(s);
      },
      () => {
        if (open) setStatus(null);
      },
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
      report(`${what} failed: ${e instanceof Error ? e.message : String(e)}`, true);
    } finally {
      setWorking(false);
    }
  };
  const connect = () =>
    act("Connecting", async () => {
      const s = await browserApi.connect();
      setStatus(s);
      report(s.attach.connected ? `Connected: ${s.attach.tabs} ${s.attach.tabs === 1 ? "tab" : "tabs"}` : `Not connected: ${s.attach.detail}`, !s.attach.connected);
    });
  const disconnect = () =>
    act("Disconnecting", async () => {
      await browserApi.disconnect();
      setStatus(await browserApi.status());
      report("Disconnected");
    });
  const saveFolder = (e: FormEvent) => {
    e.preventDefault();
    if (!dirty) return;
    void act("Saving the folder", async () => {
      if (await save("browser", { attachProfile: folder.trim() })) setStatus(await browserApi.status());
    });
  };
  const copyLink = () => navigator.clipboard?.writeText(INSTRUCTION_URL).then(() => report("Link copied"), () => report("Copy failed", true));

  return (
    <>
      <GroupCard title="My running browser" description="KAgent attaches through the remote debugging the browser allows itself. Every connection asks Allow in the browser's own window.">
        <form className="ui-form" onSubmit={saveFolder}>
          <Field label="Profile folder" hint={detected ? `Detected: ${detected}. The user data folder of your browser; empty: the first of Brave, Chrome, Edge and Chromium that is installed.` : "The user data folder of your browser (Brave, Chrome, Edge or Chromium). Empty: the first of them that is installed."}>
            <input value={folder} placeholder={detected || "/home/you/.config/BraveSoftware/Brave-Browser"} onChange={(e) => setFolder(e.target.value)} />
          </Field>
          <div className="row">
            <button className="primary" type="submit" disabled={!dirty || busy}>
              Save folder
            </button>
          </div>
        </form>
        <Row label="Trust level" hint={TRUST.find((t) => t.id === trust)?.hint}>
          <Select aria-label="Trust level" value={trust} onChange={(e) => void save("browser", { attachTrust: e.target.value as AttachTrust })}>
            {TRUST.map((t) => (
              <option key={t.id} value={t.id}>
                {t.label}
              </option>
            ))}
          </Select>
        </Row>
        <Row label="Connection" hint="Connect makes the browser ask: click Allow in its window. Without a call for ten minutes, KAgent disconnects.">
          <div className="row">
            <button type="button" disabled={working || attach?.connected === true} onClick={() => void connect()}>
              {working ? "Working..." : "Connect"}
            </button>
            <button type="button" disabled={working || !attach?.connected} onClick={() => void disconnect()}>
              Disconnect
            </button>
          </div>
        </Row>
      </GroupCard>
      <GroupCard title="Status">
        <div className="br-status">
          <div className="br-status-line">
            <Badge tone={state.tone}>{state.text}</Badge>
            {attach?.browser && <span className="br-status-name">{attach.browser}</span>}
            {attach?.connected && (
              <span className="hint">
                {attach.tabs} {attach.tabs === 1 ? "tab" : "tabs"}
              </span>
            )}
          </div>
          {/* What the daemon says (browser.md section 6), also when debugging is off. */}
          {attach?.detail && <p className="hint">{attach.detail}</p>}
          {attach && attach.debugging !== "on" && (
            <>
              <p className="br-instruction">{instructionLine(attach.browser || "your browser")}</p>
              <div className="row">
                <button type="button" title={`Copy ${INSTRUCTION_URL}`} onClick={copyLink}>
                  Copy link
                </button>
              </div>
            </>
          )}
          {attach?.debugging === "on" && !attach.connected && <p className="hint">The next call of the agent, or Connect, makes the browser ask Allow.</p>}
          {status && status.backend !== "attach" && <p className="hint ui-note">KAgent still uses its own window until the daemon restarts.</p>}
        </div>
      </GroupCard>
    </>
  );
}
