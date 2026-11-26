// Browser: which browser the agent drives. KAgent's own window (a separate profile on a
// DevTools port), or the owner's running browser, attached through the remote debugging
// it allows itself (browser.md). The backend and the trust level are read when the
// daemon starts; the profile folder applies as soon as it is saved.
import type { FormEvent } from "react";
import { useStore } from "../../store";
import type { SectionProps } from "../registry";
import { GroupCard, Field, SectionPage } from "../ui";
import { useDraft } from "../useDraft";
import { useSettingPatch } from "../useSettingPatch";
import { AttachCard } from "../browser/AttachCard";
import type { BrowserBackend } from "../browser/api";
import "../browser/browser.css";

const DEFAULT_PORT = 9333;

const BACKENDS: { id: BrowserBackend; label: string; hint: string }[] = [
  { id: "own", label: "KAgent's own window (separate profile)", hint: "KAgent starts a Brave or Chrome window with a profile of its own. You sign in there once for the sites the agent should use; your main browser stays out of its reach." },
  { id: "attach", label: "My running browser: attach through its DevTools remote debugging", hint: "The agent reads and acts in the tabs of the browser you are signed in to. You allow each connection in the browser, and only tabs the agent opened can be closed." },
];

/** KAgent's own window: the DevTools port of its browser and the program to start. */
function OwnWindow() {
  const settings = useStore().state?.settings;
  const { save, busy } = useSettingPatch();
  const [browser, setBrowser, dirty] = useDraft({ port: settings?.browser?.port ?? DEFAULT_PORT, binary: settings?.browser?.binary ?? "" });
  const valid = Number.isInteger(browser.port) && browser.port >= 1024 && browser.port <= 65535;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (dirty && valid) void save("browser", browser);
  };

  return (
    <GroupCard title="KAgent's own window" description="The Brave or Chrome window KAgent drives over DevTools, in its own profile.">
      <form className="ui-form" onSubmit={submit}>
        <Field label="DevTools port" hint="1024 to 65535. A taken port keeps the browser from opening.">
          <input type="number" min={1024} max={65535} value={browser.port || ""} onChange={(e) => setBrowser({ ...browser, port: Number(e.target.value) })} />
        </Field>
        <Field label="Binary" hint="The brave or chrome program to start. Empty: KAgent detects one.">
          <input value={browser.binary} placeholder="/usr/bin/brave" onChange={(e) => setBrowser({ ...browser, binary: e.target.value })} />
        </Field>
        <div className="row">
          <button className="primary" type="submit" disabled={!dirty || !valid || busy}>
            Save browser
          </button>
          {!valid && dirty && <span className="ui-inline-bad">The port must be between 1024 and 65535.</span>}
        </div>
      </form>
    </GroupCard>
  );
}

export default function BrowserSection({ title, description }: SectionProps) {
  const settings = useStore().state?.settings;
  const { save } = useSettingPatch();
  const backend: BrowserBackend = settings?.browser?.backend ?? "own";

  return (
    <SectionPage title={title} description={description}>
      <GroupCard title="Which browser" description="Where the browser tool acts.">
        <div className="br-radios" role="radiogroup" aria-label="Which browser">
          {BACKENDS.map((b) => (
            <label key={b.id} className="br-radio">
              <input type="radio" name="browser-backend" value={b.id} checked={backend === b.id} onChange={() => void save("browser", { backend: b.id })} />
              <span>
                <span className="br-radio-label">{b.label}</span>
                <span className="hint">{b.hint}</span>
              </span>
            </label>
          ))}
        </div>
        <p className="hint ui-note">The backend applies after a daemon restart.</p>
      </GroupCard>
      {backend === "attach" ? <AttachCard /> : <OwnWindow />}
    </SectionPage>
  );
}
